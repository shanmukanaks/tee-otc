//! Integration tests package lib
#![allow(dead_code)]

mod utils;

#[cfg(test)]
mod market_maker_otc_auth_test;

#[cfg(test)]
mod simple_swap_test;

#[cfg(test)]
mod indexer_client_test;

#[cfg(test)]
mod evm_wallet_test;

#[cfg(test)]
mod bitcoin_wallet_test;

#[cfg(test)]
mod rfq_flow_test;
