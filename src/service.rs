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
//! # Relayer Service Module 🕸️
//!
//! A module for starting long-running tasks for event watching.
//!
//! ## Overview
//!
//! Services are tasks which the relayer constantly runs throughout its lifetime.
//! Services handle keeping up to date with the configured chains.

use std::sync::Arc;

use ethereum_types::U256;
use webb::evm::ethers::prelude::Middleware;
use webb::evm::ethers::providers;
use webb::substrate::dkg_runtime::api::runtime_types::webb_proposals::header::TypedChainId;
use webb::substrate::dkg_runtime::api::RuntimeApi as DkgRuntimeApi;
use webb::substrate::subxt;
use webb::substrate::subxt::PairSigner;

use crate::config::*;
use crate::context::RelayerContext;
use crate::events_watcher::signing_backend::*;
use crate::events_watcher::*;
use crate::tx_queue::TxQueue;
/// Type alias for providers
type Client = providers::Provider<providers::Http>;
/// Type alias for the DKG DefaultConfig
type DkgClient = subxt::Client<subxt::DefaultConfig>;
/// Type alias for the DKG RuntimeApi
type DkgRuntime = DkgRuntimeApi<
    subxt::DefaultConfig,
    subxt::DefaultExtra<subxt::DefaultConfig>,
>;
/// Type alias for [Sled](https://sled.rs)-based database store
type Store = crate::store::sled::SledStore;
/// Starts all background services for all chains configured in the config file.
///
/// Returns a future that resolves when all services are started successfully.
///
/// # Arguments
///
/// * `ctx` - RelayContext reference that holds the configuration
/// * `store` -[Sled](https://sled.rs)-based database store
///
/// # Examples
///
/// ```
/// let _ = service::ignite(&ctx, Arc::new(store)).await?;
/// ```
pub async fn ignite(
    ctx: &RelayerContext,
    store: Arc<Store>,
) -> anyhow::Result<()> {
    // now we go through each chain, in our configuration
    for (chain_name, chain_config) in &ctx.config.evm {
        if !chain_config.enabled {
            continue;
        }
        let provider = ctx.evm_provider(chain_name).await?;
        let client = Arc::new(provider);
        tracing::debug!(
            "Starting Background Services for ({}) chain.",
            chain_name
        );

        for contract in &chain_config.contracts {
            match contract {
                Contract::Tornado(config) => {
                    start_tornado_events_watcher(
                        ctx,
                        config,
                        client.clone(),
                        store.clone(),
                    )?;
                }
                Contract::Anchor(config) => {
                    start_anchor_events_watcher(
                        ctx,
                        config,
                        client.clone(),
                        store.clone(),
                    )
                    .await?;
                }
                Contract::SignatureBridge(config) => {
                    start_signature_bridge_events_watcher(
                        ctx,
                        config,
                        client.clone(),
                        store.clone(),
                    )
                    .await?;
                }
                Contract::GovernanceBravoDelegate(_) => {}
            }
        }
        // start the transaction queue after starting other tasks.
        start_tx_queue(ctx.clone(), chain_name.clone(), store.clone())?;
    }
    // now, we start substrate service/tasks
    for (node_name, node_config) in &ctx.config.substrate {
        if !node_config.enabled {
            continue;
        }
        match node_config.runtime {
            SubstrateRuntime::Dkg => {
                let client = ctx
                    .substrate_provider::<subxt::DefaultConfig>(node_name)
                    .await?;
                let api = client.clone().to_runtime_api::<DkgRuntime>();
                let chain_id =
                    api.constants().dkg_proposals().chain_identifier()?;
                let chain_id = match chain_id {
                    TypedChainId::None => 0,
                    TypedChainId::Evm(id)
                    | TypedChainId::Substrate(id)
                    | TypedChainId::PolkadotParachain(id)
                    | TypedChainId::KusamaParachain(id)
                    | TypedChainId::RococoParachain(id)
                    | TypedChainId::Cosmos(id)
                    | TypedChainId::Solana(id) => id,
                };
                let chain_id = U256::from(chain_id);
                for pallet in &node_config.pallets {
                    match pallet {
                        Pallet::DKGProposalHandler(config) => {
                            start_dkg_proposal_handler(
                                ctx,
                                config,
                                client.clone(),
                                node_name.clone(),
                                chain_id,
                                store.clone(),
                            )?;
                        }
                        Pallet::DKGProposals(_) => {
                            // TODO(@shekohex): start the dkg proposals service
                        }
                    }
                }
            }
            SubstrateRuntime::WebbProtocol => {
                // Handle Webb Protocol here
            }
        };
    }
    Ok(())
}
/// Starts the event watcher for DKG proposal handler events.
///
/// Returns Ok(()) if successful, or an error if not.
///
/// # Arguments
///
/// * `ctx` - RelayContext reference that holds the configuration
/// * `config` - DKG proposal handler configuration
/// * `client` - DKG client
/// * `node_name` - Name of the node
/// * `chain_id` - An U256 representing the chain id of the chain
/// * `store` -[Sled](https://sled.rs)-based database store
fn start_dkg_proposal_handler(
    ctx: &RelayerContext,
    config: &DKGProposalHandlerPalletConfig,
    client: DkgClient,
    node_name: String,
    chain_id: U256,
    store: Arc<Store>,
) -> anyhow::Result<()> {
    // check first if we should start the events watcher for this contract.
    if !config.events_watcher.enabled {
        tracing::warn!(
            "DKG Proposal Handler events watcher is disabled for ({}).",
            node_name,
        );
        return Ok(());
    }
    tracing::debug!(
        "DKG Proposal Handler events watcher for ({}) Started.",
        node_name,
    );
    let node_name2 = node_name.clone();
    let mut shutdown_signal = ctx.shutdown_signal();
    let webb_config = ctx.config.clone();
    let task = async move {
        let proposal_handler = ProposalHandlerWatcher::new(webb_config);
        let watcher = proposal_handler.run(node_name, chain_id, client, store);
        tokio::select! {
            _ = watcher => {
                tracing::warn!(
                    "DKG Proposal Handler events watcher stopped for ({})",
                    node_name2,
                );
            },
            _ = shutdown_signal.recv() => {
                tracing::trace!(
                    "Stopping DKG Proposal Handler events watcher for ({})",
                    node_name2,
                );
            },
        }
    };
    // kick off the watcher.
    tokio::task::spawn(task);
    Ok(())
}
/// Starts the event watcher for tornado events.
///
/// Returns Ok(()) if successful, or an error if not.
///
/// # Arguments
///
/// * `ctx` - RelayContext reference that holds the configuration
/// * `config` - Tornado contract configuration
/// * `client` - Tornado client * `store` -[Sled](https://sled.rs)-based database store
fn start_tornado_events_watcher(
    ctx: &RelayerContext,
    config: &TornadoContractConfig,
    client: Arc<Client>,
    store: Arc<Store>,
) -> anyhow::Result<()> {
    // check first if we should start the events watcher for this contract.
    if !config.events_watcher.enabled {
        tracing::warn!(
            "Tornado events watcher is disabled for ({}).",
            config.common.address,
        );
        return Ok(());
    }
    let wrapper = TornadoContractWrapper::new(config.clone(), client.clone());
    tracing::debug!(
        "Tornado events watcher for ({}) Started.",
        config.common.address,
    );
    let watcher = TornadoLeavesWatcher.run(client, store, wrapper);
    let mut shutdown_signal = ctx.shutdown_signal();
    let contract_address = config.common.address;
    let task = async move {
        tokio::select! {
            _ = watcher => {
                tracing::warn!(
                    "Tornado events watcher stopped for ({})",
                    contract_address,
                );
            },
            _ = shutdown_signal.recv() => {
                tracing::trace!(
                    "Stopping Tornado events watcher for ({})",
                    contract_address,
                );
            },
        }
    };
    // kick off the watcher.
    tokio::task::spawn(task);
    Ok(())
}
/// Starts the event watcher for Anchor events.
///
/// Returns Ok(()) if successful, or an error if not.
///
/// # Arguments
///
/// * `ctx` - RelayContext reference that holds the configuration
/// * `config` - Anchor contract configuration
/// * `client` - DKG client
/// * `store` -[Sled](https://sled.rs)-based database store
async fn start_anchor_events_watcher(
    ctx: &RelayerContext,
    config: &AnchorContractConfig,
    client: Arc<Client>,
    store: Arc<Store>,
) -> anyhow::Result<()> {
    if !config.events_watcher.enabled {
        tracing::warn!(
            "Anchor events watcher is disabled for ({}).",
            config.common.address,
        );
        return Ok(());
    }
    let wrapper = AnchorContractWrapper::new(
        config.clone(),
        ctx.config.clone(), // the original config to access all networks.
        client.clone(),
    );
    let mut shutdown_signal = ctx.shutdown_signal();
    let contract_address = config.common.address;
    let my_ctx = ctx.clone();
    let signing_backend = config.signing_backend.clone();
    let task = async move {
        tracing::debug!(
            "Anchor events watcher for ({}) Started.",
            contract_address,
        );
        let leaves_watcher = AnchorLeavesWatcher::default();
        let anchor_leaves_watcher =
            leaves_watcher.run(client.clone(), store.clone(), wrapper.clone());
        match signing_backend {
            SigningBackendConfig::DkgNode(c) => {
                let dkg_client = my_ctx
                    .substrate_provider::<subxt::DefaultConfig>(&c.node)
                    .await?;
                let pair = my_ctx.substrate_wallet(&c.node).await?;
                let backend =
                    DkgSigningBackend::new(dkg_client, PairSigner::new(pair));
                let watcher = AnchorWatcher::new(backend);
                let anchor_watcher_task = watcher.run(client, store, wrapper);
                tokio::select! {
                    _ = anchor_watcher_task => {
                        tracing::warn!(
                            "Anchor watcher task stopped for ({})",
                            contract_address,
                        );
                    },
                    _ = anchor_leaves_watcher => {
                        tracing::warn!(
                            "Anchor leaves watcher stopped for ({})",
                            contract_address,
                        );
                    },
                    _ = shutdown_signal.recv() => {
                        tracing::trace!(
                            "Stopping Anchor watcher for ({})",
                            contract_address,
                        );
                    },
                }
            }
            SigningBackendConfig::Mocked(c) => {
                let chain_id = client.get_chainid().await?;
                let signature_bridge_address = my_ctx
                    .config
                    .evm
                    .values()
                    .find(|c| c.chain_id == chain_id.as_u64())
                    .and_then(|c| {
                        c.contracts.iter().find(|contract| {
                            matches!(contract, Contract::SignatureBridge(_))
                        })
                    })
                    .and_then(|contract| match contract {
                        Contract::SignatureBridge(bridge) => Some(bridge),
                        _ => None,
                    })
                    .map(|config| config.common.address)
                    .ok_or(anyhow::anyhow!(
                        "No SignatureBridge contract found"
                    ))?;
                let backend = MockedSigningBackend::builder()
                    .store(store.clone())
                    .private_key(c.private_key)
                    .chain_id(chain_id.as_u64())
                    .signature_bridge_address(signature_bridge_address)
                    .build();
                let watcher = AnchorWatcher::new(backend);
                let anchor_watcher_task = watcher.run(client, store, wrapper);
                tokio::select! {
                    _ = anchor_watcher_task => {
                        tracing::warn!(
                            "Anchor watcher task stopped for ({})",
                            contract_address,
                        );
                    },
                    _ = anchor_leaves_watcher => {
                        tracing::warn!(
                            "Anchor leaves watcher stopped for ({})",
                            contract_address,
                        );
                    },
                    _ = shutdown_signal.recv() => {
                        tracing::trace!(
                            "Stopping Anchor watcher for ({})",
                            contract_address,
                        );
                    },
                }
            }
        };

        Result::<_, anyhow::Error>::Ok(())
    };
    // kick off the watcher.
    tokio::task::spawn(task);

    Ok(())
}

/// Starts the event watcher for Signature Bridge contract.
async fn start_signature_bridge_events_watcher(
    ctx: &RelayerContext,
    config: &SignatureBridgeContractConfig,
    client: Arc<Client>,
    store: Arc<Store>,
) -> anyhow::Result<()> {
    if !config.events_watcher.enabled {
        tracing::warn!(
            "Signature Bridge events watcher is disabled for ({}).",
            config.common.address,
        );
        return Ok(());
    }
    let mut shutdown_signal = ctx.shutdown_signal();
    let contract_address = config.common.address;
    let wrapper =
        SignatureBridgeContractWrapper::new(config.clone(), client.clone());
    let task = async move {
        tracing::debug!("Bridge watcher for ({}) Started.", contract_address);
        let bridge_contract_watcher = SignatureBridgeContractWatcher::default();
        let events_watcher_task = EventWatcher::run(
            &bridge_contract_watcher,
            client.clone(),
            store.clone(),
            wrapper.clone(),
        );
        let cmd_handler_task = BridgeWatcher::run(
            &bridge_contract_watcher,
            client,
            store,
            wrapper,
        );
        tokio::select! {
            _ = events_watcher_task => {
                tracing::warn!(
                    "signature bridge events watcher task stopped for ({})",
                    contract_address
                );
            },
            _ = cmd_handler_task => {
                tracing::warn!(
                    "signature bridge cmd handler task stopped for ({})",
                    contract_address
                );
            },
            _ = shutdown_signal.recv() => {
                tracing::trace!(
                    "Stopping Signature Bridge watcher for ({})",
                    contract_address,
                );
            },
        }
    };
    // kick off the watcher.
    tokio::task::spawn(task);
    Ok(())
}

/// Starts the transaction queue task
///
/// Returns Ok(()) if successful, or an error if not.
///
/// # Arguments
///
/// * `ctx` - RelayContext reference that holds the configuration
/// * `chain_name` - Name of the chain
/// * `store` -[Sled](https://sled.rs)-based database store
fn start_tx_queue(
    ctx: RelayerContext,
    chain_name: String,
    store: Arc<Store>,
) -> anyhow::Result<()> {
    let mut shutdown_signal = ctx.shutdown_signal();
    let tx_queue = TxQueue::new(ctx, chain_name.clone(), store);

    tracing::debug!("Transaction Queue for ({}) Started.", chain_name);
    let task = async move {
        tokio::select! {
            _ = tx_queue.run() => {
                tracing::warn!(
                    "Transaction Queue task stopped for ({})",
                    chain_name,
                );
            },
            _ = shutdown_signal.recv() => {
                tracing::trace!(
                    "Stopping Transaction Queue for ({})",
                    chain_name,
                );
            },
        }
    };
    // kick off the tx_queue.
    tokio::task::spawn(task);
    Ok(())
}
