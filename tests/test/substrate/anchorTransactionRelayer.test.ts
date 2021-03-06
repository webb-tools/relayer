/*
 * Copyright 2022 Webb Technologies Inc.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 *
 */
// This our basic Substrate Anchor Transaction Relayer Tests.
// These are for testing the basic relayer functionality. which is just to relay transactions for us.

import '@webb-tools/types';
import { expect } from 'chai';
import getPort, { portNumbers } from 'get-port';
import temp from 'temp';
import path from 'path';
import fs from 'fs';
import isCi from 'is-ci';
import child from 'child_process';
import {
  WebbRelayer,
  Pallet,
  LeavesCacheResponse,
} from '../../lib/webbRelayer.js';
import { LocalProtocolSubstrate } from '../../lib/localProtocolSubstrate.js';
import {
  UsageMode,
  defaultEventsWatcherValue,
} from '../../lib/substrateNodeBase.js';
import { ApiPromise, Keyring } from '@polkadot/api';
import { u8aToHex, hexToU8a } from '@polkadot/util';
import { SubmittableExtrinsic } from '@polkadot/api/types';
import { decodeAddress } from '@polkadot/util-crypto';
import {
  Note,
  NoteGenInput,
  ProvingManagerSetupInput,
  ArkworksProvingManager,
} from '@webb-tools/sdk-core';

describe('Substrate Anchor Transaction Relayer', function () {
  const tmpDirPath = temp.mkdirSync();
  let aliceNode: LocalProtocolSubstrate;
  let bobNode: LocalProtocolSubstrate;

  let webbRelayer: WebbRelayer;

  before(async () => {
    const usageMode: UsageMode = isCi
      ? { mode: 'docker', forcePullImage: false }
      : {
          mode: 'host',
          nodePath: path.resolve(
            '../../protocol-substrate/target/release/webb-standalone-node'
          ),
        };
    const enabledPallets: Pallet[] = [
      {
        pallet: 'AnchorBn254',
        eventsWatcher: defaultEventsWatcherValue,
      },
    ];

    aliceNode = await LocalProtocolSubstrate.start({
      name: 'substrate-alice',
      authority: 'alice',
      usageMode,
      ports: 'auto',
      enabledPallets,
    });

    bobNode = await LocalProtocolSubstrate.start({
      name: 'substrate-bob',
      authority: 'bob',
      usageMode,
      ports: 'auto',
    });

    // Wait until we are ready and connected
    const api = await aliceNode.api();
    await api.isReady;

    let chainId = await aliceNode.getChainId();

    await aliceNode.writeConfig(`${tmpDirPath}/${aliceNode.name}.json`, {
      suri: '//Charlie',
      chainId: chainId,
    });

    // now start the relayer
    const relayerPort = await getPort({ port: portNumbers(8000, 8888) });
    webbRelayer = new WebbRelayer({
      port: relayerPort,
      tmp: true,
      configDir: tmpDirPath,
      showLogs: false,
    });
    await webbRelayer.waitUntilReady();
  });

  it('number of deposits made should be equal to number of leaves in cache', async () => {
    const api = await aliceNode.api();
    const account = createAccount('//Dave');
    // Make multiple deposits
    const noOfDeposit = 3;
    for (let i = 0, len = noOfDeposit; i < len; i++) {
      const note = await makeDeposit(api, aliceNode, account);
    }
    // now we wait for all deposit to be saved in LeafStorageCache
    await webbRelayer.waitForEvent({
      kind: 'leaves_store',
      event: {
        leaf_index: (noOfDeposit - 1).toString(),
      },
    });
    // chainId
    let chainId = await aliceNode.getChainId();
    const chainIdHex = chainId.toString(16);
    const treeIds = await api.query.anchorBn254.anchors.keys();
    const sorted = treeIds.map((id) => Number(id.toHuman())).sort();
    const treeId = sorted[0] || 5;

    // now we call relayer leaf API to check no of leaves stored in LeafStorageCache
    // are equal to no of deposits made.
    const response = await webbRelayer.getLeavesSubstrate(
      chainIdHex,
      treeId.toString()
    );
    expect(response.status).equal(200);
    let leavesStore = response.json() as Promise<LeavesCacheResponse>;
    leavesStore.then((resp) => {
      expect(noOfDeposit).to.equal(resp.leaves.length);
    });
  });

  it('Simple Anchor Transaction', async () => {
    const api = await aliceNode.api();
    const account = createAccount('//Dave');
    const note = await makeDeposit(api, aliceNode, account);
    const withdrawalProof = await initWithdrawal(
      api,
      webbRelayer,
      account,
      note
    );

    // get the initial balance
    let { data: balance } = await api.query.system.account(
      withdrawalProof.recipient
    );
    let initialBalance = balance.free.toBigInt();
    const roots = [
      Array.from(withdrawalProof.treeRoot),
      Array.from(withdrawalProof.neighborRoot),
    ];
    // get chainId
    let chainId = await aliceNode.getChainId();
    // now we need to submit the withdrawal transaction.
    const txHash = await webbRelayer.substrateAnchorWithdraw({
      chainId: chainId,
      id: withdrawalProof.id,
      proof: Array.from(hexToU8a(withdrawalProof.proofBytes)),
      roots: roots,
      nullifierHash: Array.from(hexToU8a(withdrawalProof.nullifierHash)),
      refund: withdrawalProof.refund,
      fee: withdrawalProof.fee,
      recipient: withdrawalProof.recipient,
      relayer: withdrawalProof.relayer,
      refreshCommitment: Array.from(
        hexToU8a(withdrawalProof.refreshCommitment)
      ),
      extDataHash: Array.from(
        hexToU8a(
          '0x0000000000000000000000000000000000000000000000000000000000000000'
        )
      ),
    });

    expect(txHash).to.be.not.null;

    // get the balance after withdrawal is done and see if it increases
    const { data: balanceAfter } = await api.query.system.account(
      withdrawalProof.recipient
    );
    let balanceAfterWithdraw = balanceAfter.free.toBigInt();
    expect(balanceAfterWithdraw > initialBalance);
  });

  it('Should fail to withdraw if recipient address is invalid', async () => {
    const api = await aliceNode.api();
    const account = createAccount('//Dave');
    const note = await makeDeposit(api, aliceNode, account);
    const withdrawalProof = await initWithdrawal(
      api,
      webbRelayer,
      account,
      note
    );

    const roots = [
      Array.from(withdrawalProof.treeRoot),
      Array.from(withdrawalProof.neighborRoot),
    ];

    const invalidAddress = '5DAAnrj7VHTznn2AWBemMuyBwZWs6FNFjdyVXUeYum3PTXFy';
    // get chainId
    let chainId = await aliceNode.getChainId();
    // now we need to submit the withdrawal transaction.
    try {
      // try to withdraw with invalid address
      await webbRelayer.substrateAnchorWithdraw({
        chainId: chainId,
        id: withdrawalProof.id,
        proof: Array.from(hexToU8a(withdrawalProof.proofBytes)),
        roots: roots,
        nullifierHash: Array.from(hexToU8a(withdrawalProof.nullifierHash)),
        refund: withdrawalProof.refund,
        fee: withdrawalProof.fee,
        recipient: invalidAddress,
        relayer: withdrawalProof.relayer,
        refreshCommitment: Array.from(
          hexToU8a(withdrawalProof.refreshCommitment)
        ),
        extDataHash: Array.from(
          hexToU8a(
            '0x0000000000000000000000000000000000000000000000000000000000000000'
          )
        ),
      });
    } catch (e) {
      console.log(`error is ${e}`);

      // Expect an error to be thrown
      expect(e).to.not.be.null;
    }
  });

  it('Should fail to withdraw if proof is invalid', async () => {
    const api = await aliceNode.api();
    const account = createAccount('//Eve');
    const note = await makeDeposit(api, aliceNode, account);
    const withdrawalProof = await initWithdrawal(
      api,
      webbRelayer,
      account,
      note
    );

    const proofBytes = hexToU8a(withdrawalProof.proofBytes);
    proofBytes[1] = 0x42;
    const invalidProofBytes = u8aToHex(proofBytes);
    expect(withdrawalProof.proofBytes).to.not.eq(invalidProofBytes);

    const roots = [
      Array.from(withdrawalProof.treeRoot),
      Array.from(withdrawalProof.neighborRoot),
    ];
    // get chainId
    let chainId = await aliceNode.getChainId();
    // now we need to submit the withdrawal transaction.
    try {
      // try to withdraw with invalid proof
      await webbRelayer.substrateAnchorWithdraw({
        chainId: chainId,
        id: withdrawalProof.id,
        proof: Array.from(hexToU8a(invalidProofBytes)),
        roots: roots,
        nullifierHash: Array.from(hexToU8a(withdrawalProof.nullifierHash)),
        refund: withdrawalProof.refund,
        fee: withdrawalProof.fee,
        recipient: withdrawalProof.recipient,
        relayer: withdrawalProof.relayer,
        refreshCommitment: Array.from(
          hexToU8a(withdrawalProof.refreshCommitment)
        ),
        extDataHash: Array.from(
          hexToU8a(
            '0x0000000000000000000000000000000000000000000000000000000000000000'
          )
        ),
      });
    } catch (e) {
      console.log(`error is ${e}`);

      // Expect an error to be thrown
      expect(e).to.not.be.null;
      expect(e).to.match(/InvalidWithdrawProof|VerifyError/gim);
    }
  });

  it('Should fail to withdraw if fee is not expected', async () => {
    const api = await aliceNode.api();
    const account = createAccount('//Ferdie');
    const note = await makeDeposit(api, aliceNode, account);
    const withdrawalProof = await initWithdrawal(
      api,
      webbRelayer,
      account,
      note
    );

    const invalidFee = 100;

    const roots = [
      Array.from(withdrawalProof.treeRoot),
      Array.from(withdrawalProof.neighborRoot),
    ];
    // get chainId
    let chainId = await aliceNode.getChainId();
    // now we need to submit the withdrawal transaction.
    try {
      // try to withdraw with invalid address
      await webbRelayer.substrateAnchorWithdraw({
        chainId: chainId,
        id: withdrawalProof.id,
        proof: Array.from(hexToU8a(withdrawalProof.proofBytes)),
        roots: roots,
        nullifierHash: Array.from(hexToU8a(withdrawalProof.nullifierHash)),
        refund: withdrawalProof.refund,
        fee: invalidFee,
        recipient: withdrawalProof.recipient,
        relayer: withdrawalProof.relayer,
        refreshCommitment: Array.from(
          hexToU8a(withdrawalProof.refreshCommitment)
        ),
        extDataHash: Array.from(
          hexToU8a(
            '0x0000000000000000000000000000000000000000000000000000000000000000'
          )
        ),
      });
    } catch (e) {
      console.log(`error is ${e}`);

      // Expect an error to be thrown
      expect(e).to.not.be.null;
      // Runtime Error that indicates invalid withdrawal proof
      expect(e).to.match(/InvalidWithdrawProof|VerifyError/gim);
    }
  });

  it('Should fail to withdraw if neighbor root is invalid', async () => {
    const api = await aliceNode.api();
    const account = createAccount('//Eve');
    const note = await makeDeposit(api, aliceNode, account);
    const withdrawalProof = await initWithdrawal(
      api,
      webbRelayer,
      account,
      note
    );

    const invalidRoots = [
      Array.from(withdrawalProof.treeRoot),
      Array.from(
        hexToU8a(
          '0x27f427ccbf58a44b1270abbe4eda6ba53bd6ac4d88cf1e00a13c4371ce71d366'
        )
      ),
    ];
    // get chainId
    let chainId = await aliceNode.getChainId();
    // now we need to submit the withdrawal transaction.
    try {
      // try to withdraw with invalid roots
      await webbRelayer.substrateAnchorWithdraw({
        chainId: chainId,
        id: withdrawalProof.id,
        proof: Array.from(hexToU8a(withdrawalProof.proofBytes)),
        roots: invalidRoots,
        nullifierHash: Array.from(hexToU8a(withdrawalProof.nullifierHash)),
        refund: withdrawalProof.refund,
        fee: withdrawalProof.fee,
        recipient: withdrawalProof.recipient,
        relayer: withdrawalProof.relayer,
        refreshCommitment: Array.from(
          hexToU8a(withdrawalProof.refreshCommitment)
        ),
        extDataHash: Array.from(
          hexToU8a(
            '0x0000000000000000000000000000000000000000000000000000000000000000'
          )
        ),
      });
    } catch (e) {
      console.log(`error is ${e}`);

      // Expect an error to be thrown
      expect(e).to.not.be.null;
      // Runtime Error that indicates invalid neighbor roots
      expect(e).to.match(/UnknownRoot|InvalidNeighborWithdrawRoot/gim);
    }
  });

  it('Should fail to withdraw if tree root is invalid', async () => {
    const api = await aliceNode.api();
    const account = createAccount('//Eve');
    const note = await makeDeposit(api, aliceNode, account);
    const withdrawalProof = await initWithdrawal(
      api,
      webbRelayer,
      account,
      note
    );

    const invalidRoots = [
      Array.from(
        hexToU8a(
          '0x27f427ccbf58a44b1270abbe4eda6ba53bd6ac4d88cf1e00a13c4371ce71d366'
        )
      ),
      Array.from(withdrawalProof.neighborRoot),
    ];
    // get chainId
    let chainId = await aliceNode.getChainId();
    // now we need to submit the withdrawal transaction.
    try {
      // try to withdraw with invalid roots
      await webbRelayer.substrateAnchorWithdraw({
        chainId: chainId,
        id: withdrawalProof.id,
        proof: Array.from(hexToU8a(withdrawalProof.proofBytes)),
        roots: invalidRoots,
        nullifierHash: Array.from(hexToU8a(withdrawalProof.nullifierHash)),
        refund: withdrawalProof.refund,
        fee: withdrawalProof.fee,
        recipient: withdrawalProof.recipient,
        relayer: withdrawalProof.relayer,
        refreshCommitment: Array.from(
          hexToU8a(withdrawalProof.refreshCommitment)
        ),
        extDataHash: Array.from(
          hexToU8a(
            '0x0000000000000000000000000000000000000000000000000000000000000000'
          )
        ),
      });
    } catch (e) {
      console.log(`error is ${e}`);

      // Expect an error to be thrown
      expect(e).to.not.be.null;
      // Runtime Error that indicates Unknown Root
      expect(e).to.match(/UnknownRoot/gim);
    }
  });

  it('Should fail to withdraw if relayer address is invalid', async () => {
    const api = await aliceNode.api();
    const account = createAccount('//Dave');
    const note = await makeDeposit(api, aliceNode, account);
    const withdrawalProof = await initWithdrawal(
      api,
      webbRelayer,
      account,
      note
    );

    const roots = [
      Array.from(withdrawalProof.treeRoot),
      Array.from(withdrawalProof.neighborRoot),
    ];

    const invalidAddress = '5DAAnrj7VHTznn2AWBemMuyBwZWs6FNFjdyVXUeYum3PTXFy';
    // get chainId
    let chainId = await aliceNode.getChainId();
    // now we need to submit the withdrawal transaction.
    try {
      // try to withdraw with invalid address
      await webbRelayer.substrateAnchorWithdraw({
        chainId: chainId,
        id: withdrawalProof.id,
        proof: Array.from(hexToU8a(withdrawalProof.proofBytes)),
        roots: roots,
        nullifierHash: Array.from(hexToU8a(withdrawalProof.nullifierHash)),
        refund: withdrawalProof.refund,
        fee: withdrawalProof.fee,
        recipient: withdrawalProof.recipient,
        relayer: invalidAddress,
        refreshCommitment: Array.from(
          hexToU8a(withdrawalProof.refreshCommitment)
        ),
        extDataHash: Array.from(
          hexToU8a(
            '0x0000000000000000000000000000000000000000000000000000000000000000'
          )
        ),
      });
    } catch (e) {
      console.log(`error is ${e}`);

      // Expect an error to be thrown
      expect(e).to.not.be.null;
      // Runtime Error that indicates invalid withdrawal proof
      expect(e).to.contain(
        'Runtime error: RuntimeError(Module { index: 41, error: 2 }'
      );
    }
  });

  it('Should fail to withdraw with invalid nullifier hash', async () => {
    const api = await aliceNode.api();
    const account = createAccount('//Ferdie');
    const note = await makeDeposit(api, aliceNode, account);
    const withdrawalProof = await initWithdrawal(
      api,
      webbRelayer,
      account,
      note
    );

    const roots = [
      Array.from(withdrawalProof.treeRoot),
      Array.from(withdrawalProof.neighborRoot),
    ];

    const nullifierHash = hexToU8a(withdrawalProof.nullifierHash);
    const flipCount = nullifierHash.length / 8;
    for (let i = 0; i < flipCount; i++) {
      nullifierHash[i] = 0x42;
    }
    const invalidNullifierHash = u8aToHex(nullifierHash);
    expect(withdrawalProof.nullifierHash).to.not.eq(invalidNullifierHash);
    // get chainId
    let chainId = await aliceNode.getChainId();
    // now we need to submit the withdrawal transaction.
    try {
      // try to withdraw with invalid address
      await webbRelayer.substrateAnchorWithdraw({
        chainId: chainId,
        id: withdrawalProof.id,
        proof: Array.from(hexToU8a(withdrawalProof.proofBytes)),
        roots: roots,
        nullifierHash: Array.from(hexToU8a(invalidNullifierHash)),
        refund: withdrawalProof.refund,
        fee: withdrawalProof.fee,
        recipient: withdrawalProof.recipient,
        relayer: withdrawalProof.relayer,
        refreshCommitment: Array.from(
          hexToU8a(withdrawalProof.refreshCommitment)
        ),
        extDataHash: Array.from(
          hexToU8a(
            '0x0000000000000000000000000000000000000000000000000000000000000000'
          )
        ),
      });
    } catch (e) {
      console.log(`error is ${e}`);

      // Expect an error to be thrown
      expect(e).to.not.be.null;
      // Runtime Error that indicates invalid withdrawal proof
      expect(e).to.match(/InvalidWithdrawProof/gim);
    }
  });

  after(async () => {
    await aliceNode?.stop();
    await bobNode?.stop();
    await webbRelayer?.stop();
  });
});

// Helper methods, we can move them somewhere if we end up using them again.

async function createAnchorDepositTx(api: ApiPromise): Promise<{
  tx: SubmittableExtrinsic<'promise'>;
  note: Note;
}> {
  const treeIds = await api.query.anchorBn254.anchors.keys();
  const sorted = treeIds.map((id) => Number(id.toHuman())).sort();
  const treeId = sorted[0] || 5;

  const noteInput: NoteGenInput = {
    protocol: 'anchor',
    version: 'v2',
    sourceChain: '2199023256632',
    targetChain: '2199023256632',
    sourceIdentifyingData: treeId.toString(),
    targetIdentifyingData: treeId.toString(),
    tokenSymbol: 'WEBB',
    amount: '1',
    denomination: '18',
    backend: 'Arkworks',
    hashFunction: 'Poseidon',
    curve: 'Bn254',
    width: '4',
    exponentiation: '5',
  };
  const note = await Note.generateNote(noteInput);
  const leaf = note.getLeaf();
  const tx = api.tx.anchorBn254.deposit(treeId, leaf);
  return { tx, note };
}

type WithdrawalOpts = {
  relayer: string;
  recipient: string;
  fee?: number;
  refund?: number;
};

type WithdrawalProof = {
  id: number;
  proofBytes: string;
  nullifierHash: string;
  recipient: string;
  relayer: string;
  fee: number;
  refund: number;
  refreshCommitment: string;
  treeRoot: Uint8Array;
  neighborRoot: Uint8Array;
};

async function createAnchorWithdrawProof(
  api: ApiPromise,
  note: Note,
  opts: WithdrawalOpts
): Promise<WithdrawalProof> {
  try {
    const recipientAddressHex = u8aToHex(decodeAddress(opts.recipient)).replace(
      '0x',
      ''
    );
    const relayerAddressHex = u8aToHex(decodeAddress(opts.relayer)).replace(
      '0x',
      ''
    );
    const treeIds = await api.query.anchorBn254.anchors.keys();
    const sorted = treeIds.map((id) => Number(id.toHuman())).sort();
    const treeId = sorted[0] || 5;
    const leafCount: number =
      await api.derive.merkleTreeBn254.getLeafCountForTree(treeId);
    const treeLeaves: Uint8Array[] =
      await api.derive.merkleTreeBn254.getLeavesForTree(
        treeId,
        0,
        leafCount - 1
      );

    //@ts-ignore
    const getNeighborRoots = api.rpc.lt.getNeighborRoots;
    let neighborRoots = await getNeighborRoots(treeId);

    let neighborRootsU8: Uint8Array[] = new Array(neighborRoots.length);
    for (let i = 0; i < neighborRootsU8.length; i++) {
      neighborRootsU8[i] = hexToU8a(neighborRoots[0].toString());
    }

    // Get tree root on chain
    const treeRoot = await api.query.merkleTreeBn254.trees(treeId);

    const provingManager = new ArkworksProvingManager(null);
    const leafHex = u8aToHex(note.getLeaf());

    const leafIndex = treeLeaves.findIndex((l) => u8aToHex(l) === leafHex);
    expect(leafIndex).to.be.greaterThan(-1);
    const gitRoot = child
      .execSync('git rev-parse --show-toplevel')
      .toString()
      .trim();

    // make a root set from the tree root
    const rootValue = treeRoot.toHuman() as { root: string };

    const treeRootArray = [hexToU8a(rootValue.root), ...neighborRootsU8];

    const provingKeyPath = path.join(
      gitRoot,
      'tests',
      'protocol-substrate-fixtures',
      'fixed-anchor',
      'bn254',
      'x5',
      '2',
      'proving_key_uncompressed.bin'
    );
    const provingKey = fs.readFileSync(provingKeyPath);

    const proofInput: ProvingManagerSetupInput<'anchor'> = {
      note: note.serialize(),
      relayer: relayerAddressHex,
      recipient: recipientAddressHex,
      leaves: treeLeaves,
      leafIndex,
      fee: opts.fee === undefined ? 0 : opts.fee,
      refund: opts.refund === undefined ? 0 : opts.refund,
      provingKey,
      roots: treeRootArray,
      refreshCommitment:
        '0000000000000000000000000000000000000000000000000000000000000000',
    };

    const zkProof = await provingManager.prove('anchor', proofInput);
    return {
      id: treeId,
      proofBytes: `0x${zkProof.proof}`,
      nullifierHash: `0x${zkProof.nullifierHash}`,
      recipient: opts.recipient,
      relayer: opts.relayer,
      fee: opts.fee === undefined ? 0 : opts.fee,
      refund: opts.refund === undefined ? 0 : opts.refund,
      refreshCommitment:
        '0x0000000000000000000000000000000000000000000000000000000000000000',
      treeRoot: hexToU8a(rootValue.root),
      neighborRoot: neighborRoots[0]!,
    };
  } catch (error) {
    //@ts-ignore
    console.error(error.error_message);
    //@ts-ignore
    console.error(error.code);
    throw error;
  }
}

function createAccount(accountId: string): any {
  const keyring = new Keyring({ type: 'sr25519' });
  const account = keyring.addFromUri(accountId);

  return account;
}

async function makeDeposit(
  api: any,
  aliceNode: any,
  account: any
): Promise<Note> {
  const { tx, note } = await createAnchorDepositTx(api);

  // send the deposit transaction.
  const txSigned = await tx.signAsync(account);

  await aliceNode.executeTransaction(txSigned);

  return note;
}

async function initWithdrawal(
  api: any,
  webbRelayer: any,
  account: any,
  note: Note
): Promise<WithdrawalProof> {
  // next we need to prepare the withdrawal transaction.
  // create correct proof with right address
  const withdrawalProof = await createAnchorWithdrawProof(api, note, {
    recipient: account.address,
    relayer: account.address,
  });
  // ping the relayer!
  await webbRelayer.ping();

  return withdrawalProof;
}
