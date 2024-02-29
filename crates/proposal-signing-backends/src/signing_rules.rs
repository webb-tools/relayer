// Copyright (C) 2022-2024 Webb Technologies Inc.
//
// Tangle is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// Tangle is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should receive a copy of the GNU General Public License
// If not, see <http://www.gnu.org/licenses/>.

use crate::SigningRulesContractWrapper;
use std::sync::Arc;
use tokio::sync::Mutex;
use webb::evm::ethers::types::transaction::eip2718::TypedTransaction;
use webb::evm::ethers::utils;
use webb_proposals::ProposalTrait;
use webb_relayer_store::queue::{
    QueueItem, QueueStore, TransactionQueueItemKey,
};
use webb_relayer_store::sled::SledQueueKey;
use webb_relayer_store::SledStore;
use webb_relayer_types::EthersClient;
use webb_relayer_utils::metric;

/// A ProposalSigningBackend that uses Signing Rules Contract for Signing Proposals.
#[derive(typed_builder::TypedBuilder)]
pub struct SigningRulesBackend {
    /// Signing rules contract
    wrapper: SigningRulesContractWrapper<EthersClient>,
    /// Something that implements the QueueStore trait.
    #[builder(setter(into))]
    store: Arc<SledStore>,
    /// The chain id of the chain that this backend is running on.
    ///
    /// This used as the source chain id for the proposals.
    #[builder(setter(into))]
    src_chain_id: u32,
}

//AnchorUpdateProposal for evm
#[async_trait::async_trait]
impl super::ProposalSigningBackend for SigningRulesBackend {
    async fn can_handle_proposal(
        &self,
        _proposal: &(impl ProposalTrait + Sync + Send + 'static),
    ) -> webb_relayer_utils::Result<bool> {
        Ok(true)
    }

    async fn handle_proposal(
        &self,
        proposal: &(impl ProposalTrait + Sync + Send + 'static),
        _metrics: Arc<Mutex<metric::Metrics>>,
    ) -> webb_relayer_utils::Result<()> {
        let resource_id = proposal.header().resource_id();
        let nonce = proposal.header().nonce();
        tracing::debug!(
            nonce = nonce.0,
            resource_id = hex::encode(resource_id.into_bytes()),
            src_chain_id = ?self.src_chain_id,
            proposal = format!("0x{}", hex::encode(proposal.to_vec())),
            "Sending proposal for voting though signing rules contract"
        );

        let phase1_job_id = self.wrapper.config.phase1_job_id;
        // TODO: Remove phase1 job details if not required, for now using dummy.
        let phase1_job_details = vec![1u8; 32];
        let phase2_job_details = proposal.to_vec();
        let call = self.wrapper.contract.vote_proposal(
            phase1_job_id,
            phase1_job_details.into(),
            phase2_job_details.into(),
        );

        let typed_tx: TypedTransaction = call.tx;
        let item = QueueItem::new(typed_tx.clone());
        let tx_key = SledQueueKey::from_evm_with_custom_key(
            self.src_chain_id,
            typed_tx.item_key(),
        );
        let proposal_data_hash = utils::keccak256(proposal.to_vec());
        // check if we already have a queued tx for this proposal.
        // if we do, we should not enqueue it again.
        let qq = QueueStore::<TypedTransaction>::has_item(&self.store, tx_key)?;
        if qq {
            tracing::debug!(
                proposal_data_hash = %hex::encode(proposal_data_hash),
                "Skipping execution of this proposal: Already Exists in Queue",
            );
            return Ok(());
        }

        QueueStore::<TypedTransaction>::enqueue_item(
            &self.store,
            tx_key,
            item,
        )?;
        tracing::debug!(
            proposal_data_hash = %hex::encode(proposal_data_hash),
            "Enqueued voting call for Anchor update proposal through evm tx queue",
        );
        Ok(())
    }
}