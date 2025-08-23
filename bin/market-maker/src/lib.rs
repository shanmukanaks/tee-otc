pub mod bitcoin_wallet;
mod config;
pub mod evm_wallet;
mod otc_client;
mod otc_handler;
pub mod price_oracle;
pub mod quote_storage;
mod rfq_client;
mod rfq_handler;
mod strategy;
pub mod wallet;
mod wrapped_bitcoin_quoter;

use std::{str::FromStr, sync::Arc};

use alloy::{primitives::Address, providers::Provider};
use bdk_wallet::bitcoin;
use clap::Parser;
use common::{create_websocket_wallet_provider, handle_background_thread_result};
use config::Config;
use otc_models::ChainType;
use snafu::{prelude::*, ResultExt};
use tokio::task::JoinSet;
use tracing::info;
use uuid::Uuid;

use crate::{
    bitcoin_wallet::BitcoinWallet, evm_wallet::EVMWallet, quote_storage::QuoteStorage,
    wallet::WalletManager, wrapped_bitcoin_quoter::WrappedBitcoinQuoter,
};

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Configuration error: {}", source))]
    Config { source: config::ConfigError },

    #[snafu(display("Client error: {}", source))]
    Client { source: otc_client::ClientError },

    #[snafu(display("Bitcoin wallet error: {}", source))]
    BitcoinWallet {
        source: bitcoin_wallet::BitcoinWalletError,
    },

    #[snafu(display("EVM wallet error: {}", source))]
    GenericWallet { source: wallet::WalletError },

    #[snafu(display("Provider error: {}", source))]
    Provider { source: common::ProviderError },

    #[snafu(display("Esplora client error: {}", source))]
    EsploraInitialization { source: esplora_client::Error },

    #[snafu(display("Background thread error: {}", source))]
    BackgroundThread {
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[snafu(display("Quote storage error: {}", source))]
    QuoteStorage {
        source: quote_storage::QuoteStorageError,
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

impl From<otc_client::ClientError> for Error {
    fn from(error: otc_client::ClientError) -> Self {
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

    /// Trade spread in basis points
    #[arg(long, env = "TRADE_SPREAD_BPS", default_value = "0")]
    pub trade_spread_bps: u64,

    /// Fee safety multiplier, by default 1.5x
    #[arg(long, env = "FEE_SAFETY_MULTIPLIER", default_value = "1.5")]
    pub fee_safety_multiplier: f64,

    /// Log level
    #[arg(long, env = "RUST_LOG", default_value = "info")]
    pub log_level: String,

    /// Database URL for quote storage
    #[arg(long, env = "MM_DATABASE_URL")]
    pub database_url: String,
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
    let market_maker_id = Uuid::parse_str(&args.market_maker_id).map_err(|e| Error::Config {
        source: config::ConfigError::InvalidUuid {
            uuid: args.market_maker_id,
            error: e,
        },
    })?;

    info!("Starting market maker with ID: {}", market_maker_id);

    // Initialize quote storage
    let quote_storage = Arc::new(
        QuoteStorage::new(&args.database_url, &mut join_set)
            .await
            .context(QuoteStorageSnafu)?,
    );

    let esplora_client = esplora_client::Builder::new(&args.bitcoin_wallet_esplora_url)
        .build_async()
        .context(EsploraInitializationSnafu)?;

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

    let provider = Arc::new(
        create_websocket_wallet_provider(
            &args.ethereum_rpc_ws_url,
            args.ethereum_wallet_private_key,
        )
        .await?,
    );
    let evm_wallet = Arc::new(EVMWallet::new(
        provider.clone(),
        args.ethereum_rpc_ws_url,
        args.ethereum_confirmations,
        &mut join_set,
    ));

    // TODO: something better than adhoc approval?
    evm_wallet
        .ensure_inf_approval_on_disperse(
            &Address::from_str("0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf").unwrap(),
        )
        .await
        .expect("Failed to ensure inf approval on disperse contract");

    wallet_manager.register(ChainType::Ethereum, evm_wallet.clone());
    let btc_eth_price_oracle = price_oracle::BitcoinEtherPriceOracle::new(&mut join_set);

    let wrapped_bitcoin_quoter = WrappedBitcoinQuoter::new(
        btc_eth_price_oracle,
        esplora_client,
        provider.clone().erased(),
        args.trade_spread_bps,
        args.fee_safety_multiplier,
    );

    let otc_fill_client = otc_client::OtcFillClient::new(
        Config {
            market_maker_id,
            api_key_id: args.api_key_id.clone(),
            api_key: args.api_key.clone(),
            otc_ws_url: args.otc_ws_url.clone(),
            reconnect_interval_secs: 5,
            max_reconnect_attempts: 5,
        },
        wallet_manager,
        quote_storage.clone(),
    );
    join_set.spawn(async move { otc_fill_client.run().await.map_err(Error::from) });

    // Add RFQ client for handling quote requests
    let rfq_client = rfq_client::RfqClient::new(
        Config {
            market_maker_id,
            api_key_id: args.api_key_id,
            api_key: args.api_key,
            otc_ws_url: args.otc_ws_url,
            reconnect_interval_secs: 5,
            max_reconnect_attempts: 5,
        },
        args.rfq_ws_url,
        wrapped_bitcoin_quoter,
        quote_storage,
    );
    join_set.spawn(async move {
        rfq_client.run().await.map_err(|e| Error::Client {
            source: otc_client::ClientError::BackgroundThreadExited {
                source: Box::new(e),
            },
        })
    });

    handle_background_thread_result(join_set.join_next().await).context(BackgroundThreadSnafu)?;

    Ok(())
}
