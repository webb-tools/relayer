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
// This our basic Substrate VAnchor Transaction Relayer Tests.
// These are for testing the basic relayer functionality. which is just to relay transactions for us.

import '@webb-tools/protocol-substrate-types';
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
import { LocalTangle } from '../../lib/localTangle.js';

import { u8aToHex, hexToU8a } from '@polkadot/util';
import { decodeAddress } from '@polkadot/util-crypto';
import { naclEncrypt, randomAsU8a } from '@polkadot/util-crypto';

import {
  ProvingManagerSetupInput,
  ArkworksProvingManager,
  Utxo,
  VAnchorProof,
  LeafIdentifier,
  calculateTypedChainId,
  ChainType,
} from '@webb-tools/sdk-core';
import { currencyToUnitI128, UsageMode } from '@webb-tools/test-utils';
import {
  createAccount,
  defaultEventsWatcherValue,
  generateVAnchorNote,
} from '../../lib/utils.js';
import { ethers } from 'ethers';

describe('Substrate VAnchor Transaction Relayer Tests', function () {
  const tmpDirPath = temp.mkdirSync();
  let aliceNode: LocalTangle;
  let bobNode: LocalTangle;

  let webbRelayer: WebbRelayer;
  const PK1 = u8aToHex(ethers.utils.randomBytes(32));

  before(async () => {
    const usageMode: UsageMode = isCi
      ? { mode: 'docker', forcePullImage: false }
      : {
          mode: 'host',
          nodePath: path.resolve(
            '../../tangle/target/release/tangle-standalone'
          ),
        };
    const enabledPallets: Pallet[] = [
      {
        pallet: 'VAnchorBn254',
        eventsWatcher: defaultEventsWatcherValue,
      },
    ];

    aliceNode = await LocalTangle.start({
      name: 'substrate-alice',
      authority: 'alice',
      usageMode,
      ports: 'auto',
      enableLogging: false,
    });

    bobNode = await LocalTangle.start({
      name: 'substrate-bob',
      authority: 'bob',
      usageMode,
      ports: 'auto',
      enableLogging: false,
    });
    // Wait until we are ready and connected
    const api = await aliceNode.api();
    await api.isReady;

    const chainId = await aliceNode.getChainId();

    await aliceNode.writeConfig(`${tmpDirPath}/${aliceNode.name}.json`, {
      suri: '//Charlie',
      chainId: chainId,
      proposalSigningBackend: { type: 'Mocked', privateKey: PK1 },
      enabledPallets,
    });

    // now start the relayer
    const relayerPort = await getPort({ port: portNumbers(8000, 8888) });
    webbRelayer = new WebbRelayer({
      commonConfig: {
        port: relayerPort,
      },
      tmp: true,
      configDir: tmpDirPath,
      showLogs: true,
    });
    await webbRelayer.waitUntilReady();
  });

  it('number of deposits made should be equal to number of leaves in cache', async () => {
    const api = await aliceNode.api();
    const account = createAccount('//Dave');
    //create vanchor
    const createVAnchorCall = api.tx.vAnchorBn254.create(1, 30, 0);
    // execute sudo transaction.
    await aliceNode.sudoExecuteTransaction(createVAnchorCall);

    const nextTreeId = await api.query.merkleTreeBn254.nextTreeId();
    const treeId = nextTreeId.toNumber() - 1;

    // ChainId of the substrate chain
    const chainId = await aliceNode.getChainId();
    const typedSourceChainId = calculateTypedChainId(
      ChainType.Substrate,
      chainId
    );
    const outputChainId = BigInt(typedSourceChainId);
    const secret = randomAsU8a();
    const gitRoot = child
      .execSync('git rev-parse --show-toplevel')
      .toString()
      .trim();

    const pkPath = path.join(
      // tests path
      gitRoot,
      'tests',
      'substrate-fixtures',
      'vanchor',
      'bn254',
      'x5',
      '2-2-2',
      'proving_key_uncompressed.bin'
    );
    const pk_hex = fs.readFileSync(pkPath).toString('hex');
    const pk = hexToU8a(pk_hex);

    // Creating two empty vanchor notes
    const note1 = await generateVAnchorNote(
      0,
      Number(outputChainId.toString()),
      Number(outputChainId.toString()),
      0
    );
    const note2 = await note1.getDefaultUtxoNote();
    const publicAmount = currencyToUnitI128(1000);
    const notes = [note1, note2];
    // Output UTXOs configs
    const output1 = await Utxo.generateUtxo({
      curve: 'Bn254',
      backend: 'Arkworks',
      amount: publicAmount.toString(),
      chainId: chainId.toString(),
    });
    const output2 = await Utxo.generateUtxo({
      curve: 'Bn254',
      backend: 'Arkworks',
      amount: '0',
      chainId: chainId.toString(),
    });

    // Configure a new proving manager with direct call
    const provingManager = new ArkworksProvingManager(null);
    const leavesMap = {};

    const address = account.address;
    const extAmount = currencyToUnitI128(1000);
    const fee = 0;
    const refund = 0;
    // Empty leaves
    leavesMap[outputChainId.toString()] = [];
    const tree = await api.query.merkleTreeBn254.trees(treeId);
    const root = tree.unwrap().root.toHex();
    const rootsSet = [hexToU8a(root), hexToU8a(root)];
    const decodedAddress = decodeAddress(address);
    const { encrypted: comEnc1 } = naclEncrypt(output1.commitment, secret);
    const { encrypted: comEnc2 } = naclEncrypt(output2.commitment, secret);
    const assetId = new Uint8Array([0, 0, 0, 0]); // WEBB native token asset Id.
    const dummyLeafId: LeafIdentifier = {
      index: 0,
      typedChainId: Number(outputChainId.toString()),
    };

    const setup: ProvingManagerSetupInput<'vanchor'> = {
      chainId: outputChainId.toString(),
      inputUtxos: notes.map((n) => new Utxo(n.note.getUtxo())),
      leafIds: [dummyLeafId, dummyLeafId],
      leavesMap: leavesMap,
      output: [output1, output2],
      encryptedCommitments: [comEnc1, comEnc2],
      provingKey: pk,
      publicAmount: String(publicAmount),
      roots: rootsSet,
      relayer: decodedAddress,
      recipient: decodedAddress,
      extAmount: extAmount.toString(),
      fee: fee.toString(),
      refund: String(refund),
      token: assetId,
    };

    const data = (await provingManager.prove('vanchor', setup)) as VAnchorProof;
    const extData = {
      relayer: address,
      recipient: address,
      fee,
      refund: String(refund),
      token: assetId,
      extAmount: extAmount,
      encryptedOutput1: u8aToHex(comEnc1),
      encryptedOutput2: u8aToHex(comEnc2),
    };

    const vanchorProofData = {
      proof: `0x${data.proof}`,
      publicAmount: data.publicAmount,
      roots: rootsSet,
      inputNullifiers: data.inputUtxos.map((input) => `0x${input.nullifier}`),
      outputCommitments: data.outputUtxos.map((utxo) => utxo.commitment),
      extDataHash: data.extDataHash,
    };
    // eslint-disable-next-line @typescript-eslint/ban-ts-comment
    // @ts-ignore
    const leafsCount = await api.derive.merkleTreeBn254.getLeafCountForTree(
      Number(treeId)
    );
    const indexBeforeInsetion = Math.max(leafsCount - 1, 0);

    // now we call the vanchor transact
    const transactCall = api.tx.vAnchorBn254.transact(
      treeId,
      vanchorProofData,
      extData
    );
    const txSigned = await transactCall.signAsync(account);
    await aliceNode.executeTransaction(txSigned);

    // now we wait for all deposit to be saved in LeafStorageCache.
    await webbRelayer.waitForEvent({
      kind: 'leaves_store',
      event: {
        leaf_index: indexBeforeInsetion + 2,
      },
    });

    // chainId
    const chainIdentifier = await aliceNode.getChainId();
    // now we call relayer leaf API to check no of leaves stored in LeafStorageCache
    // are equal to no of deposits made.
    const response = await webbRelayer.getLeavesSubstrate(
      chainIdentifier.toString(),
      treeId.toString(),
      '44' // pallet Id
    );
    expect(response.status).equal(200);
    const leavesStore = response.json() as Promise<LeavesCacheResponse>;

    leavesStore.then((resp) => {
      expect(indexBeforeInsetion + 2).to.equal(resp.leaves.length);
    });
  });

  after(async () => {
    await aliceNode?.stop();
    await bobNode?.stop();
    await webbRelayer?.stop();
  });
});
