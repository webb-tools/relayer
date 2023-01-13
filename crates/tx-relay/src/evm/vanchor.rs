use super::*;
use crate::evm::handle_evm_tx;
use ethereum_types::U256;
use std::{collections::HashMap, sync::Arc};
use webb::evm::{
    contract::protocol_solidity::{
        variable_anchor::{CommonExtData, Encryptions, PublicInputs},
        VAnchorContract,
    },
    ethers::prelude::{Signer, SignerMiddleware},
};
use webb_proposals::{ResourceId, TargetSystem, TypedChainId};
use webb_relayer_context::RelayerContext;
use webb_relayer_handler_utils::{CommandStream, EvmCommand, NetworkStatus};
use webb_relayer_utils::fees::{
    calculate_exchange_rate, calculate_wrapped_fee, max_refund,
};
use webb_relayer_utils::metric::Metrics;

/// Handler for VAnchor commands
///
/// # Arguments
///
/// * `ctx` - RelayContext reference that holds the configuration
/// * `cmd` - The command to execute
/// * `stream` - The stream to write the response to
pub async fn handle_vanchor_relay_tx<'a>(
    ctx: RelayerContext,
    cmd: EvmCommand,
    stream: CommandStream,
) {
    use CommandResponse::*;
    let cmd = match cmd {
        EvmCommand::VAnchor(cmd) => cmd,
        _ => return,
    };

    let requested_chain = cmd.chain_id;
    let chain = match ctx.config.evm.get(&requested_chain.to_string()) {
        Some(v) => v,
        None => {
            tracing::warn!("Unsupported Chain: {}", requested_chain);
            let _ = stream.send(Network(NetworkStatus::UnsupportedChain)).await;
            return;
        }
    };
    let supported_contracts: HashMap<_, _> = chain
        .contracts
        .iter()
        .cloned()
        .filter_map(|c| match c {
            webb_relayer_config::evm::Contract::VAnchor(c) => Some(c),
            _ => None,
        })
        .map(|c| (c.common.address, c))
        .collect();
    // get the contract configuration
    let contract_config = match supported_contracts.get(&cmd.id) {
        Some(config) => config,
        None => {
            tracing::warn!("Unsupported Contract: {:?}", cmd.id);
            let _ = stream
                .send(Network(NetworkStatus::UnsupportedContract))
                .await;
            return;
        }
    };

    let wallet = match ctx.evm_wallet(&cmd.chain_id.to_string()).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Misconfigured Network: {}", e);
            let _ = stream
                .send(Error(format!(
                    "Misconfigured Network: {:?}",
                    cmd.chain_id
                )))
                .await;
            return;
        }
    };
    // validate the relayer address first before trying
    // send the transaction.
    let reward_address = match chain.beneficiary {
        Some(account) => account,
        None => wallet.address(),
    };

    if cmd.ext_data.relayer != reward_address {
        let _ = stream
            .send(Network(NetworkStatus::InvalidRelayerAddress))
            .await;
        return;
    }

    // validate that the roots are multiple of 32s
    let roots = cmd.proof_data.roots.to_vec();
    if roots.len() % 32 != 0 {
        let _ = stream
            .send(Withdraw(WithdrawStatus::InvalidMerkleRoots))
            .await;
        return;
    }

    // validate refund amount
    let exchange_rate = calculate_exchange_rate("usd-coin", "ethereum").await;
    let max_refund = max_refund("usd-coin").await;
    // TODO: This can fail unexpectedly if the exchange rate changes.
    if cmd.ext_data.refund > max_refund {
        tracing::error!(
            "User requested a refund which is higher than the maximum"
        );
        let msg = format!("User requested a refund which is higher than the maximum of {max_refund}"        );
        let _ = stream.send(Error(msg)).await;
        return;
    }

    tracing::debug!(
        "Connecting to chain {:?} .. at {}",
        cmd.chain_id,
        chain.http_endpoint
    );
    let _ = stream.send(Network(NetworkStatus::Connecting)).await;
    let provider = match ctx.evm_provider(&cmd.chain_id.to_string()).await {
        Ok(value) => {
            let _ = stream.send(Network(NetworkStatus::Connected)).await;
            value
        }
        Err(e) => {
            let reason = e.to_string();
            let _ =
                stream.send(Network(NetworkStatus::Failed { reason })).await;
            let _ = stream.send(Network(NetworkStatus::Disconnected)).await;
            return;
        }
    };

    let client = SignerMiddleware::new(provider, wallet);
    let client = Arc::new(client);
    let contract = VAnchorContract::new(cmd.id, client.clone());

    let common_ext_data = CommonExtData {
        recipient: cmd.ext_data.recipient,
        ext_amount: cmd.ext_data.ext_amount.0,
        relayer: cmd.ext_data.relayer,
        fee: cmd.ext_data.fee,
        refund: cmd.ext_data.refund,
        token: cmd.ext_data.token,
    };
    let public_inputs = PublicInputs {
        roots: roots.into(),
        extension_roots: cmd.proof_data.extension_roots,
        input_nullifiers: cmd
            .proof_data
            .input_nullifiers
            .iter()
            .map(|v| v.to_fixed_bytes().into())
            .collect(),
        output_commitments: [
            cmd.proof_data.output_commitments[0].to_fixed_bytes().into(),
            cmd.proof_data.output_commitments[1].to_fixed_bytes().into(),
        ],
        public_amount: U256::from_big_endian(
            &cmd.proof_data.public_amount.to_fixed_bytes(),
        ),
        ext_data_hash: cmd.proof_data.ext_data_hash.to_fixed_bytes().into(),
    };

    let encryptions = Encryptions {
        encrypted_output_1: cmd.ext_data.encrypted_output1,
        encrypted_output_2: cmd.ext_data.encrypted_output2,
    };

    tracing::trace!(?cmd.proof_data.proof, ?common_ext_data, "Client Proof");

    let call = contract.transact(
        cmd.proof_data.proof,
        [0u8; 32].into(),
        common_ext_data,
        public_inputs,
        encryptions,
    );

    // check the fee
    let gas_estimate = client.estimate_gas(&call.tx).await.unwrap();
    let expected_fee_wrapped =
        calculate_wrapped_fee(gas_estimate, exchange_rate).await;

    // TODO: Just like refund, this check can fail unexpectedly if exchange rate or gas price change.
    //       Its probably better to lock in the price for each user at the time fee_info is called,
    //       and use the exact same exchange rate/transaction fee here.
    //       This could be done based on IP address, but its fragile because multiple users can have
    //       the same IP. Better to return a token with `FeeInfo` which then gets passed to
    //       `handle_vanchor_relay_tx()` in order to use that preagreed fee (for a limited time).
    if cmd.ext_data.fee < expected_fee_wrapped {
        tracing::error!("Received a fee lower than configuration");
        let msg = format!(
            "User sent a fee that is too low {} but expected {expected_fee_wrapped}",
            cmd.ext_data.fee,
        );
        let _ = stream.send(Error(msg)).await;
        return;
    }

    let target_system = TargetSystem::new_contract_address(
        contract_config.common.address.to_fixed_bytes(),
    );
    let typed_chain_id = TypedChainId::Evm(chain.chain_id);
    let resource_id = ResourceId::new(target_system, typed_chain_id);

    tracing::trace!("About to send Tx to {:?} Chain", cmd.chain_id);
    handle_evm_tx(call, stream, cmd.chain_id, ctx.metrics.clone(), resource_id)
        .await;

    // update metric
    let metrics_clone = ctx.metrics.clone();
    let mut metrics = metrics_clone.lock().await;
    // update metric for total fee earned by relayer on particular resource
    let resource_metric = metrics
        .resource_metric_map
        .entry(resource_id)
        .or_insert_with(|| Metrics::register_resource_id_counters(resource_id));
    resource_metric
        .total_fee_earned
        .inc_by(cmd.ext_data.fee.as_u64() as f64);

    // update metric for total fee earned by relayer
    metrics
        .total_fee_earned
        .inc_by(cmd.ext_data.fee.as_u64() as f64);
}
