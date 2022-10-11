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
use crate::context::RelayerContext;
use crate::store::sled::SledQueueKey;
use crate::store::QueueStore;
use ethereum_types::U256;
use futures::StreamExt;
use futures::TryFutureExt;
use rand::Rng;

use std::sync::Arc;
use std::time::Duration;

use crate::types::dynamic_payload::WebbDynamicTxPayload;
use std::marker::PhantomData;
use webb::substrate::subxt;
use webb::substrate::subxt::ext::sp_core::sr25519;
use webb::substrate::subxt::ext::sp_runtime::traits::{
    IdentifyAccount, Verify,
};
use webb::substrate::subxt::tx::{
    ExtrinsicParams, PairSigner, TxStatus as TransactionStatus,
};

/// The SubstrateTxQueue stores transaction call params in bytes so the relayer can process them later.
/// This prevents issues such as creating transactions with the same nonce.
/// Randomized sleep intervals are used to prevent relayers from submitting
/// the same transaction.
#[derive(Clone)]
pub struct SubstrateTxQueue<'a, S>
where
    S: QueueStore<WebbDynamicTxPayload<'a>, Key = SledQueueKey>,
{
    ctx: RelayerContext,
    chain_id: U256,
    store: Arc<S>,
    _marker: PhantomData<&'a ()>,
}
impl<'a, S> SubstrateTxQueue<'a, S>
where
    S: QueueStore<WebbDynamicTxPayload<'a>, Key = SledQueueKey>,
{
    /// Creates a new SubstrateTxQueue instance.
    ///
    /// Returns a SubstrateTxQueue instance.
    ///
    /// # Arguments
    ///
    /// * `ctx` - RelayContext reference that holds the configuration
    /// * `chain_name` - The name of the chain that this queue is for
    /// * `store` - [Sled](https://sled.rs)-based database store
    ///
    /// # Examples
    ///
    /// ```
    /// use crate::tx_queue::SubstrateTxQueue;
    /// let tx_queue = SubstrateTxQueue::new(ctx, chain_name.clone(), store);
    /// ```

    pub fn new(ctx: RelayerContext, chain_id: U256, store: Arc<S>) -> Self {
        Self {
            ctx,
            chain_id,
            store,
            _marker: PhantomData {},
        }
    }
    /// Starts the SubstrateTxQueue service.
    ///
    /// Returns a future that resolves `Ok(())` on success, otherwise returns an error.
    ///
    /// # Examples
    ///
    /// ```
    /// use crate::tx_queue::SubstrateTxQueue;
    /// let tx_queue = TxQueue::new(ctx, chain_name.clone(), store);
    ///  let task = async move {
    ///     tokio::select! {
    ///         _ = tx_queue.run() => {
    ///             // do something
    ///         },
    ///         _ = shutdown_signal.recv() => {
    ///             // do something
    ///         },
    ///     }
    /// };
    /// ```
    #[tracing::instrument(skip_all, fields(node = %self.chain_id))]
    pub async fn run<X>(self) -> crate::Result<()>
    where
        X: subxt::Config,
        <<X>::ExtrinsicParams as ExtrinsicParams<<X>::Index, <X>::Hash>>::OtherParams:Default,
        <X>::Signature: From<sr25519::Signature>,
        <X>::Address: From<<X>::AccountId>,
        <<X>::Signature as Verify>::Signer: From<sr25519::Public> + IdentifyAccount<AccountId = <X>::AccountId>,

    {
        let chain_config = self
            .ctx
            .config
            .substrate
            .get(&self.chain_id.to_string())
            .ok_or(crate::Error::NodeNotFound {
                chain_id: self.chain_id.to_string(),
            })?;
        let chain_id = self.chain_id;
        let store = self.store;
        let backoff = backoff::ExponentialBackoff {
            max_elapsed_time: None,
            ..Default::default()
        };
        //  protocol-substrate client
        let client = self
            .ctx
            .substrate_provider::<X>(&chain_id.to_string())
            .await?;

        // get pair
        let pair = self.ctx.substrate_wallet(&chain_id.to_string()).await?;
        let signer = PairSigner::new(pair);

        tracing::event!(
            target: crate::probe::TARGET,
            tracing::Level::DEBUG,
            kind = %crate::probe::Kind::TxQueue,
            ty = "SUBSTRATE",
            chain_id = %chain_id.as_u64(),
            starting = true,
        );

        let metrics = self.ctx.metrics.clone();
        let task = || async {
            loop {
                tracing::trace!("Checking for any txs in the queue ...");
                // dequeue transaction call data. This are call params stored as bytes
                let maybe_call_data = store.dequeue_item(
                    SledQueueKey::from_substrate_chain_id(chain_id),
                )?;
                if let Some(payload) = maybe_call_data {
                    let dynamic_tx_payload = subxt::dynamic::tx(
                        payload.pallet_name,
                        payload.call_name,
                        payload.fields,
                    );
                    let signed_extrinsic = client
                        .tx()
                        .create_signed(
                            &dynamic_tx_payload,
                            &signer,
                            Default::default(),
                        )
                        .map_err(Into::into)
                        .map_err(backoff::Error::transient)
                        .await?;
                    // dry run test
                    let dry_run_outcome = signed_extrinsic.dry_run(None).await;
                    match dry_run_outcome {
                        Ok(_) => {
                            tracing::event!(
                                target: crate::probe::TARGET,
                                tracing::Level::DEBUG,
                                kind = %crate::probe::Kind::TxQueue,
                                ty = "SUBSTRATE",
                                chain_id = %chain_id.as_u64(),
                                dry_run = "passed"
                            );
                        }
                        Err(err) => {
                            tracing::event!(
                                target: crate::probe::TARGET,
                                tracing::Level::DEBUG,
                                kind = %crate::probe::Kind::TxQueue,
                                ty = "SUBSTRATE",
                                chain_id = %chain_id.as_u64(),
                                errored = true,
                                error = %err,
                                dry_run = "failed"
                            );
                            continue; // keep going.
                        }
                    }
                    // watch_extrinsic submits and returns transaction subscription
                    let mut progress = client
                        .tx()
                        .sign_and_submit_then_watch_default(
                            &dynamic_tx_payload,
                            &signer,
                        )
                        .map_err(Into::into)
                        .map_err(backoff::Error::transient)
                        .await?;

                    while let Some(event) = progress.next().await {
                        let e = match event {
                            Ok(e) => e,
                            Err(err) => {
                                tracing::event!(
                                    target: crate::probe::TARGET,
                                    tracing::Level::DEBUG,
                                    kind = %crate::probe::Kind::TxQueue,
                                    ty = "SUBSTRATE",
                                    chain_id = %chain_id.as_u64(),
                                    errored = true,
                                    error = %err,
                                );
                                continue; // keep going.
                            }
                        };

                        match e {
                            TransactionStatus::Future => {
                                tracing::event!(
                                    target: crate::probe::TARGET,
                                    tracing::Level::DEBUG,
                                    kind = %crate::probe::Kind::TxQueue,
                                    ty = "SUBSTRATE",
                                    chain_id = %chain_id.as_u64(),
                                    status = "Future",
                                );
                            }
                            TransactionStatus::Ready => {
                                tracing::event!(
                                    target: crate::probe::TARGET,
                                    tracing::Level::DEBUG,
                                    kind = %crate::probe::Kind::TxQueue,
                                    ty = "SUBSTRATE",
                                    chain_id = %chain_id.as_u64(),
                                    status = "Ready",
                                );
                            }
                            TransactionStatus::Broadcast(_) => {
                                tracing::event!(
                                    target: crate::probe::TARGET,
                                    tracing::Level::DEBUG,
                                    kind = %crate::probe::Kind::TxQueue,
                                    ty = "SUBSTRATE",
                                    chain_id = %chain_id.as_u64(),
                                    status = "Broadcast",
                                );
                            }
                            TransactionStatus::InBlock(_) => {
                                tracing::event!(
                                    target: crate::probe::TARGET,
                                    tracing::Level::DEBUG,
                                    kind = %crate::probe::Kind::TxQueue,
                                    ty = "SUBSTRATE",
                                    chain_id = %chain_id.as_u64(),
                                    status = "InBlock",
                                );
                            }
                            TransactionStatus::Retracted(_) => {
                                tracing::event!(
                                    target: crate::probe::TARGET,
                                    tracing::Level::DEBUG,
                                    kind = %crate::probe::Kind::TxQueue,
                                    ty = "SUBSTRATE",
                                    chain_id = %chain_id.as_u64(),
                                    status = "Retracted",
                                );
                            }
                            TransactionStatus::FinalityTimeout(_) => {
                                tracing::event!(
                                    target: crate::probe::TARGET,
                                    tracing::Level::DEBUG,
                                    kind = %crate::probe::Kind::TxQueue,
                                    ty = "SUBSTRATE",
                                    chain_id = %chain_id.as_u64(),
                                    status = "FinalityTimeout",
                                );
                            }
                            TransactionStatus::Finalized(_) => {
                                tracing::event!(
                                    target: crate::probe::TARGET,
                                    tracing::Level::DEBUG,
                                    kind = %crate::probe::Kind::TxQueue,
                                    ty = "SUBSTRATE",
                                    chain_id = %chain_id.as_u64(),
                                    status = "Finalized",
                                    finalized = true,
                                );
                                // metrics for proposal processed by substrate tx queue
                                metrics
                                    .proposals_processed_tx_queue_metric
                                    .inc();
                                metrics.proposals_processed_substrate_tx_queue_metric.inc();
                            }

                            TransactionStatus::Usurped(_) => {
                                tracing::event!(
                                    target: crate::probe::TARGET,
                                    tracing::Level::DEBUG,
                                    kind = %crate::probe::Kind::TxQueue,
                                    ty = "SUBSTRATE",
                                    chain_id = %chain_id.as_u64(),
                                    status = "Usurped",
                                );
                            }
                            TransactionStatus::Dropped => {
                                tracing::event!(
                                    target: crate::probe::TARGET,
                                    tracing::Level::DEBUG,
                                    kind = %crate::probe::Kind::TxQueue,
                                    ty = "SUBSTRATE",
                                    chain_id = %chain_id.as_u64(),
                                    status = "Dropped",
                                );
                            }
                            TransactionStatus::Invalid => {
                                tracing::event!(
                                    target: crate::probe::TARGET,
                                    tracing::Level::DEBUG,
                                    kind = %crate::probe::Kind::TxQueue,
                                    ty = "SUBSTRATE",
                                    chain_id = %chain_id.as_u64(),
                                    status = "Invalid",
                                );
                            }
                        }
                    }
                }
                // sleep for a random amount of time.
                let max_sleep_interval =
                    chain_config.tx_queue.max_sleep_interval;
                let s =
                    rand::thread_rng().gen_range(1_000..=max_sleep_interval);
                tracing::trace!("next queue round after {} ms", s);
                tokio::time::sleep(Duration::from_millis(s)).await;
            }
        };
        // transaction queue backoff metric
        metrics.transaction_queue_back_off_metric.inc();
        metrics.substrate_transaction_queue_back_off_metric.inc();
        backoff::future::retry::<(), _, _, _, _>(backoff, task).await?;
        Ok(())
    }
}
