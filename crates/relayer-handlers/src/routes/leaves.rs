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

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::{collections::HashMap, convert::Infallible, sync::Arc};

use ethereum_types::Address;
use serde::Serialize;
use webb_proposals::{
    ResourceId, SubstrateTargetSystem, TargetSystem, TypedChainId,
};
use webb_relayer_context::RelayerContext;
use webb_relayer_store::LeafCacheStore;

use super::{OptionalRangeQuery, UnsupportedFeature};

// Leaves cache response
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeavesCacheResponse {
    leaves: Vec<Vec<u8>>,
    last_queried_block: u64,
}

pub struct LeavesError(StatusCode, String);

impl IntoResponse for LeavesError {
    fn into_response(self) -> Response {
        (self.0, Json(UnsupportedFeature { message: self.1 })).into_response()
    }
}

/// Handles leaf data requests for evm
///
/// Returns a Result with the `LeafDataResponse` on success
///
/// # Arguments
///
/// * `chain_id` - An u32 representing the chain id of the chain to query
/// * `contract` - An address of the contract to query
/// * `query_range` - An Optinal Query range.
pub async fn handle_leaves_cache_evm(
    State(ctx): State<Arc<RelayerContext>>,
    Path((chain_id, contract)): Path<(u32, Address)>,
    Query(query_range): Query<OptionalRangeQuery>,
) -> Result<Json<LeavesCacheResponse>, LeavesError> {
    let config = ctx.config.clone();
    // check if data query is enabled for relayer
    if !config.features.data_query {
        tracing::warn!("Data query is not enabled for relayer.");
        return Err(LeavesError(
            StatusCode::FORBIDDEN,
            "Data query is not enabled for relayer.".to_string(),
        ));
    }

    // check if chain is supported
    let chain = match ctx.config.evm.get(&chain_id.to_string()) {
        Some(v) => v,
        None => {
            tracing::warn!("Unsupported Chain: {chain_id}");
            return Err(LeavesError(
                StatusCode::BAD_REQUEST,
                format!("Unsupported Chain: {chain_id}"),
            ));
        }
    };

    let supported_contracts: HashMap<_, _> = chain
        .contracts
        .iter()
        .cloned()
        .filter_map(|c| match c {
            webb_relayer_config::evm::Contract::VAnchor(c)
            | webb_relayer_config::evm::Contract::OpenVAnchor(c) => {
                Some((c.common.address, c.events_watcher))
            }
            _ => None,
        })
        .collect();

    // check if contract is supported
    let event_watcher_config = match supported_contracts.get(&contract) {
        Some(config) => config,
        None => {
            tracing::warn!(
                "Unsupported Contract: {contract} for chaind : {chain_id}"
            );
            return Err(LeavesError(
                StatusCode::BAD_REQUEST,
                format!(
                    "Unsupported Contract: {contract} for chaind : {chain_id}",
                ),
            ));
        }
    };
    // check if data query is enabled for contract
    if !event_watcher_config.enable_data_query {
        tracing::warn!("Enbable data query for contract : ({contract})");
        return Err(LeavesError(
            StatusCode::FORBIDDEN,
            format!("Enbable data query for contract : ({contract})"),
        ));
    }
    // create history store key
    let src_target_system =
        TargetSystem::new_contract_address(contract.to_fixed_bytes());
    let src_typed_chain_id = TypedChainId::Evm(chain_id);
    let history_store_key =
        ResourceId::new(src_target_system, src_typed_chain_id);
    let leaves = ctx
        .store()
        .get_leaves_with_range(history_store_key, query_range.into())
        .unwrap();
    let last_queried_block = ctx
        .store()
        .get_last_deposit_block_number(history_store_key)
        .unwrap();

    Ok(Json(LeavesCacheResponse {
        leaves,
        last_queried_block,
    }))
}

/// Handles leaf data requests for substrate
///
/// Returns a Result with the `LeafDataResponse` on success
///
/// # Arguments
///
/// * `store` - [Sled](https://sled.rs)-based database store
/// * `chain_id` - An u32 representing the chain id of the chain to query
/// * `tree_id` - Tree id of the the source system to query
/// * `pallet_id` - Pallet id of the the source system to query
/// * `query_range` - An Optinal Query range.
/// * `ctx` - RelayContext reference that holds the configuration
pub async fn handle_leaves_cache_substrate(
    store: Arc<webb_relayer_store::sled::SledStore>,
    chain_id: u32,
    tree_id: u32,
    pallet_id: u8,
    query_range: OptionalRangeQuery,
    ctx: Arc<RelayerContext>,
) -> Result<impl warp::Reply, Infallible> {
    let config = ctx.config.clone();
    // check if data querying is enabled
    if !config.features.data_query {
        tracing::warn!("Data query is not enabled for relayer.");
        return Ok(warp::reply::with_status(
            warp::reply::json(&UnsupportedFeature {
                message: "Data query is not enabled for relayer.".to_string(),
            }),
            warp::http::StatusCode::FORBIDDEN,
        ));
    }

    // create history store key
    let src_typed_chain_id = TypedChainId::Substrate(chain_id);
    let target = SubstrateTargetSystem::builder()
        .pallet_index(pallet_id)
        .tree_id(tree_id)
        .build();
    let src_target_system = TargetSystem::Substrate(target);
    let history_store_key =
        ResourceId::new(src_target_system, src_typed_chain_id);

    let leaves = store
        .get_leaves_with_range(history_store_key, query_range.into())
        .unwrap();
    let last_queried_block = store
        .get_last_deposit_block_number(history_store_key)
        .unwrap();

    Ok(warp::reply::with_status(
        warp::reply::json(&LeavesCacheResponse {
            leaves,
            last_queried_block,
        }),
        warp::http::StatusCode::OK,
    ))
}
