use std::sync::Arc;

use webb::substrate::dkg_runtime::api::dkg_proposal_handler;
use webb::substrate::{dkg_runtime, subxt};

use crate::config::{self, Contract};
use crate::store::sled::{SledQueueKey, SledStore};
use crate::store::{BridgeCommand, BridgeKey, QueueStore};

use super::{BlockNumberOf, SubstrateEventWatcher};

#[derive(Clone, Debug)]
pub struct ProposalHandlerWatcher {
    webb_config: config::WebbRelayerConfig,
}

impl ProposalHandlerWatcher {
    pub fn new(webb_config: config::WebbRelayerConfig) -> Self {
        Self { webb_config }
    }
}

#[async_trait::async_trait]
impl SubstrateEventWatcher for ProposalHandlerWatcher {
    const TAG: &'static str = "DKG Signed Proposal Watcher";

    type RuntimeConfig = subxt::DefaultConfig;

    type Api = dkg_runtime::api::RuntimeApi<
        Self::RuntimeConfig,
        subxt::DefaultExtra<Self::RuntimeConfig>,
    >;

    type Event = dkg_proposal_handler::events::ProposalSigned;

    type Store = SledStore;

    async fn handle_event(
        &self,
        store: Arc<Self::Store>,
        _api: Arc<Self::Api>,
        (event, block_number): (Self::Event, BlockNumberOf<Self>),
    ) -> anyhow::Result<()> {
        tracing::debug!(
            "Received `ProposalSigned` Event: {:?} at block number: #{}",
            event,
            block_number
        );
        // we need to signal all the signature bridges in our system with this proposal.
        let bridge_keys = self.webb_config.evm.values().flat_map(|c| {
            c.contracts
                .iter()
                .filter_map(move |contract| match contract {
                    Contract::SignatureBridge(v) => Some(BridgeKey::new(
                        v.common.address,
                        c.chain_id.into(),
                    )),
                    _ => None,
                })
        }); // there is no need to collect here, since we can iterate over the keys.

        // now we just signal each bridge with the proposal.
        for bridge_key in bridge_keys {
            tracing::debug!(
                %bridge_key,
                proposal = ?event,
                "Signaling Signature Bridge to execute proposal",
            );
            store.enqueue_item(
                SledQueueKey::from_bridge_key(bridge_key),
                BridgeCommand::ExecuteProposalWithSignature {
                    data: event.data.clone(),
                    signature: event.signature.clone(),
                },
            )?;
        }
        Ok(())
    }
}
