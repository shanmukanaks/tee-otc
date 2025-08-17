use std::time::Duration;

use crate::{
    bitcoin_wallet::BitcoinWallet, evm_wallet::EVMWallet, price_oracle::BitcoinEtherPriceOracle,
};
use alloy::eips::BlockNumberOrTag;
use alloy::providers::DynProvider;
use alloy::{primitives::U256, providers::Provider};
use chrono::Utc;
use otc_models::{supported_currencies, ChainType, Lot, Quote, QuoteMode, QuoteRequest};
use snafu::Snafu;
use tracing::{debug, info};
use uuid::Uuid;

const QUOTE_EXPIRATION_TIME: Duration = Duration::from_secs(60 * 5);

#[derive(Debug, Snafu)]
pub enum WrappedBitcoinQuoterError {
    #[snafu(display("Failed to get fee rate from esplora: {}", source))]
    Esplora { source: esplora_client::Error },
}

impl From<esplora_client::Error> for WrappedBitcoinQuoterError {
    fn from(error: esplora_client::Error) -> Self {
        WrappedBitcoinQuoterError::Esplora { source: error }
    }
}

type Result<T, E = WrappedBitcoinQuoterError> = std::result::Result<T, E>;

pub struct WrappedBitcoinQuoter {
    btc_eth_price_oracle: BitcoinEtherPriceOracle,
    esplora_client: esplora_client::AsyncClient,
    eth_provider: DynProvider,
    trade_spread_bps: u64,
    fee_safety_multiplier: f64,
}

impl WrappedBitcoinQuoter {
    pub fn new(
        btc_eth_price_oracle: BitcoinEtherPriceOracle,
        esplora_client: esplora_client::AsyncClient,
        eth_provider: DynProvider,
        trade_spread_bps: u64,
        fee_safety_multiplier: f64,
    ) -> Self {
        Self {
            btc_eth_price_oracle,
            esplora_client,
            eth_provider,
            trade_spread_bps,
            fee_safety_multiplier,
        }
    }

    /// Compute a quote for the given amount and quote mode.
    /// Note that fill_chain is the chain that the market maker will fill the quote on.
    /// which is relevant for computing fees
    pub async fn compute_quote(
        &self,
        market_maker_id: Uuid,
        quote_request: &QuoteRequest,
    ) -> Result<Option<Quote>> {
        if !is_fillable_request(quote_request) {
            info!("Unfillable quote request: {:?}", quote_request);
            return Ok(None);
        }
        if quote_request.amount > U256::from(u64::MAX) {
            info!("Invalid amount: {:?}", quote_request);
            return Ok(None);
        }
        let amount = quote_request.amount.to::<u64>();
        let send_fees_in_sats = {
            match quote_request.to.chain {
                ChainType::Bitcoin => {
                    //TODO: put updating this fee rate behind a RwLock that we cache so it's not fetched on every quote
                    let sats_per_vbyte_by_confirmations =
                        self.esplora_client.get_fee_estimates().await?;
                    let sats_per_vbyte = sats_per_vbyte_by_confirmations.get(&1).unwrap_or(&1.5);
                    let sats_per_vbyte = sats_per_vbyte * self.fee_safety_multiplier;

                    calculate_fees_in_sats_to_send_btc(sats_per_vbyte)
                }
                ChainType::Ethereum => {
                    //TODO: put updating this fee rate behind a RwLock that we cache so it's not fetched on every quote
                    let fee_history = match self
                        .eth_provider
                        .get_fee_history(10u64, BlockNumberOrTag::Latest, &[25.0, 50.0, 75.0])
                        .await
                    {
                        Ok(history) => history,
                        Err(_) => {
                            info!("Failed to get fee history: {:?}", quote_request);
                            return Ok(None);
                        }
                    };

                    let base_fee_wei: u128 = fee_history.next_block_base_fee().unwrap_or(0u128);
                    let base_fee_gwei: f64 = (base_fee_wei as f64) / 1e9f64;

                    let mid_priority_wei: u128 = fee_history
                        .reward
                        .as_ref()
                        .and_then(|rewards| rewards.last())
                        .and_then(|percentiles| percentiles.get(1)) // 50th percentile
                        .copied()
                        .unwrap_or(1_500_000_000u128); // default 1.5 gwei
                    let mut max_priority_fee_gwei: f64 = (mid_priority_wei as f64) / 1e9f64;

                    max_priority_fee_gwei *= self.fee_safety_multiplier;

                    let eth_per_btc_price = match self.btc_eth_price_oracle.get_eth_per_btc().await
                    {
                        Ok(p) => p,
                        Err(e) => {
                            info!("Failed to get BTC/ETH price: {:?}", e);
                            return Ok(None);
                        }
                    };

                    calculate_fees_in_sats_to_send_cbbtc_on_eth(
                        base_fee_gwei,
                        max_priority_fee_gwei,
                        eth_per_btc_price,
                    )
                }
            }
        };

        let quote_id = Uuid::new_v4();
        match quote_request.mode {
            QuoteMode::ExactInput => {
                let rx_btc = tx_btc_to_rx_btc(amount, send_fees_in_sats, self.trade_spread_bps);
                if rx_btc.is_none() {
                    info!("Rx BTC is none: {:?}", quote_request);
                    return Ok(None);
                }
                let rx_btc = rx_btc.unwrap();

                Ok(Some(Quote {
                    id: quote_id,
                    market_maker_id,
                    from: Lot {
                        currency: quote_request.from.clone(),
                        amount: quote_request.amount,
                    },
                    to: Lot {
                        currency: quote_request.to.clone(),
                        amount: U256::from(rx_btc),
                    },
                    expires_at: Utc::now() + QUOTE_EXPIRATION_TIME,
                    created_at: Utc::now(),
                }))
            }
            QuoteMode::ExactOutput => {
                let tx_btc = rx_btc_to_tx_btc(amount, send_fees_in_sats, self.trade_spread_bps);
                if tx_btc.is_none() {
                    info!("Tx BTC is none: {:?}", quote_request);
                    return Ok(None);
                }
                let tx_btc = tx_btc.unwrap();

                Ok(Some(Quote {
                    id: quote_id,
                    market_maker_id,
                    from: Lot {
                        currency: quote_request.from.clone(),
                        amount: U256::from(tx_btc),
                    },
                    to: Lot {
                        currency: quote_request.to.clone(),
                        amount: quote_request.amount,
                    },
                    expires_at: Utc::now() + QUOTE_EXPIRATION_TIME,
                    created_at: Utc::now(),
                }))
            }
        }
    }
}

fn is_fillable_request(quote_request: &QuoteRequest) -> bool {
    if !((quote_request.from.chain == ChainType::Bitcoin
        && quote_request.to.chain == ChainType::Ethereum)
        || (quote_request.to.chain == ChainType::Bitcoin
            && quote_request.from.chain == ChainType::Ethereum))
    {
        info!("Invalid chain selection: {:?}", quote_request);
        // invalid chain selection
        return false;
    }
    match supported_currencies::SUPPORTED_TOKENS_BY_CHAIN.get(&quote_request.from.chain) {
        Some(supported_tokens) => {
            if !supported_tokens.contains(&quote_request.from.token) {
                info!("Invalid token selection: {:?}", quote_request);
                return false;
            }
        }
        None => return false,
    }

    match supported_currencies::SUPPORTED_TOKENS_BY_CHAIN.get(&quote_request.to.chain) {
        Some(supported_tokens) => {
            if !supported_tokens.contains(&quote_request.to.token) {
                info!("Invalid token selection: {:?}", quote_request);
                return false;
            }
        }
        None => return false,
    }

    true
}

/// ExactInput: User sends exactly `tx` sats, receives less after fees and spread
/// rx = floor( tx * (10_000 - s) / 10_000 ) - fee
/// Returns None if the computed rx would be negative or on overflow.
fn tx_btc_to_rx_btc(sent_sats: u64, fee_sats: u64, trade_spread_bps: u64) -> Option<u64> {
    const BPS_DENOM: u128 = 10_000;

    let tx = sent_sats as u128;
    let fee = fee_sats as u128;
    let s = trade_spread_bps as u128;

    // Check spread is not >= 100%
    if s >= BPS_DENOM {
        return None;
    }

    // Floor division: deduct spread from amount
    let rx_before_fee = tx.saturating_mul(BPS_DENOM - s) / BPS_DENOM;
    
    // Deduct fees
    let rx = rx_before_fee.checked_sub(fee)?;

    let rx_u64 = u64::try_from(rx).ok()?;
    Some(rx_u64)
}

/// ExactOutput: User wants to receive exactly `rx` sats, must send more to cover fees and spread
/// tx = ceil( 10_000 * (rx + fee) / (10_000 - s) )
/// Returns None if spread >= 100%, or on overflow.
fn rx_btc_to_tx_btc(received_sats: u64, fee_sats: u64, trade_spread_bps: u64) -> Option<u64> {
    const BPS_DENOM: u128 = 10_000;
    
    let rx = received_sats as u128;
    let fee = fee_sats as u128;
    let s = trade_spread_bps as u128;

    // Check spread is not >= 100%
    if s >= BPS_DENOM {
        return None;
    }

    let rx_plus_fee = rx.saturating_add(fee);
    let num = BPS_DENOM.saturating_mul(rx_plus_fee);
    let denom = BPS_DENOM - s;

    // Ceil division: (num + denom - 1) / denom
    let tx = (num + denom - 1) / denom;
    let tx_u64 = u64::try_from(tx).ok()?;
    Some(tx_u64)
}

// TODO: This should be computed by the wallet
fn calculate_fees_in_sats_to_send_btc(sats_per_vbyte: f64) -> u64 {
    let bytes = 167.25;
    let fee = sats_per_vbyte * bytes;
    fee.ceil() as u64
}

// TODO: This should be computed by the wallet
fn calculate_fees_in_sats_to_send_cbbtc_on_eth(
    base_fee_gwei: f64,
    max_priority_fee_gwei: f64,
    eth_per_btc_price: f64,
) -> u64 {
    let transfer_gas_limit = 65_626f64;
    let gas_cost_gwei = transfer_gas_limit * (max_priority_fee_gwei + base_fee_gwei);
    let gas_cost_wei = U256::from(gas_cost_gwei.ceil() as u64) * U256::from(1e9);
    let wei_per_sat = U256::from((eth_per_btc_price * 1e10).round() as u128);
    (gas_cost_wei / wei_per_sat).to::<u64>()
}
