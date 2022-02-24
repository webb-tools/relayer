// This our basic EVM Transaction Relayer Tests.
// These are for testing the basic relayer functionality. which is just relay transactions for us.

import { jest } from '@jest/globals';
import { Bridges, Tokens } from '@webb-tools/protocol-solidity';
import { ethers } from 'ethers';
import temp from 'temp';
import { LocalChain } from './lib/localTestnet';
import { WebbRelayer } from './lib/webbRelayer';

describe('EVM Transaction Relayer', () => {
  const tmp = temp.track(true);
  jest.setTimeout(40_000);
  const tmpDirPath = tmp.mkdirSync({ prefix: 'webb-relayer-test-' });
  let localChain1: LocalChain;
  let localChain2: LocalChain;
  let signatureBridge: Bridges.SignatureBridge;

  let wallet1 = new ethers.Wallet(
    '0xc0d375903fd6f6ad3edafc2c5428900c0757ce1da10e5dd864fe387b32b91d7e'
  );
  let wallet2 = new ethers.Wallet(
    '0xc0d375903fd6f6ad3edafc2c5428900c0757ce1da10e5dd864fe387b32b91d7f'
  );

  let webbRelayer: WebbRelayer;

  beforeAll(async () => {
    // first we need to start local evm node.
    localChain1 = new LocalChain('TestA', 3333, [
      {
        secretKey: wallet1.privateKey,
        balance: ethers.utils.parseEther('10').toHexString(),
      },
    ]);

    localChain2 = new LocalChain('TestB', 4444, [
      {
        secretKey: wallet2.privateKey,
        balance: ethers.utils.parseEther('10').toHexString(),
      },
    ]);

    wallet1 = wallet1.connect(localChain1.provider());
    wallet2 = wallet2.connect(localChain2.provider());
    // Deploy the token.
    const localToken1 = await localChain2.deployToken(
      'Webb Token',
      'WEBB',
      wallet1
    );
    const localToken2 = await localChain2.deployToken(
      'Webb Token',
      'WEBB',
      wallet2
    );

    signatureBridge = await localChain1.deploySignatureBridge(
      localChain2,
      localToken1,
      localToken2,
      wallet1,
      wallet2
    );
    // save the chain configs.
    await localChain1.writeConfig(
      `${tmpDirPath}/${localChain1.name}.json`,
      signatureBridge
    );
    await localChain2.writeConfig(
      `${tmpDirPath}/${localChain2.name}.json`,
      signatureBridge
    );

    // get the anhor on localchain1
    const anchor = signatureBridge.getAnchor(
      localChain1.chainId,
      ethers.utils.parseEther('1')
    )!;
    await anchor.setSigner(wallet1);
    // approve token spending
    const tokenAddress = signatureBridge.getWebbTokenAddress(
      localChain1.chainId
    )!;
    const token = await Tokens.MintableToken.tokenFromAddress(
      tokenAddress,
      wallet1
    );
    await token.approveSpending(anchor.contract.address);
    await token.mintTokens(wallet1.address, ethers.utils.parseEther('1000'));

    // do the same but on localchain2
    const anchor2 = signatureBridge.getAnchor(
      localChain2.chainId,
      ethers.utils.parseEther('1')
    )!;
    await anchor2.setSigner(wallet2);
    const tokenAddress2 = signatureBridge.getWebbTokenAddress(
      localChain2.chainId
    )!;
    const token2 = await Tokens.MintableToken.tokenFromAddress(
      tokenAddress2,
      wallet2
    );
    await token2.approveSpending(anchor2.contract.address);
    await token2.mintTokens(wallet2.address, ethers.utils.parseEther('1000'));

    // now start the relayer
    webbRelayer = new WebbRelayer({
      port: 9955,
      tmp: true,
      configDir: tmpDirPath,
    });
    await webbRelayer.waitUntilReady();
  });

  test.todo('it should relay transaction');

  afterAll(async () => {
    await localChain1.stop();
    await localChain2.stop();
    await webbRelayer.stop();
    tmp.cleanupSync(); // clean up the temp dir.
  });
});
