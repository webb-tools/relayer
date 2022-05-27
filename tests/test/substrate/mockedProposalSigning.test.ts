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
// These are for testing mocked proposal signing backend for substrate

import '@webb-tools/types';
import getPort, { portNumbers } from 'get-port';
import temp from 'temp';
import path from 'path';
import isCi from 'is-ci';
import { WebbRelayer, Pallet } from '../../lib/webbRelayer.js';
import { LocalProtocolSubstrate } from '../../lib/localProtocolSubstrate.js';
import {
  UsageMode,
  defaultEventsWatcherValue,
} from '../../lib/substrateNodeBase.js';
import { ApiPromise, Keyring } from '@polkadot/api';
import { SubmittableExtrinsic } from '@polkadot/api/types';
import {
  Note,
  NoteGenInput,
} from '@webb-tools/sdk-core';

describe('Substrate Signing Backend', function () {
  const tmpDirPath = temp.mkdirSync();
  let aliceNode: LocalProtocolSubstrate;
  let bobNode: LocalProtocolSubstrate;

  // Governer key
  const PK1 = "0x9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
  const uncompressedKey = "0xed7000a10ba086a64a5af64555d09c7d375c4f29e575fa3da290ac2e6e7a227ae649b1a057dc0e85e290d0d061732d7f64d533a2ffe8f712069a5664e00efe53";


  let webbRelayer: WebbRelayer;

  before(async () => {
    const usageMode: UsageMode = isCi
      ? { mode: 'docker', forcePullImage: false }
      : {
          mode: 'host',
          nodePath: path.resolve(
            '../../../protocol-substrate/target/release/webb-standalone-node'
          ),
        };
    
    // enable pallets
    const enabledPallets: Pallet[] = [
      {
        pallet: 'AnchorBn254',
        eventsWatcher: defaultEventsWatcherValue,
      },
      {
        pallet: 'SignatureBridge',
        eventsWatcher: defaultEventsWatcherValue,
      },
    ];

    aliceNode = await LocalProtocolSubstrate.start({
      name: 'substrate-alice',
      authority: 'alice',
      usageMode,
      ports: 'auto',
      enabledPallets,
      enableLogging: true
    });

    bobNode = await LocalProtocolSubstrate.start({
      name: 'substrate-bob',
      authority: 'bob',
      usageMode,
      ports: 'auto',
    });

    // set proposal signing backend and linked anchors
    await aliceNode.writeConfig(`${tmpDirPath}/${aliceNode.name}.json`,
      {
          suri: '//Charlie',
          proposalSigningBackend: { type: 'Mocked', privateKey: PK1},
          linkedAnchors: [
              {
                  chain: 1080,
                  tree: 5
              }
          ]
    });


    // Wait until we are ready and connected
    const api = await aliceNode.api();
    await api.isReady;


    //force set maintainer
    let setMaintainerCall = api.tx.signatureBridge!.forceSetMaintainer!(uncompressedKey);
    // send the deposit transaction.
    await aliceNode.sudoExecuteTransaction(setMaintainerCall);

    // now start the relayer
    const relayerPort = await getPort({ port: portNumbers(8000, 8888) });
    webbRelayer = new WebbRelayer({
      port: relayerPort,
      tmp: true,
      configDir: tmpDirPath,
      showLogs: true,
    });
    await webbRelayer.waitUntilReady();
  });

  it('Simple Anchor Deposit', async () => {
    const api = await aliceNode.api();
    const account = createAccount('//Dave');
    const note = await makeDeposit(api, aliceNode, account);

    // now we wait for the proposal to be signed by mocked backend and then send data to signature bridge
    await webbRelayer.waitForEvent({
      kind: 'signing_backend',
      event: {
        backend: 'Mocked'
      }
    });

    // now we wait for the proposals to verified and executed by signature bridge

    await webbRelayer.waitForEvent({
      kind: 'signature_bridge',
      event: {
        call: 'execute_proposal_with_signature'
      }
    });

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
  const noteInput: NoteGenInput = {
    protocol: 'anchor',
    version: 'v2',
    sourceChain: '2199023256632',
    targetChain: '2199023256632',
    sourceIdentifyingData: `5`,
    targetIdentifyingData: `5`,
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
  // @ts-ignore
  const treeIds = await api.query.anchorBn254.anchors.keys();
  const sorted = treeIds.map((id) => Number(id.toHuman())).sort();
  const treeId = sorted[0] || 5;
  const leaf = note.getLeaf();
  // @ts-ignore
  const tx = api.tx.anchorBn254.deposit(treeId, leaf);
  return { tx, note };
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


