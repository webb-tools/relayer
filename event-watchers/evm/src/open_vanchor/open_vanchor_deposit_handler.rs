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

use super::OpenVAnchorContractWrapper;
use ethereum_types::H256;
use std::sync::Arc;
use tokio::sync::Mutex;
use webb::evm::contract::protocol_solidity::OpenVAnchorContractEvents;
use webb::evm::ethers::prelude::{LogMeta, Middleware};
use webb_bridge_registry_backends::BridgeRegistryBackend;
use webb_event_watcher_traits::evm::EventHandler;
use webb_event_watcher_traits::EthersClient;
use webb_proposal_signing_backends::{
    proposal_handler, ProposalSigningBackend,
};
use webb_relayer_config::anchor::LinkedAnchorConfig;
use webb_relayer_store::EventHashStore;
use webb_relayer_store::SledStore;
use webb_relayer_utils::metric;

/// Represents an VAnchor Contract Watcher which will use a configured signing backend for signing proposals.
pub struct OpenVAnchorDepositHandler<B, C> {
    proposal_signing_backend: B,
    bridge_registry_backend: C,
}

impl<B, C> OpenVAnchorDepositHandler<B, C>
where
    B: ProposalSigningBackend,
    C: BridgeRegistryBackend,
{
    pub fn new(
        proposal_signing_backend: B,
        bridge_registry_backend: C,
    ) -> Self {
        Self {
            proposal_signing_backend,
            bridge_registry_backend,
        }
    }
}

#[async_trait::async_trait]
impl<B, C> EventHandler for OpenVAnchorDepositHandler<B, C>
where
    B: ProposalSigningBackend + Send + Sync,
    C: BridgeRegistryBackend + Send + Sync,
{
    type Contract = OpenVAnchorContractWrapper<EthersClient>;

    type Events = OpenVAnchorContractEvents;

    type Store = SledStore;

    async fn can_handle_events(
        &self,
        (events, _logs): (Self::Events, LogMeta),
        _wrapper: &Self::Contract,
    ) -> webb_relayer_utils::Result<bool> {
        use OpenVAnchorContractEvents::*;
        let has_event = matches!(events, NewCommitmentFilter(_));
        Ok(has_event)
    }

    #[tracing::instrument(skip_all)]
    async fn handle_event(
        &self,
        store: Arc<Self::Store>,
        wrapper: &Self::Contract,
        (event, log): (Self::Events, LogMeta),
        metrics: Arc<Mutex<metric::Metrics>>,
    ) -> webb_relayer_utils::Result<()> {
        use OpenVAnchorContractEvents::*;
        let event_data = match event {
            NewCommitmentFilter(data) => {
                let chain_id = wrapper.contract.client().get_chainid().await?;
                let commitment: [u8; 32] = data.commitment.into();
                let info = (data.leaf_index.as_u32(), H256::from(commitment));
                tracing::event!(
                    target: webb_relayer_utils::probe::TARGET,
                    tracing::Level::DEBUG,
                    kind = %webb_relayer_utils::probe::Kind::MerkleTreeInsertion,
                    leaf_index = %info.0,
                    leaf = %info.1,
                    chain_id = %chain_id,
                    block_number = %log.block_number
                );
                data
            }
            _ => return Ok(()),
        };

        let client = wrapper.contract.client();
        let chain_id = client.get_chainid().await?;
        let root: [u8; 32] =
            wrapper.contract.get_last_root().call().await?.into();
        let leaf_index = event_data.leaf_index.as_u32();
        let src_chain_id = webb_proposals::TypedChainId::Evm(chain_id.as_u32());
        let src_target_system =
            webb_proposals::TargetSystem::new_contract_address(
                wrapper.contract.address().to_fixed_bytes(),
            );
        let src_resource_id =
            webb_proposals::ResourceId::new(src_target_system, src_chain_id);

        let linked_anchors = self
            .bridge_registry_backend
            .config_or_dkg_bridges(
                &wrapper.config.linked_anchors,
                &src_resource_id,
            )
            .await?;

        for linked_anchor in linked_anchors {
            let target_resource_id = match linked_anchor {
                LinkedAnchorConfig::Raw(target) => {
                    let bytes: [u8; 32] = target.resource_id.into();
                    webb_proposals::ResourceId::from(bytes)
                }
                _ => unreachable!("unsupported"),
            };

            let _ = match target_resource_id.target_system() {
                webb_proposals::TargetSystem::ContractAddress(_) => {
                    let proposal = proposal_handler::evm_anchor_update_proposal(
                        root,
                        leaf_index,
                        target_resource_id,
                        src_resource_id,
                    );
                    proposal_handler::handle_proposal(
                        &proposal,
                        &self.proposal_signing_backend,
                        metrics.clone(),
                    )
                    .await
                }
                webb_proposals::TargetSystem::Substrate(_) => {
                    let proposal =
                        proposal_handler::substrate_anchor_update_proposal(
                            root,
                            leaf_index,
                            target_resource_id,
                            src_resource_id,
                        );
                    proposal_handler::handle_proposal(
                        &proposal,
                        &self.proposal_signing_backend,
                        metrics.clone(),
                    )
                    .await
                }
            };
        }
        // mark this event as processed.
        let events_bytes = serde_json::to_vec(&event_data)?;
        store.store_event(&events_bytes)?;
        Ok(())
    }
}
