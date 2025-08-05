pub mod bitcoin_wallet;
mod client;
mod config;
pub mod evm_wallet;
mod handlers;
mod rfq_client;
mod rfq_handlers;
mod strategy;
pub mod wallet;

use std::sync::Arc;

use bdk_wallet::bitcoin;
use clap::Parser;
use common::{create_websocket_wallet_provider, handle_background_thread_result};
use config::Config;
use otc_models::ChainType;
use snafu::{prelude::*, ResultExt};
use tokio::task::JoinSet;
use tracing::info;

use crate::{bitcoin_wallet::BitcoinWallet, evm_wallet::EVMWallet, wallet::WalletManager};

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Configuration error: {}", source))]
    Config { source: config::ConfigError },

    #[snafu(display("Client error: {}", source))]
    Client { source: client::ClientError },

    #[snafu(display("Bitcoin wallet error: {}", source))]
    BitcoinWallet {
        source: bitcoin_wallet::BitcoinWalletError,
    },

    #[snafu(display("EVM wallet error: {}", source))]
    GenericWallet { source: wallet::WalletError },

    #[snafu(display("Provider error: {}", source))]
    Provider { source: common::ProviderError },

    #[snafu(display("Background thread error: {}", source))]
    BackgroundThread {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl From<common::ProviderError> for Error {
    fn from(error: common::ProviderError) -> Self {
        Error::Provider { source: error }
    }
}

impl From<wallet::WalletError> for Error {
    fn from(error: wallet::WalletError) -> Self {
        Error::GenericWallet { source: error }
    }
}

impl From<client::ClientError> for Error {
    fn from(error: client::ClientError) -> Self {
        Error::Client { source: error }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Parser, Debug)]
#[command(name = "market-maker")]
#[command(about = "Market Maker client for TEE-OTC")]
pub struct MarketMakerArgs {
    /// Market maker identifier
    #[arg(long, env = "MM_ID")]
    pub market_maker_id: String,

    /// API key ID (UUID) for authentication
    #[arg(long, env = "MM_API_KEY_ID")]
    pub api_key_id: String,

    /// API key for authentication
    #[arg(long, env = "MM_API_KEY")]
    pub api_key: String,

    /// OTC server WebSocket URL
    #[arg(long, env = "OTC_WS_URL", default_value = "ws://localhost:3000/ws/mm")]
    pub otc_ws_url: String,

    /// RFQ server WebSocket URL
    #[arg(long, env = "RFQ_WS_URL", default_value = "ws://localhost:3001/ws/mm")]
    pub rfq_ws_url: String,

    /// Bitcoin wallet database file
    #[arg(long, env = "BITCOIN_WALLET_DB_PATH")]
    pub bitcoin_wallet_db_file: String,

    /// Bitcoin wallet descriptor (aka private key in descriptor format)
    #[arg(long, env = "BITCOIN_WALLET_DESCRIPTOR")]
    pub bitcoin_wallet_descriptor: String,

    /// Bitcoin wallet network
    #[arg(long, env = "BITCOIN_WALLET_NETWORK", default_value = "bitcoin")]
    pub bitcoin_wallet_network: bitcoin::Network,

    /// Bitcoin Esplora URL
    #[arg(long, env = "BITCOIN_WALLET_ESPLORA_URL")]
    pub bitcoin_wallet_esplora_url: String,

    /// Ethereum wallet private key
    #[arg(long, env = "ETHEREUM_WALLET_PRIVATE_KEY", value_parser = parse_hex_string)]
    pub ethereum_wallet_private_key: [u8; 32],

    /// Ethereum confirmations necessary for a transaction to be considered confirmed (for the wallet to be allowed to send a new transaction)
    #[arg(long, env = "ETHEREUM_CONFIRMATIONS", default_value = "1")]
    pub ethereum_confirmations: u64,

    /// Ethereum RPC URL
    #[arg(long, env = "ETHEREUM_RPC_WS_URL")]
    pub ethereum_rpc_ws_url: String,

    /// Auto-accept all quotes (for testing)
    #[arg(long, env = "AUTO_ACCEPT", default_value = "false")]
    pub auto_accept: bool,

    /// Log level
    #[arg(long, env = "RUST_LOG", default_value = "info")]
    pub log_level: String,
}

fn parse_hex_string(s: &str) -> std::result::Result<[u8; 32], String> {
    let bytes = alloy::hex::decode(s).map_err(|e| e.to_string())?;
    if bytes.len() != 32 {
        return Err(format!("Expected 32 bytes, got {}", bytes.len()).into());
    }
    Ok(bytes.try_into().unwrap())
}

pub async fn run_market_maker(args: MarketMakerArgs) -> Result<()> {
    let mut join_set: JoinSet<Result<()>> = JoinSet::new();

    info!("Starting market maker with ID: {}", args.market_maker_id);

    let mut wallet_manager = WalletManager::new();
    wallet_manager.register(
        ChainType::Bitcoin,
        Arc::new(
            BitcoinWallet::new(
                &args.bitcoin_wallet_db_file,
                &args.bitcoin_wallet_descriptor,
                args.bitcoin_wallet_network,
                &args.bitcoin_wallet_esplora_url,
                &mut join_set,
            )
            .await
            .context(BitcoinWalletSnafu)?,
        ),
    );

    let provider = create_websocket_wallet_provider(
        &args.ethereum_rpc_ws_url,
        args.ethereum_wallet_private_key,
    )
    .await?;

    wallet_manager.register(
        ChainType::Ethereum,
        Arc::new(EVMWallet::new(
            Arc::new(provider),
            args.ethereum_rpc_ws_url,
            args.ethereum_confirmations,
            &mut join_set,
        )),
    );

    let otc_fill_client = client::OtcFillClient::new(
        Config {
            market_maker_id: args.market_maker_id.clone(),
            api_key_id: args.api_key_id.clone(),
            api_key: args.api_key.clone(),
            otc_ws_url: args.otc_ws_url.clone(),
            auto_accept: args.auto_accept,
            reconnect_interval_secs: 5,
            max_reconnect_attempts: 5,
        },
        wallet_manager,
    );
    join_set.spawn(async move { otc_fill_client.run().await.map_err(Error::from) });
    
    // Add RFQ client for handling quote requests
    let rfq_client = rfq_client::RfqClient::new(
        Config {
            market_maker_id: args.market_maker_id,
            api_key_id: args.api_key_id,
            api_key: args.api_key,
            otc_ws_url: args.otc_ws_url,
            auto_accept: args.auto_accept,
            reconnect_interval_secs: 5,
            max_reconnect_attempts: 5,
        },
        args.rfq_ws_url,
    );
    join_set.spawn(async move { 
        rfq_client.run().await.map_err(|e| Error::Client { 
            source: client::ClientError::BackgroundThreadExited { 
                source: Box::new(e) 
            } 
        }) 
    });

    handle_background_thread_result(join_set.join_next().await).context(BackgroundThreadSnafu)?;

    Ok(())
}
