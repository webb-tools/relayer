use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use coingecko::CoinGeckoClient;
use ethers::etherscan;
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Provider};
use ethers::signers::LocalWallet;
use ethers::types::{Address, Chain};
use ethers::utils::{parse_ether, parse_units};
use once_cell::sync::Lazy;
use serde::Serialize;
use std::collections::HashMap;
use std::ops::Add;
use std::sync::{Arc, Mutex};
use webb::evm::contract::protocol_solidity::{
    FungibleTokenWrapperContract, OpenVAnchorContract,
};
use webb::evm::ethers::prelude::U256;

/// Maximum refund amount per relay transaction in USD.
const MAX_REFUND_USD: f64 = 1.;
/// Amount of time for which a `FeeInfo` is valid after creation
static FEE_CACHE_TIME: Lazy<Duration> = Lazy::new(|| Duration::minutes(1));
/// Amount of profit that the relay should make with each transaction (in USD).
const TRANSACTION_PROFIT_USD: f64 = 5.;

static COIN_GECKO_CLIENT: Lazy<CoinGeckoClient> =
    Lazy::new(CoinGeckoClient::default);
static ETHERSCAN_CLIENT: Lazy<etherscan::Client> =
    Lazy::new(|| etherscan::Client::new_from_env(Chain::Mainnet).unwrap());
/// Cache for previously generated fee info. Key consists of the VAnchor address and chain id.
/// Entries are valid as long as `timestamp` is no older than `FEE_CACHE_TIME`.
static FEE_INFO_CACHED: Lazy<Mutex<HashMap<(Address, u64), FeeInfo>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Return value of fee_info API call. Contains information about relay transaction fee and refunds.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FeeInfo {
    /// Estimated fee for an average relay transaction, in `wrappedToken`. This is only for
    /// display to the user
    pub estimated_fee: U256,
    /// Price per gas using "normal" confirmation speed, in `nativeToken`
    pub gas_price: U256,
    /// Exchange rate for refund from `wrappedToken` to `nativeToken`
    pub refund_exchange_rate: U256,
    /// Maximum amount of `wrappedToken` which can be exchanged to `nativeToken` by relay
    pub max_refund: U256,
    /// Time when this FeeInfo was generated
    timestamp: DateTime<Utc>,
}

/// Get the current fee info.
///
/// If fee info was recently requested, the cached value is used. Otherwise it is regenerated
/// based on the current exchange rate and estimated gas price.
pub async fn get_fee_info(
    chain_id: u64,
    vanchor: Address,
    estimated_gas_amount: U256,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
) -> crate::Result<FeeInfo> {
    evict_cache();
    // Check if there is an existing fee info. Return it directly if thats the case.
    {
        let lock = FEE_INFO_CACHED.lock().unwrap();
        if let Some(fee_info) = lock.get(&(vanchor, chain_id)) {
            return Ok(fee_info.clone());
        }
    }

    let gas_price = estimate_gas_price().await?;
    let estimated_fee =
        calculate_transaction_fee(gas_price, estimated_gas_amount, chain_id)
            .await?;
    let max_refund = max_refund(vanchor, client).await?;

    let fee_info = FeeInfo {
        estimated_fee,
        gas_price,
        refund_exchange_rate: 0.into(), // TODO
        max_refund,
        timestamp: Utc::now(),
    };
    // Insert newly generated fee info into cache.
    FEE_INFO_CACHED
        .lock()
        .unwrap()
        .insert((vanchor, chain_id), fee_info.clone());
    Ok(fee_info)
}

/// Remove all items from fee_info cache which are older than `FEE_CACHE_TIME`.
fn evict_cache() {
    let mut cache = FEE_INFO_CACHED.lock().unwrap();
    cache.retain(|_, v| {
        let fee_info_valid_time = v.timestamp.add(*FEE_CACHE_TIME);
        fee_info_valid_time > Utc::now()
    });
}

/// Pull USD prices of base token from coingecko.com, and use this to calculate the transaction
/// fee in wei. This fee includes a profit for the relay of `TRANSACTION_PROFIT_USD`.
async fn calculate_transaction_fee(
    gas_price: U256,
    gas_amount: U256,
    chain_id: u64,
) -> crate::Result<U256> {
    let base_token = get_base_token_name(chain_id)?;
    let tokens = &[base_token];
    let prices = COIN_GECKO_CLIENT
        .price(tokens, &["usd"], false, false, false, false)
        .await?;
    let base_token_price = prices[base_token].usd.unwrap();
    let relay_profit = parse_ether(TRANSACTION_PROFIT_USD / base_token_price)?;

    let transaction_fee = gas_price * gas_amount;
    let fee_with_profit = relay_profit + transaction_fee;
    Ok(fee_with_profit)
}

/// Estimate gas price using etherscan.io. Note that this functionality is only available
/// on mainnet.
async fn estimate_gas_price() -> crate::Result<U256> {
    let gas_oracle = ETHERSCAN_CLIENT.gas_oracle().await?;
    // use the "average" gas price
    let gas_price_gwei = U256::from(gas_oracle.propose_gas_price);
    Ok(parse_units(gas_price_gwei, "gwei")?)
}

/// Calculate the maximum refund amount per relay transaction in `wrappedToken`, based on
/// `MAX_REFUND_USD`.
async fn max_refund(
    vanchor: Address,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
) -> crate::Result<U256> {
    let wrapped_token = &get_wrapped_token_name(vanchor, client).await?;
    let prices = COIN_GECKO_CLIENT
        .price(&[wrapped_token], &["usd"], false, false, false, false)
        .await?;
    let wrapped_price = prices[wrapped_token].usd.unwrap();
    let max_refund_wrapped = MAX_REFUND_USD / wrapped_price;

    Ok(to_u256(max_refund_wrapped))
}

/// Convert exchange rates to `wrappedToken` U256.
fn to_u256(amount: f64) -> U256 {
    // TODO: this gives wrong result, test fails with
    //       "revert amount is larger than maximumDepositAmount"
    parse_ether(amount).unwrap()
    /*
    TODO: in case wrappedToken is USDC, need to use this code for conversion
    let multiplier = f64::from(10_i32.pow(USDC_DECIMALS));
    dbg!(&amount, &multiplier);
    let val = amount * multiplier;
    U256::from(val.round() as i128)
     */
}

/// Retrieves the token name of a given anchor contract. Wrapper prefixes are stripped in order
/// to get a token name which coingecko understands.
async fn get_wrapped_token_name(
    vanchor: Address,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
) -> crate::Result<String> {
    let anchor_contract = OpenVAnchorContract::new(vanchor, client.clone());
    let token_address = anchor_contract.token().call().await?;
    let token_contract =
        FungibleTokenWrapperContract::new(token_address, client.clone());
    let token_name = token_contract.name().call().await?;
    // TODO: add all supported tokens
    Ok(match token_name.replace("webb", "").as_str() {
        "WETH" => "ethereum",
        x => x,
    }
    .to_string())
}

/// Hardcodede mapping from chain id to base token name. Testnets use the mainnet name because
/// otherwise there is no exchange rate available.
///
/// https://github.com/DefiLlama/chainlist/blob/main/constants/chainIds.json
fn get_base_token_name(chain_id: u64) -> crate::Result<&'static str> {
    match chain_id {
        1 | 5 | 5001 | 5002 | 5003 | 11155111 => Ok("ethereum"),
        10 | 420 => Ok("optimism"),
        127 | 80001 => Ok("polygon"),
        1284 | 1287 => Ok("moonbeam"),
        _ => {
            // Typescript tests use randomly generated chain id, so we always return "ethereum"
            // in debug mode to make them work.
            if cfg!(debug_assertions) {
                Ok("ethereum")
            } else {
                let chain_id = chain_id.to_string();
                Err(crate::Error::ChainNotFound { chain_id })
            }
        }
    }
}
