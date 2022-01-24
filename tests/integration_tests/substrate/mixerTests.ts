import {
  generate_proof_js,
  JsNote,
  JsNoteBuilder,
  ProofInputBuilder,
} from '@webb-tools/wasm-utils/njs/wasm-utils';
import { ApiPromise, WsProvider } from '@polkadot/api';
import { decodeAddress, Keyring } from '@polkadot/keyring';
import { KeyringPair } from '@polkadot/keyring/types';
import { hexToU8a, u8aToHex } from '@polkadot/util';
import {
  generateSubstrateMixerWithdrawRequest,
  getRelayerSubstrateConfig,
  handleMessage,
  RelayerChainConfig,
  Result,
  sleep,
  startWebbRelayer,
} from '../../relayerUtils';
import { ChildProcessWithoutNullStreams } from 'child_process';
import WebSocket from 'ws';
import path from 'path';
import * as fs from 'fs';

let apiPromise: ApiPromise | null = null;
let keyring: {
  bob: KeyringPair;
  alice: KeyringPair;
  charlie: KeyringPair;
} | null = null;
let relayer: ChildProcessWithoutNullStreams;
let relayerEndpoint: string;

let relayerChain1Info: RelayerChainConfig;

let client: WebSocket;

async function preparePolkadotApi() {
  const wsProvider = new WsProvider('ws://127.0.0.1:9944');
  const api = await ApiPromise.create({
    provider: wsProvider,
    rpc: {
      mt: {
        getLeaves: {
          description: 'Query for the tree leaves',
          params: [
            {
              name: 'tree_id',
              type: 'u32',
              isOptional: false,
            },
            {
              name: 'from',
              type: 'u32',
              isOptional: false,
            },
            {
              name: 'to',
              type: 'u32',
              isOptional: false,
            },
            {
              name: 'at',
              type: 'Hash',
              isOptional: true,
            },
          ],
          type: 'Vec<[u8; 32]>',
        },
      },
    },
  });
  return api.isReady;
}

const BOBPhrase =
  'asthma early danger glue satisfy spatial decade wing organ bean census announce';

function getKeyring() {
  if (keyring) {
    return keyring;
  }
  const k = new Keyring({ type: 'sr25519' });
  const bob = k.addFromMnemonic(BOBPhrase);
  const alice = k.addFromUri('//Alice');
  const charlie = k.addFromUri('//Charlie');
  keyring = {
    bob,
    alice,
    charlie,
  };
  return keyring;
}

async function transferBalance(api: ApiPromise) {
  console.log('Transfer balances');
  const amount = '1_000_000_000_000';
  const { charlie, bob, alice } = getKeyring();
  // transfer to alice
  // @ts-ignore
  const aliceTransfer = api.tx.balances.transfer(alice.address, amount);
  console.log(
    'Transferring native funds to Alice ',
    aliceTransfer.hash.toString()
  );
  await aliceTransfer.signAndSend(charlie, { nonce: -1 });
  // transfer to test accounts
  // @ts-ignore
  const bobTransfer = api.tx.balances.transfer(bob.address, amount);
  console.log(
    'Transferring native funds to Bob ',
    bob.address,
    ` ${bobTransfer.hash.toString()}`
  );
  await bobTransfer.signAndSend(charlie, { nonce: -1 });
}

async function sendWebbToken(api: ApiPromise, receiver: KeyringPair) {
  const { alice: sudoPair } = getKeyring();
  console.log(
    `Setting Bob ${receiver.address} balance to ${100_000_000_000_000} `
  );
  return new Promise((resolve, reject) => {
    const address = api.createType('MultiAddress', { Id: receiver.address });
    // @ts-ignore
    api.tx.sudo
      .sudo(
        // @ts-ignore
        api.tx.currencies.updateBalance(address, 0, 100_000_000_000_000)
      )
      .signAndSend(sudoPair, (res) => {
        if (res.isFinalized || res.isCompleted) {
          resolve(null);
        }
        if (res.isError) {
          reject(res.dispatchError);
        }
      });
  });
}

async function depositMixerBnX5_5(api: ApiPromise, depositer: KeyringPair) {
  console.log(`Depositing to tree id = 0 ; mixer bn254`);
  let noteBuilder = new JsNoteBuilder();
  noteBuilder.prefix('webb.mixer');
  noteBuilder.version('v1');

  noteBuilder.sourceChainId('1');
  noteBuilder.targetChainId('1');

  noteBuilder.tokenSymbol('WEBB');
  noteBuilder.amount('1');
  noteBuilder.denomination('18');

  noteBuilder.backend('Arkworks');
  noteBuilder.hashFunction('Poseidon');
  noteBuilder.curve('Bn254');
  noteBuilder.width('5');
  noteBuilder.exponentiation('5');
  const note = noteBuilder.build();
  const leaf = note.getLeafCommitment();
  console.log(`deposited leaf ${u8aToHex(leaf)}`);
  console.log(`Deposit note ${note.serialize()}`);
  //@ts-ignore
  const depositTx = api.tx.mixerBn254.deposit(0, leaf);
  await depositTx.signAndSend(depositer);
  return note;
}

async function fetchTreeLeaves(
  api: ApiPromise,
  treeId: string | number
): Promise<Uint8Array[]> {
  let done = false;
  let from = 0;
  let to = 511;
  const leaves: Uint8Array[] = [];

  while (done === false) {
    const treeLeaves: any[] = await (api.rpc as any).mt.getLeaves(
      treeId,
      from,
      to
    );
    if (treeLeaves.length === 0) {
      done = true;
      break;
    }
    leaves.push(...treeLeaves.map((i) => i.toU8a()));
    from = to;
    to = to + 511;
  }
  return leaves;
}

async function withdrawMixerBnX5_5(
  api: ApiPromise,
  signer: KeyringPair,
  note: JsNote
) {
  const accountId = signer.address;
  const relayerAccountId = relayerChain1Info.beneficiary;

  const addressHex = u8aToHex(decodeAddress(accountId));
  const relayerAddressHex = u8aToHex(decodeAddress(relayerAccountId));
  // fetch leaves
  const leaves = await fetchTreeLeaves(api, 0);
  const proofInputBuilder = new ProofInputBuilder();
  const leafHex = u8aToHex(note.getLeafCommitment());
  proofInputBuilder.setNote(note);
  proofInputBuilder.setLeaves(leaves);
  const leafIndex = leaves.findIndex((l) => u8aToHex(l) === leafHex);
  console.log(
    leafHex,
    leafIndex,
    leaves.map((i) => u8aToHex(i))
  );
  proofInputBuilder.setLeafIndex(String(leafIndex));

  proofInputBuilder.setFee('0');
  proofInputBuilder.setRefund('0');

  proofInputBuilder.setRecipient(addressHex.replace('0x', ''));
  proofInputBuilder.setRelayer(relayerAddressHex.replace('0x', ''));
  const pkPath = path.join(
    // tests path
    process.cwd(),
    '..',
    'protocol-substrate-fixtures',
    'mixer',
    'bn254',
    'x5',
    'proving_key_uncompresed.bin'
  );
  const pk = fs.readFileSync(pkPath);

  proofInputBuilder.setPk(pk.toString('hex'));

  const proofInput = proofInputBuilder.build_js();
  console.log('Generating Zero knowledge proof');
  const proofPayload = generate_proof_js(proofInput);

  const req = generateSubstrateMixerWithdrawRequest(
    0,
    0,
    0,

    signer.address,
    relayerAccountId,

    hexToU8a(`0x${proofPayload.nullifierHash}`),
    hexToU8a(`0x${proofPayload.root}`),
    hexToU8a(`0x${proofPayload.proof}`)
  );

  return req;
}

describe('Mixer tests', function () {
  // increase the timeout for relayer tests
  this.timeout(120_000);

  before(async function () {
    [relayer, relayerEndpoint] = await startWebbRelayer(8888);
    console.log(relayer.pid);
    await sleep(1500); // wait for the relayer start-up
    relayerChain1Info = await getRelayerSubstrateConfig(
      'localnode',
      relayerEndpoint
    );

    apiPromise = await preparePolkadotApi();

    client = new WebSocket(`${relayerEndpoint.replace('http', 'ws')}/ws`);
    await new Promise((resolve) => client.on('open', resolve));
    console.log('Connected to Relayer!');
  });
  it('should relay successfully', async function () {
    const { bob } = getKeyring();

    await transferBalance(apiPromise!);

    await sendWebbToken(apiPromise!, bob);

    let note: JsNote;
    let req: any;

    note = await depositMixerBnX5_5(apiPromise!, bob);

    await sleep(10_000);

    req = await withdrawMixerBnX5_5(apiPromise!, bob, note!);

    if (client.readyState === client.OPEN) {
      const data = JSON.stringify(req);
      console.log('Sending Proof to the Relayer ..');
      console.log('=>', data);
      client.send(data, (err) => {
        console.log('Proof Sent!');
        if (err !== undefined) {
          console.log('!!Error!!', err);
          throw new Error('Client error sending proof');
        }
      });
    } else {
      console.error('Relayer Connection closed!');
      throw new Error('Client error, not OPEN');
    }

    await new Promise((resolve, reject) => {
      client.on('message', async (data) => {
        console.log('Received data from the relayer');
        console.log('<==', data);
        const msg = JSON.parse(data as string);
        const result = handleMessage(msg);
        if (result === Result.Errored) {
          reject(msg);
        } else if (result === Result.Continue) {
          // all good.
          return;
        } else if (result === Result.CleanExit) {
          console.log('Transaction Done and Relayed Successfully!');
          resolve(msg);
        }
      });
    });
  });

  after(function () {
    client?.terminate();
    relayer.kill('SIGINT');
  });
});
