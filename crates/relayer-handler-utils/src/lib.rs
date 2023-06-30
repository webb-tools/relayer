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

#![allow(clippy::large_enum_variant)]
#![allow(missing_docs)]

use serde::{Deserialize, Deserializer, Serialize};
use tokio::sync::mpsc;
use webb::evm::ethers::abi::Address;
use webb::evm::ethers::prelude::{ContractError, I256, U128};
use webb::evm::ethers::providers::Middleware;
use webb::evm::ethers::types::Bytes;
use webb::evm::ethers::types::{H256, H512, U256};
use webb_relayer_store::queue::QueueItemState;
use webb_relayer_tx_relay_utils::VAnchorRelayTransaction;

/// Representation for IP address response
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpInformationResponse {
    pub ip: String,
}

/// A wrapper type around [`I256`] that implements a correct way for [`Serialize`] and [`Deserialize`].
///
/// This supports the signed integer hex values that are not originally supported by the [`I256`] type.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct WebbI256(pub I256);

impl<'de> Deserialize<'de> for WebbI256 {
    fn deserialize<D>(deserializer: D) -> Result<WebbI256, D::Error>
    where
        D: Deserializer<'de>,
    {
        let i128_str = String::deserialize(deserializer)?;
        let i128_val =
            I256::from_hex_str(&i128_str).map_err(serde::de::Error::custom)?;
        Ok(WebbI256(i128_val))
    }
}
/// A wrapper type around [`i128`] that implements a correct way for [`Serialize`] and [`Deserialize`].
///
/// This supports the signed integer hex values that are not originally supported by the [`i128`] type.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct WebbI128(pub i128);

impl<'de> Deserialize<'de> for WebbI128 {
    fn deserialize<D>(deserializer: D) -> Result<WebbI128, D::Error>
    where
        D: Deserializer<'de>,
    {
        let i128_str = String::deserialize(deserializer)?;
        let value = i128::from_str_radix(&i128_str, 16)
            .map_err(serde::de::Error::custom)?;
        Ok(WebbI128(value))
    }
}

/// Type of Command to use
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Command {
    /// Substrate specific subcommand.
    Substrate(SubstrateCommandType),
    /// EVM specific subcommand.
    Evm(EvmCommandType),
    /// Ping?
    Ping(),
}

/// Enumerates the supported evm commands for relaying transactions
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EvmCommandType {
    /// Webb Variable Anchors.
    VAnchor(EvmVanchorCommand),
}

/// Enumerates the supported substrate commands for relaying transactions
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubstrateCommandType {
    /// Webb Variable Anchors.
    VAnchor(SubstrateVAchorCommand),
}

/// Vanchor tx relaying errors.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TransactionRelayingError {
    /// Unsupported chain
    UnsupportedChain,
    /// Unsupported contract address
    UnsupportedContract,
    /// Invalid relayer address
    InvalidRelayerAddress,
    /// Invalid Merkle root
    InvalidMerkleRoots,
    /// Invalid refund amount
    InvalidRefundAmount(String),
    /// Error while wrapping fee
    WrappingFeeError(String),
    /// Transaction queue error
    TransactionQueueError(String),
    /// Network Error
    NetworkConfigurationError(String),
    /// Client Error
    ClientError(String),
}

/// Enumerates the command responses
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandResponse {
    /// Pong?
    Pong(),
    /// Network Status
    Network(NetworkStatus),
    /// Withdrawal Status
    Withdraw(WithdrawStatus),
    /// An error occurred
    Error(String),
    /// Tx status
    TxStatus {
        /// The transaction item key.
        #[serde(rename = "itemKey")]
        item_key: H512,
        status: QueueItemState,
    },
}
/// Enumerates the network status response of the relayer
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkStatus {
    /// Relayer is connecting to the network.
    Connecting,
    /// Relayer is connected to the network.
    Connected,
    /// Network failure with error message.
    Failed {
        /// Error message
        reason: String,
    },
    /// Relayer is disconnected from the network.
    Disconnected,
    /// This contract is not supported by the relayer.
    UnsupportedContract,
    /// This network (chain) is not supported by the relayer.
    UnsupportedChain,
    /// Invalid Relayer address in the proof
    InvalidRelayerAddress,
}
/// Enumerates the withdraw status response of the relayer
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WithdrawStatus {
    /// The transaction is sent to the network.
    Sent,
    /// The transaction is submitted to the network.
    Submitted {
        /// The transaction hash.
        #[serde(rename = "txHash")]
        tx_hash: H256,
    },
    /// The transaction is in the block.
    Finalized {
        /// The transaction hash.
        #[serde(rename = "txHash")]
        tx_hash: H256,
    },
    /// Valid transaction.
    Valid,
    /// Invalid Merkle roots.
    InvalidMerkleRoots,
    /// Transaction dropped from mempool, send it again.
    DroppedFromMemPool,
    /// Invalid transaction.
    Errored {
        /// Error Code.
        code: i32,
        /// Error Message.
        reason: String,
    },
}

/// Type alias for mpsc::Sender<CommandResponse>
pub type CommandStream = mpsc::Sender<CommandResponse>;
/// The command type for EVM vanchor transactions
pub type EvmVanchorCommand = VAnchorRelayTransaction<
    Address,  // Contract address
    Bytes,    // Proof bytes
    Bytes,    // Roots format
    H256,     // Element type
    Address,  // Account identifier
    U256,     // Balance type
    WebbI256, // Signed amount type
    Address,  // Token Address
>;

type Id = u32; //  Substrate tree Id
type P = Bytes; // Substrate raw proof bytes
type R = Vec<H256>; // Substrate roots format
type E = H256; // Substrate element type
type I = H256; // Substrate account identifier (32 bytes)
type B = U128; // Substrate balance type
type A = WebbI128; // Substrate signed amount type
type T = u32; // Substrate assetId

/// The command type for Substrate vanchor txes
pub type SubstrateVAchorCommand =
    VAnchorRelayTransaction<Id, P, R, E, I, B, A, T>;

/// A helper function to extract the error code and the reason from EVM errors.
pub fn into_withdraw_error<M: Middleware>(
    e: ContractError<M>,
) -> WithdrawStatus {
    // a poor man error parser
    // WARNING: **don't try this at home**.
    let msg = format!("{e}");
    // split the error into words, lazily.
    let mut words = msg.split_whitespace();
    let mut reason = "unknown".to_string();
    let mut code = -1;

    while let Some(current_word) = words.next() {
        if current_word == "(code:" {
            code = match words.next() {
                Some(val) => {
                    let mut v = val.to_string();
                    v.pop(); // remove ","
                    v.parse().unwrap_or(-1)
                }
                _ => -1, // unknown code
            };
        } else if current_word == "message:" {
            // next we need to collect all words in between "message:"
            // and "data:", that would be the error message.
            let msg: Vec<_> =
                words.clone().take_while(|v| *v != "data:").collect();
            reason = msg.join(" ");
            reason.pop(); // remove the "," at the end.
        }
    }

    WithdrawStatus::Errored { reason, code }
}
