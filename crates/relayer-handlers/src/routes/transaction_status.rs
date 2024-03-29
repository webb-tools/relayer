use std::sync::Arc;

use axum::extract::{Path, State};

use axum::http::StatusCode;
use axum::Json;
use ethereum_types::H512;
use serde::Serialize;
use webb::evm::ethers::types::transaction::eip2718::TypedTransaction;
use webb_relayer_context::RelayerContext;
use webb_relayer_store::queue::{QueueItem, QueueStore};
use webb_relayer_store::{queue::QueueItemState, sled::SledQueueKey};
use webb_relayer_utils::HandlerError;

/// Transaction status response struct
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionStatusResponse {
    status: QueueItemState,
    item_key: String,
}

/// Handles transaction progress of item in queue for evm chains.
///
/// Returns a Result with the `TransactionStatusResponse` on success
///
/// # Arguments
///
/// * `chain_id` - An u32 representing the chain id of the chain.
/// * `item_key` - An 64 bytes hash string, used to access transaction item from queue.
pub async fn handle_transaction_status_evm(
    State(ctx): State<Arc<RelayerContext>>,
    Path((chain_id, item_key)): Path<(u32, H512)>,
) -> Result<Json<TransactionStatusResponse>, HandlerError> {
    let store = ctx.store();
    let maybe_item: Option<QueueItem<TypedTransaction>> = store
        .get_item(SledQueueKey::from_evm_with_custom_key(chain_id, item_key.0))
        .unwrap_or(None);

    if let Some(item) = maybe_item {
        return Ok(Json(TransactionStatusResponse {
            status: item.state(),
            item_key: item_key.to_string(),
        }));
    }
    Err(HandlerError(
        StatusCode::NOT_FOUND,
        format!("Transaction item for key : {} not found in queue", item_key),
    ))
}
