use std::sync::Arc;

use ethers::prelude::*;
use futures::prelude::*;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use webb::evm::contract::anchor::AnchorContract;
use webb::evm::ethers;

/// A Leaf Cache Store is a simple trait that would help in
/// getting the leaves and insert them with a simple API.
pub trait LeafCacheStore {
    type Output: IntoIterator<Item = H256>;

    fn get_leaves(&self, contract: Address) -> anyhow::Result<Self::Output>;

    fn insert_leaves(
        &self,
        contract: Address,
        leaves: &[(u32, H256)],
    ) -> anyhow::Result<()>;
    /// Sets the new block number for the cache and returns the old one.
    fn set_last_block_number(&self, block_number: u64) -> anyhow::Result<u64>;
    fn get_last_block_number(&self) -> anyhow::Result<u64>;
}

type MemStore = HashMap<Address, Vec<(u32, H256)>>;

#[derive(Debug, Clone, Default)]
pub struct InMemoryLeafCache {
    store: Arc<RwLock<MemStore>>,
    last_block_number: Arc<AtomicU64>,
}

impl LeafCacheStore for InMemoryLeafCache {
    type Output = Vec<H256>;

    fn get_leaves(&self, contract: Address) -> anyhow::Result<Self::Output> {
        let guard = self.store.read();
        let val = guard
            .get(&contract)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|v| v.1)
            .collect();
        Ok(val)
    }

    fn insert_leaves(
        &self,
        contract: Address,
        leaves: &[(u32, H256)],
    ) -> anyhow::Result<()> {
        let mut guard = self.store.write();
        guard
            .entry(contract)
            .and_modify(|v| v.extend_from_slice(leaves))
            .or_insert_with(|| leaves.to_vec());
        Ok(())
    }

    fn get_last_block_number(&self) -> anyhow::Result<u64> {
        let val = self.last_block_number.load(Ordering::Relaxed);
        Ok(val)
    }

    fn set_last_block_number(&self, block_number: u64) -> anyhow::Result<u64> {
        let old = self.last_block_number.swap(block_number, Ordering::Relaxed);
        Ok(old)
    }
}

#[derive(Debug, Clone)]
pub struct LeavesWatcher<S> {
    ws_endpoint: String,
    store: S,
    contract: Address,
}

impl<S> LeavesWatcher<S>
where
    S: LeafCacheStore,
{
    #[allow(unused)]
    pub fn new(
        ws_endpoint: impl Into<String>,
        store: S,
        contract: Address,
    ) -> Self {
        Self {
            ws_endpoint: ws_endpoint.into(),
            contract,
            store,
        }
    }

    #[allow(unused)]
    pub async fn watch(self) -> anyhow::Result<()> {
        log::debug!("Connecting to {}", self.ws_endpoint);
        let ws = Ws::connect(&self.ws_endpoint).await?;
        let fetch_interval = Duration::from_millis(200);
        let provider = Provider::new(ws).interval(fetch_interval);
        let client = Arc::new(provider);
        let contract = AnchorContract::new(self.contract, client.clone());
        let block = self.store.get_last_block_number()?;
        log::debug!("Starting from block {}", block + 1);
        let filter = contract.deposit_filter().from_block(block + 1);
        let missing_events = filter.query_with_meta().await?;
        log::debug!("Got #{} missing events", missing_events.len());
        for (e, log) in missing_events {
            self.store.insert_leaves(
                self.contract,
                &[(e.leaf_index, H256::from_slice(&e.commitment))],
            )?;
            let old = self
                .store
                .set_last_block_number(log.block_number.as_u64())?;
            log::debug!(
                "Going from #{} to #{}",
                old,
                log.block_number.as_u64()
            );
        }
        let events = filter.subscribe().await?;
        let mut events_with_meta = events.with_meta();
        while let Some((e, log)) = events_with_meta.try_next().await? {
            self.store.insert_leaves(
                self.contract,
                &[(e.leaf_index, H256::from_slice(&e.commitment))],
            )?;
            self.store
                .set_last_block_number(log.block_number.as_u64())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::convert::TryFrom;

    use rand::rngs::StdRng;
    use rand::SeedableRng;

    use crate::test_utils::*;

    use super::*;
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn watcher() -> anyhow::Result<()> {
        env_logger::builder().is_test(true).init();
        let ganache = launch_ganache().await;
        let provider = Provider::<Http>::try_from(ganache.endpoint())?
            .interval(Duration::from_millis(10u64));
        let key = ganache.keys().first().cloned().unwrap();
        let wallet = LocalWallet::from(key).with_chain_id(1337u64);
        let client = SignerMiddleware::new(provider, wallet);
        let client = Arc::new(client);
        let contract_address = deploy_anchor_contract(client.clone()).await?;
        let contract = AnchorContract::new(contract_address, client.clone());
        let mut expected_leaves = Vec::new();
        let mut rng = StdRng::from_seed([0u8; 32]);
        // make a couple of deposit now, before starting the watcher.
        make_deposit(&mut rng, &contract, &mut expected_leaves).await?;
        make_deposit(&mut rng, &contract, &mut expected_leaves).await?;
        let store = InMemoryLeafCache::default();
        let leaves_watcher = LeavesWatcher::new(
            ganache.ws_endpoint(),
            store.clone(),
            contract_address,
        );
        // run the leaves watcher in another task
        let task_handle = tokio::task::spawn(leaves_watcher.watch());
        // then, make another deposit, while the watcher is running.
        make_deposit(&mut rng, &contract, &mut expected_leaves).await?;
        // it should now contains the 2 leaves when the watcher was offline, and
        // the new one that happened while it is watching.
        let leaves = store.get_leaves(contract_address)?;
        assert_eq!(expected_leaves, leaves);
        // now let's abort it, and try to do another deposit.
        task_handle.abort();
        make_deposit(&mut rng, &contract, &mut expected_leaves).await?;
        // let's run it again, using the same old store.
        let leaves_watcher = LeavesWatcher::new(
            ganache.ws_endpoint(),
            store.clone(),
            contract_address,
        );
        let task_handle = tokio::task::spawn(leaves_watcher.watch());
        log::debug!("Waiting for 5s allowing the task to run..");
        // let's wait for a bit.. to allow the task to run.
        tokio::time::sleep(Duration::from_secs(5)).await;
        // now it should now contain all the old leaves + the missing one.
        let leaves = store.get_leaves(contract_address)?;
        assert_eq!(expected_leaves, leaves);
        task_handle.abort();
        Ok(())
    }
}
