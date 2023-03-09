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
// This our basic EVM Vanchor Transaction Relayer Tests.
// These are for testing the basic relayer functionality. which is just relay transactions for us.

import { expect } from 'chai';
import { Tokens, VBridge } from '@webb-tools/protocol-solidity';
import { CircomUtxo, Keypair, parseTypedChainId, toFixedHex, Utxo } from '@webb-tools/sdk-core';

import { BigNumber, ethers } from 'ethers';
import temp from 'temp';
import { LocalChain } from '../../lib/localTestnet.js';
import {
  defaultWithdrawConfigValue,
  EnabledContracts,
  FeeInfo,
  WebbRelayer,
} from '../../lib/webbRelayer.js';
import getPort, { portNumbers } from 'get-port';
import { u8aToHex, hexToU8a } from '@polkadot/util';
import { MintableToken } from '@webb-tools/tokens';
import { formatEther, parseEther } from 'ethers/lib/utils.js';

// This test is meant to prove that utxo transfer flows are possible, and the receiver
// can query on-chain data to construct and spend a utxo generated by the sender.
describe('Relayer transfer assets', function () {
  const tmpDirPath = temp.mkdirSync();
  let localChain1: LocalChain;
  let localChain2: LocalChain;
  let signatureVBridge: VBridge.VBridge;
  let govWallet1: ethers.Wallet;
  let govWallet2: ethers.Wallet;
  let relayerWallet1: ethers.Wallet;

  let webbRelayer: WebbRelayer;

  before(async () => {
    const govPk = u8aToHex(ethers.utils.randomBytes(32));
    const relayerPk = u8aToHex(ethers.utils.randomBytes(32));

    // first we need to start local evm node.
    const localChain1Port = await getPort({
      port: portNumbers(3333, 4444),
    });

    const enabledContracts: EnabledContracts[] = [
      {
        contract: 'VAnchor',
      },
    ];
    parseTypedChainId;
    localChain1 = await LocalChain.init({
      port: localChain1Port,
      chainId: localChain1Port,
      name: 'Hermes',
      populatedAccounts: [
        {
          secretKey: govPk,
          balance: ethers.utils
            .parseEther('100000000000000000000000')
            .toHexString(),
        },
        {
          secretKey: relayerPk,
          balance: ethers.utils
            .parseEther('100000000000000000000000')
            .toHexString(),
        },
      ],
      enabledContracts: enabledContracts,
    });

    const localChain2Port = await getPort({
      port: portNumbers(3333, 4444),
    });
    localChain2 = await LocalChain.init({
      port: localChain2Port,
      chainId: localChain2Port,
      name: 'Athena',
      populatedAccounts: [
        {
          secretKey: govPk,
          balance: ethers.utils
            .parseEther('100000000000000000000000')
            .toHexString(),
        },
        {
          secretKey: relayerPk,
          balance: ethers.utils
            .parseEther('100000000000000000000000')
            .toHexString(),
        },
      ],
      enabledContracts: enabledContracts,
    });

    govWallet1 = new ethers.Wallet(govPk, localChain1.provider());
    govWallet2 = new ethers.Wallet(govPk, localChain2.provider());

    relayerWallet1 = new ethers.Wallet(relayerPk, localChain1.provider());
    
    // Deploy the token.
    const wrappedToken1 = await localChain1.deployToken(
      'Wrapped Ethereum',
      'WETH'
    );
    const wrappedToken2 = await localChain2.deployToken(
      'Wrapped Ethereum',
      'WETH'
    );
    const unwrappedToken1 = await MintableToken.createToken(
      'Webb Token',
      'WEBB',
      govWallet1
    );
    const unwrappedToken2 = await MintableToken.createToken(
      'Webb Token',
      'WEBB',
      govWallet2
    );

    signatureVBridge = await localChain1.deploySignatureVBridge(
      localChain2,
      wrappedToken1,
      wrappedToken2,
      govWallet1,
      govWallet2,
      unwrappedToken1,
      unwrappedToken2
    );

    // save the chain configs.
    await localChain1.writeConfig(`${tmpDirPath}/${localChain1.name}.json`, {
      signatureVBridge,
      withdrawConfig: defaultWithdrawConfigValue,
      relayerWallet: relayerWallet1,
    });
    
    // get the vanhor on localchain1
    const vanchor1 = signatureVBridge.getVAnchor(localChain1.chainId);
    await vanchor1.setSigner(govWallet1);
    // get token
    const tokenAddress = signatureVBridge.getWebbTokenAddress(
      localChain1.chainId
    )!;
    const token = await Tokens.MintableToken.tokenFromAddress(
      tokenAddress,
      govWallet1
    );

    // aprove token spending for vanchor
    const tx = await token.approveSpending(
      vanchor1.contract.address,
      ethers.utils.parseEther('1000')
    );
    await tx.wait();

    // mint tokens on wallet
    await token.mintTokens(govWallet1.address, ethers.utils.parseEther('1000'));

    // Set governor
    const governorAddress = govWallet1.address;
    const currentGovernor = await signatureVBridge
      .getVBridgeSide(localChain1.chainId)
      .contract.governor();
    expect(currentGovernor).to.eq(governorAddress);

    // now start the relayer
    const relayerPort = await getPort({ port: portNumbers(9955, 9999) });

    webbRelayer = new WebbRelayer({
      commonConfig: {
        features: { dataQuery: false, governanceRelay: false },
        port: relayerPort,
      },
      tmp: true,
      configDir: tmpDirPath,
      showLogs: true,
      verbosity: 3,
    });
    await webbRelayer.waitUntilReady();
  });

  it('should be able to transfer Utxo', async() => {
    const vanchor1 = signatureVBridge.getVAnchor(localChain1.chainId);
    await vanchor1.setSigner(govWallet1);
    const vanchor2 = signatureVBridge.getVAnchor(localChain2.chainId);
    await vanchor2.setSigner(govWallet2);

    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    const tokenAddress = signatureVBridge.getWebbTokenAddress(
      localChain1.chainId
    )!;

    const token = await Tokens.MintableToken.tokenFromAddress(
        tokenAddress,
        govWallet1
      );
    
    // Step 1. Register recipient account(Bob)
    const bobKey = u8aToHex(ethers.utils.randomBytes(32));
    const bobWallet = new ethers.Wallet(bobKey, localChain1.provider());
    const bobKeypair = new Keypair();
    const tx = await vanchor1.contract.register({
        owner: govWallet1.address,
        keyData: bobKeypair.toString(),
      });

    let receipt = await tx.wait();
    // Step 2. Sender queries on chain data for keypair information of recipient
    // In this test, simply take the data from the previous transaction receipt.
    
    // eslint-disable-next-line @typescript-eslint/ban-ts-comment
    //@ts-ignore
    const registeredKeydata: string = receipt.events[0].args.key;
    const bobPublicKeypair = Keypair.fromString(registeredKeydata);
    
    // Step 3. Generate a UTXO that is only spendable by recipient(Bob)
    const transferUtxo = await CircomUtxo.generateUtxo({
      curve: 'Bn254',
      backend: 'Circom',
      amount: ethers.utils.parseEther('10').toString(),
      originChainId: localChain1.chainId.toString(),
      chainId: localChain1.chainId.toString(),
      keypair: bobPublicKeypair,
    });
    
    // insert utxo into tree
    receipt = (await vanchor1.transact(
      [],
      [transferUtxo],
      0,
      0,
      '0',
      relayerWallet1.address,
      tokenAddress,
      {
       
      }
    )) as ethers.ContractReceipt;

    // Bob queries encrypted commitments on chain
    const encryptedCommitments: string[] = receipt.events?.filter((event) => event.event === 'NewCommitment')
    .sort((a, b) => a.args?.index - b.args?.index)
    .map((e) => e.args?.encryptedOutput) ?? [];

    // Attempt to decrypt the encrypted commitments with bob's keypair
    const utxos = await Promise.all(
      encryptedCommitments.map(async (enc, index) => {
        try {
          const decryptedUtxo = await CircomUtxo.decrypt(bobKeypair, enc);
          // In order to properly calculate the nullifier, an index is required.
          decryptedUtxo.setIndex(index);
          decryptedUtxo.setOriginChainId(localChain1.chainId.toString());
          const alreadySpent = await vanchor1.contract.isSpent(
            toFixedHex('0x' + decryptedUtxo.nullifier)
          );
          if (!alreadySpent) {
            return decryptedUtxo;
          } else {
            throw new Error('Passed Utxo detected as alreadySpent');
          }
        } catch (e) {
          return undefined;
        }
      })
    );
    
    const spendableUtxos = utxos.filter((utxo): utxo is Utxo => utxo !== undefined);
    const bobBalanceBefore = await token.getBalance(bobWallet.address);
    console.log("Balance before ", bobBalanceBefore);


    const dummyOutput = await CircomUtxo.generateUtxo({
        curve: 'Bn254',
        backend: 'Circom',
        amount: '0',
        chainId: localChain1.chainId.toString(),
        keypair: bobKeypair,
      });
    
    // fetch the inserted leaves
    const leaves = vanchor1.tree.elements().map((leaf) => hexToU8a(leaf.toHexString()));
    const outputData = await vanchor1.setupTransaction(
        spendableUtxos,
        [dummyOutput, dummyOutput],
        0,
        0,
        bobWallet.address,
        relayerWallet1.address,
        tokenAddress,
        {
            [localChain1.chainId.toString()]: leaves,
        }
      );

    const gas_amount = await vanchor1.contract.estimateGas.transact(
       outputData.publicInputs.proof,
      '0x0000000000000000000000000000000000000000000000000000000000000000',
      outputData.extData,
      outputData.publicInputs,
      outputData.extData
    );

    const feeInfoResponse = await webbRelayer.getFeeInfo(
      localChain1.chainId,
      vanchor1.getAddress(),
      gas_amount
    );
    expect(feeInfoResponse.status).equal(200);
    const feeInfo = await (feeInfoResponse.json() as Promise<FeeInfo>);
    console.log(feeInfo);
    const maxRefund = Number(formatEther(feeInfo.maxRefund));
    const refundExchangeRate = Number(formatEther(feeInfo.refundExchangeRate));
    const refundAmount = BigNumber.from(
      parseEther((maxRefund * refundExchangeRate).toString())
    );
    const totalFee = refundAmount.add(feeInfo.estimatedFee);

    const { extData, publicInputs } = await vanchor1.setupTransaction(
        spendableUtxos,
        [dummyOutput, dummyOutput],
        totalFee,
        refundAmount,
        bobWallet.address,
        relayerWallet1.address,
        tokenAddress,
        {
            [localChain1.chainId.toString()]: leaves,
        }
      );

    await webbRelayer.vanchorWithdraw(
      localChain1.underlyingChainId,
      vanchor1.getAddress(),
      publicInputs,
      extData
    );
    // now we wait for relayer to execute private transaction.
    await webbRelayer.waitForEvent({
      kind: 'private_tx',
      event: {
        ty: 'EVM',
        chain_id: localChain1.underlyingChainId.toString(),
        finalized: true,
      },
    });

    const bobBalanceAfter = await token.getBalance(bobWallet.address);

    console.log("Balance after ", bobBalanceAfter);
    expect(bobBalanceAfter.toBigInt() > bobBalanceBefore.toBigInt()).to
    .be.true;
  });

  after(async () => {
    await localChain1?.stop();
    await localChain2?.stop();
    await webbRelayer?.stop();
  });
});
