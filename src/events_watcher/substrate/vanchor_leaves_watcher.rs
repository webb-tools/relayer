// Copyright 2022 Webb Technologies Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
use super::{BlockNumberOf, SubstrateEventWatcher};
use ethereum_types::H256;
use std::sync::Arc;
use webb::evm::ethers::types;
use webb::substrate::protocol_substrate_runtime;
use webb::substrate::protocol_substrate_runtime::api as RuntimeApi;
use webb::substrate::protocol_substrate_runtime::api::v_anchor_bn254;
use webb::substrate::subxt::{self, OnlineClient};
use webb_relayer_store::sled::SledStore;
use webb_relayer_store::LeafCacheStore;
// An Substrate VAnchor Leaves Watcher that watches for Deposit events and save the leaves to the store.
/// It serves as a cache for leaves that could be used by dApp for proof generation.
#[derive(Clone, Debug, Default)]
pub struct SubstrateVAnchorLeavesWatcher;

#[async_trait::async_trait]
impl SubstrateEventWatcher for SubstrateVAnchorLeavesWatcher {
    const TAG: &'static str = "Substrate V-Anchor leaves watcher";

    type RuntimeConfig = subxt::SubstrateConfig;

    type Client = OnlineClient<Self::RuntimeConfig>;

    type Event = protocol_substrate_runtime::api::Event;

    type FilteredEvent = v_anchor_bn254::events::Transaction;

    type Store = SledStore;

    async fn handle_event(
        &self,
        store: Arc<Self::Store>,
        api: Arc<Self::Client>,
        (event, block_number): (Self::FilteredEvent, BlockNumberOf<Self>),
    ) -> crate::Result<()> {
        let at_hash_addr = RuntimeApi::storage()
            .system()
            .block_hash(&(block_number as u64));
        let at_hash = api.storage().fetch(&at_hash_addr, None).await?.unwrap();

        // fetch leaf_index from merkle tree at given block_number
        let next_leaf_index_addr = RuntimeApi::storage()
            .merkle_tree_bn254()
            .next_leaf_index(&event.tree_id);
        let next_leaf_index = api
            .storage()
            .fetch(&next_leaf_index_addr, Some(at_hash))
            .await?
            .unwrap();

        // fetch chain_id
        let chain_id_addr = RuntimeApi::constants()
            .linkable_tree_bn254()
            .chain_identifier();
        let chain_id = api.constants().at(&chain_id_addr)?;
        let chain_id = types::U256::from(chain_id);

        let tree_id = event.tree_id.to_string();
        let leaf_count = event.leafs.len();
        let mut leaf_index = next_leaf_index.saturating_sub(leaf_count as u32);
        let mut leaf_store = Vec::with_capacity(leaf_count);
        for leaf in event.leafs {
            let leaf_value = H256::from_slice(&leaf.0);
            let value = (leaf_index, leaf_value);
            store.insert_leaves((chain_id, tree_id.clone()), &[value])?;
            store.insert_last_deposit_block_number(
                (chain_id, tree_id.clone()),
                types::U64::from(block_number),
            )?;
            leaf_index += 1;
            leaf_store.push(leaf_value);
        }
        tracing::event!(
            target: crate::probe::TARGET,
            tracing::Level::DEBUG,
            kind = %crate::probe::Kind::LeavesStore,
            chain_id = %chain_id,
            leaf_index = leaf_index,
            leafs = %format!("{:?}",leaf_store),
            tree_id = %tree_id,
            block_number = %block_number
        );
        Ok(())
    }
}
