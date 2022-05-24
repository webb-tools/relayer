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
use super::VAnchorContractWrapper;
use crate::events_watcher::proposal_signing_backend::ProposalSigningBackend;
use crate::store::sled::SledStore;
use crate::store::EventHashStore;
use std::sync::Arc;
use webb::evm::contract::protocol_solidity::VAnchorContractEvents;
use webb::evm::ethers::prelude::{LogMeta, Middleware};
use webb::evm::ethers::providers;

type HttpProvider = providers::Provider<providers::Http>;
/// Represents an VAnchor Contract Watcher which will use a configured signing backend for signing proposals.
pub struct VAnchorWatcher<B> {
    proposal_signing_backend: B,
}

impl<B> VAnchorWatcher<B>
where
    B: ProposalSigningBackend<webb_proposals::AnchorUpdateProposal>,
{
    pub fn new(proposal_signing_backend: B) -> Self {
        Self {
            proposal_signing_backend,
        }
    }
}

#[async_trait::async_trait]
impl<B> super::EventWatcher for VAnchorWatcher<B>
where
    B: ProposalSigningBackend<webb_proposals::AnchorUpdateProposal>
        + Send
        + Sync,
{
    const TAG: &'static str = "VAnchor Watcher";
    type Middleware = HttpProvider;

    type Contract = VAnchorContractWrapper<Self::Middleware>;

    type Events = VAnchorContractEvents;

    type Store = SledStore;

    #[tracing::instrument(skip_all)]
    async fn handle_event(
        &self,
        store: Arc<Self::Store>,
        wrapper: &Self::Contract,
        (event, _): (Self::Events, LogMeta),
    ) -> anyhow::Result<()> {
        use VAnchorContractEvents::*;
        let event_data = match event {
            InsertionFilter(data) => data,
            _ => return Ok(()),
        };
        // Only construct the `AnchorUpdateProposal` if this condition evaluates to `true`: `leaf_index % 2 != 0`
        // The reason behind this is that `VAnchor` on every `transact` call, emits two events,
        // similar to the `Deposit` event but we call it the `Insertion` event, a la two `UTXO`
        // and since we only need to update the target `VAnchor` only when needed,
        // the first `Insertion` event sounds redundant in this case.
        tracing::debug!(
            event = ?event_data,
            "VAnchor new leaf event",
        );

        if event_data.leaf_index % 2 == 0 {
            tracing::debug!(
                leaf_index = %event_data.leaf_index,
                is_even_index = %event_data.leaf_index % 2 == 0,
                "VAnchor new leaf index does not satisfy the condition, skipping proposal.",
            );
            return Ok(());
        }

        let client = wrapper.contract.client();
        let src_chain_id = client.get_chainid().await?;
        let root = wrapper.contract.get_last_root().call().await?;
        let leaf_index = event_data.leaf_index;
        let function_signature = [141, 9, 22, 157];
        let nonce = event_data.leaf_index;
        for linked_anchor in &wrapper.config.linked_anchors {
            let dest_chain = linked_anchor.chain.to_lowercase();
            let maybe_chain = wrapper.webb_config.evm.get(&dest_chain);
            let dest_chain = match maybe_chain {
                Some(chain) => chain,
                None => continue,
            };
            let target_system =
                webb_proposals::TargetSystem::new_contract_address(
                    linked_anchor.address.to_fixed_bytes(),
                );
            let typed_chain_id =
                webb_proposals::TypedChainId::Evm(dest_chain.chain_id as _);
            let resource_id =
                webb_proposals::ResourceId::new(target_system, typed_chain_id);
            let header = webb_proposals::ProposalHeader::new(
                resource_id,
                function_signature.into(),
                nonce.into(),
            );
            let proposal = webb_proposals::AnchorUpdateProposal::new(
                header,
                webb_proposals::TypedChainId::Evm(src_chain_id.as_u32()),
                leaf_index,
                root,
                target_system.into_fixed_bytes(),
            );
            let can_sign_proposal = self
                .proposal_signing_backend
                .can_handle_proposal(&proposal)
                .await?;
            if can_sign_proposal {
                self.proposal_signing_backend
                    .handle_proposal(&proposal)
                    .await?;
            } else {
                tracing::warn!(
                    "Anchor update proposal is not supported by the signing backend"
                );
            }
        }
        // mark this event as processed.

        let events_bytes = serde_json::to_vec(&event_data)?;
        store.store_event(&events_bytes)?;
        Ok(())
    }
}
