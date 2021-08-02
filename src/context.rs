use std::convert::TryFrom;
use std::time::Duration;

use webb::evm::ethers::core::k256::SecretKey;
use webb::evm::ethers::prelude::*;

use crate::chains::evm::{ChainName, EvmChain};
use crate::config;

#[derive(Clone)]
pub struct RelayerContext {
    pub config: config::WebbRelayerConfig,
}

impl RelayerContext {
    pub fn new(config: config::WebbRelayerConfig) -> Self { Self { config } }

    pub async fn evm_provider<C: EvmChain>(
        &self,
    ) -> anyhow::Result<Provider<Http>> {
        let endpoint = C::endpoint();
        let provider =
            Provider::try_from(endpoint)?.interval(Duration::from_millis(5u64));
        Ok(provider)
    }

    pub async fn evm_wallet<C: EvmChain>(&self) -> anyhow::Result<LocalWallet> {
        let evm = &self.config.evm;
        match C::name() {
            ChainName::Edgeware if evm.edgeware.is_some() => {
                let c = evm.edgeware.clone().unwrap();
                let pk = c.private_key;
                let key = SecretKey::from_bytes(pk.as_bytes())?;
                let wallet = LocalWallet::from(key).set_chain_id(C::chain_id());
                Ok(wallet)
            },
            ChainName::Webb if evm.webb.is_some() => {
                let c = evm.webb.clone().unwrap();
                let pk = c.private_key;
                let key = SecretKey::from_bytes(pk.as_bytes())?;
                let wallet = LocalWallet::from(key).set_chain_id(C::chain_id());
                Ok(wallet)
            },
            ChainName::Ganache if evm.ganache.is_some() => {
                let c = evm.ganache.clone().unwrap();
                let pk = c.private_key;
                let key = SecretKey::from_bytes(pk.as_bytes())?;
                let wallet = LocalWallet::from(key).set_chain_id(C::chain_id());
                Ok(wallet)
            },
            ChainName::Beresheet if evm.beresheet.is_some() => {
                let c = evm.beresheet.clone().unwrap();
                let pk = c.private_key;
                let key = SecretKey::from_bytes(pk.as_bytes())?;
                let wallet = LocalWallet::from(key).set_chain_id(C::chain_id());
                Ok(wallet)
            },
            ChainName::Harmony if evm.harmony.is_some() => {
                let c = evm.harmony.clone().unwrap();
                let pk = c.private_key;
                let key = SecretKey::from_bytes(pk.as_bytes())?;
                let wallet = LocalWallet::from(key).set_chain_id(C::chain_id());
                Ok(wallet)
            },
            _ => anyhow::bail!("Chain Not Configured!"),
        }
    }

    pub fn fee_percentage<C: EvmChain>(&self) -> anyhow::Result<f64> {
        let evm = &self.config.evm;
        match C::name() {
            ChainName::Edgeware if evm.edgeware.is_some() => {
                let c = evm.edgeware.clone().unwrap();
                Ok(c.withdrew_fee_percentage)
            },
            ChainName::Webb if evm.webb.is_some() => {
                let c = evm.webb.clone().unwrap();
                Ok(c.withdrew_fee_percentage)
            },
            ChainName::Ganache if evm.ganache.is_some() => {
                let c = evm.ganache.clone().unwrap();
                Ok(c.withdrew_fee_percentage)
            },
            ChainName::Beresheet if evm.beresheet.is_some() => {
                let c = evm.beresheet.clone().unwrap();
                Ok(c.withdrew_fee_percentage)
            },
            ChainName::Harmony if evm.harmony.is_some() => {
                let c = evm.harmony.clone().unwrap();
                Ok(c.withdrew_fee_percentage)
            },
            _ => anyhow::bail!("Chain Not Configured!"),
        }
    }

    pub fn reward_account<C: EvmChain>(&self) -> anyhow::Result<Option<Address>> {
        let evm = &self.config.evm;
        match C::name() {
            ChainName::Edgeware if evm.edgeware.is_some() => {
                let c = evm.edgeware.clone().unwrap();
                Ok(c.account)
            },
            ChainName::Webb if evm.webb.is_some() => {
                let c = evm.webb.clone().unwrap();
                Ok(c.account)
            },
            ChainName::Ganache if evm.ganache.is_some() => {
                let c = evm.ganache.clone().unwrap();
                Ok(c.account)
            },
            ChainName::Beresheet if evm.beresheet.is_some() => {
                let c = evm.beresheet.clone().unwrap();
                Ok(c.account)
            },
            ChainName::Harmony if evm.harmony.is_some() => {
                let c = evm.harmony.clone().unwrap();
                Ok(c.account)
            },
            _ => anyhow::bail!("Chain Not Configured!"),
        }
    }

}
