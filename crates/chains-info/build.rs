use std::{
    collections::HashMap,
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use quote::ToTokens;

/// The chains information build script.
///
/// This script generates the `chains.rs` file that contains the information about the chains
/// that are supported by the relayer.
fn main() -> anyhow::Result<()> {
    // return early if the `supported_chains.toml` file didn't change
    // since the last build
    println!("cargo:rerun-if-changed=supported_chains.toml");
    // or any of the fixtures changed.
    println!("cargo:rerun-if-changed=fixtures/chains.json");
    println!("cargo:rerun-if-changed=fixtures/coingecko_coins_list.json");

    let v = std::fs::read_to_string("supported_chains.toml")?;
    let supported_chains = toml::from_str(&v)?;
    generate_chains_info(&supported_chains)
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SupportedChains {
    #[serde(rename = "chain-ids")]
    ids: Vec<u64>,
    /// A Map of Chain Identifier to Chain Override.
    #[serde(default)]
    overrides: HashMap<String, ChainOverride>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
struct ChainOverride {
    name: Option<String>,
    short_name: Option<String>,
    native_currency_name: Option<String>,
    native_currency_symbol: Option<String>,
    native_currency_decimals: Option<u8>,
    coingecko_coin_id: Option<String>,
}

fn generate_chains_info(
    supported_chains: &SupportedChains,
) -> anyhow::Result<()> {
    let all_chains = chains::read_all()?;
    let mut chains = all_chains
        .into_iter()
        .filter(|chain| supported_chains.ids.contains(&chain.chain_id))
        .map(|chain| {
            supported_chains
                .overrides
                .get(&chain.chain_id.to_string())
                .cloned()
                .map_or(chain.clone(), |v| chain.overrides_with(v))
        })
        .collect::<Vec<_>>();
    // sort the chains by the chain identifier
    // so that the generated code is deterministic
    chains.sort_by_key(|chain| chain.chain_id);
    let coingecko_coins_list = coingecko::coins_list()?;
    let simple_chains_info =
        bake_simple_chains_info(&chains, &coingecko_coins_list);
    let path = Path::new(&std::env::var("OUT_DIR")?).join("chains.rs");
    let mut file = BufWriter::new(File::create(path)?);

    let generated_code = quote::quote! {
        //! Chains Information
        //!
        //! This file is generated by the `build.rs` script.
        //! Do not edit this file manually.
        //!
        //! This file contains the information about the chains that are supported by the relayer.
        //! The information is used to generate the `chains.rs` file and could be used by
        //! the relayer to get the information about the chains.

        /// The Chain Information.
        pub struct ChainInfo {
            /// Chain Identifier.
            pub chain_id: u64,
            /// Chain Name.
            pub name: &'static str,
            /// Chain Short Name, usually the ticker.
            pub short_name: &'static str,
            /// Chain Native Currency Information.
            pub native_currency: CurrencyInfo,
        }

        /// The Currency Information.
        pub struct CurrencyInfo {
            /// Currency Name.
            pub name: &'static str,
            /// Currency Symbol.
            pub symbol: &'static str,
            /// Currency Decimals.
            pub decimals: u8,
            /// Coingecko's Coin Identifier.
            ///
            /// This is `None` if the chain is not supported by Coingecko.
            pub coingecko_coin_id: Option<&'static str>,
        }

        #simple_chains_info
    };

    let code = quote::quote! {
        mod chains {
            #generated_code
        }
    };

    let syntax_tree = syn::parse_file(&code.to_string())?;
    let formatted = prettyplease::unparse(&syntax_tree);
    file.write_all(formatted.as_bytes())?;
    Ok(())
}

/// Bake the simple chains information.
///
/// This returns a token stream that contains the simple chains information.
fn bake_simple_chains_info(
    chains: &[chains::Chain],
    coingecko_coins_list: &[coingecko::Coin],
) -> proc_macro2::TokenStream {
    let mut tokens = proc_macro2::TokenStream::new();
    for chain in chains {
        let chain_id = chain.chain_id;
        let name = &chain.name;
        let short_name = &chain.short_name;
        let native_currency = &chain.native_currency;
        let native_currency_name = &native_currency.name;
        let native_currency_symbol = &native_currency.symbol;
        let native_currency_decimals = native_currency.decimals;
        let maybe_coingecko_coin_id =
            coingecko_coins_list.iter().find_map(|coin| {
                if coin.symbol.to_lowercase()
                    == native_currency_symbol.to_lowercase()
                {
                    Some(coin.id.clone())
                } else {
                    None
                }
            });
        // if the chain has a coingecko coin id, use it
        // otherwise use the one from the coingecko coins list
        let maybe_coingecko_coin_id =
            chain.coingecko_coin_id.clone().or(maybe_coingecko_coin_id);
        let coingecko_coin_id = if let Some(id) = maybe_coingecko_coin_id {
            quote::quote! { Some(#id) }
        } else {
            quote::quote! { None }
        };
        let token = quote::quote! {
            (
                #chain_id,
                ChainInfo {
                    chain_id: #chain_id,
                    name: #name,
                    short_name: #short_name,
                    native_currency: CurrencyInfo {
                        name: #native_currency_name,
                        symbol: #native_currency_symbol,
                        decimals: #native_currency_decimals,
                        coingecko_coin_id: #coingecko_coin_id,
                    },
                }
            ),
        };
        tokens.extend(token);
    }
    let n = chains.len();
    let result = quote::quote! {
        pub type ChainsInfo = [(u64, ChainInfo); #n];
        /// List of all the supported chains.
        pub const CHAINS_INFO: ChainsInfo = [
            #tokens
        ];
    };
    result.into_token_stream()
}

/// Reads all the chains from `fixtures/chains.json`.
///
/// You can refetch the file from `https://chainid.network/chains.json` endpoint.
mod chains {
    pub type Chains = Vec<Chain>;

    #[derive(Default, Debug, Clone, PartialEq, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Chain {
        pub name: String,
        pub chain: String,
        pub native_currency: NativeCurrency,
        pub short_name: String,
        pub chain_id: u64,
        pub network_id: u64,
        pub title: Option<String>,
        /// Used internally to override the chain's coingecko coin id.
        #[serde(skip)]
        pub coingecko_coin_id: Option<String>,
    }
    impl Chain {
        pub(crate) fn overrides_with(
            self,
            overrides: crate::ChainOverride,
        ) -> Chain {
            if let Some(name) = overrides.name {
                return Chain { name, ..self };
            }

            if let Some(short_name) = overrides.short_name {
                return Chain { short_name, ..self };
            }

            if let Some(native_currency_name) = overrides.native_currency_name {
                return Chain {
                    native_currency: NativeCurrency {
                        name: native_currency_name,
                        ..self.native_currency
                    },
                    ..self
                };
            }

            if let Some(native_currency_symbol) =
                overrides.native_currency_symbol
            {
                return Chain {
                    native_currency: NativeCurrency {
                        symbol: native_currency_symbol,
                        ..self.native_currency
                    },
                    ..self
                };
            }

            if let Some(native_currency_decimals) =
                overrides.native_currency_decimals
            {
                return Chain {
                    native_currency: NativeCurrency {
                        decimals: native_currency_decimals,
                        ..self.native_currency
                    },
                    ..self
                };
            }
            if let Some(coingecko_coin_id) = overrides.coingecko_coin_id {
                return Chain {
                    coingecko_coin_id: Some(coingecko_coin_id),
                    ..self
                };
            }
            self
        }
    }

    #[derive(Default, Debug, Clone, PartialEq, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct NativeCurrency {
        pub name: String,
        pub symbol: String,
        pub decimals: u8,
    }

    pub fn read_all() -> anyhow::Result<Chains> {
        let file = std::fs::File::open("fixtures/chains.json")?;
        let reader = std::io::BufReader::new(file);
        let chains: Chains = serde_json::from_reader(reader)?;
        Ok(chains)
    }
}

/// Reads the list of all the coins supported by coingecko
/// from `fixtures/coingecko_coins_list.json`.
///
///
/// You can refetch the file from the `https://api.coingecko.com/api/v3/coins/list` endpoint.
mod coingecko {
    pub type Coins = Vec<Coin>;

    #[derive(Default, Debug, Clone, PartialEq, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Coin {
        pub id: String,
        pub symbol: String,
        pub name: String,
    }

    pub fn coins_list() -> anyhow::Result<Coins> {
        let file = std::fs::File::open("fixtures/coingecko_coins_list.json")?;
        let reader = std::io::BufReader::new(file);
        let coins: Coins = serde_json::from_reader(reader)?;
        Ok(coins)
    }
}
