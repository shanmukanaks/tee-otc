use std::{
    env::current_dir,
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};

use ctor::ctor;
use devnet::MultichainAccount;
use market_maker::MarketMakerArgs;
use otc_server::OtcServerArgs;
use sqlx::postgres::PgConnectOptions;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

pub trait PgConnectOptionsExt {
    fn to_database_url(&self) -> String;
}

impl PgConnectOptionsExt for PgConnectOptions {
    fn to_database_url(&self) -> String {
        format!(
            "postgres://{}:{}@{}:{}/{}",
            self.get_username(),
            "password",
            self.get_host(),
            self.get_port(),
            self.get_database().expect("database should be set")
        )
    }
}

pub async fn get_free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("Should be able to bind to port");

    listener
        .local_addr()
        .expect("Should have a local address")
        .port()
}

pub const TEST_MARKET_MAKER_ID: &str = "550e8400-e29b-41d4-a716-446655440000";
pub const TEST_API_KEY_ID: &str = "d2e0a695-e3b1-494e-b645-1b41a72d7e75";
pub const TEST_API_KEY: &str = "7KNJu1t1j9DtVqS0d8FB6pfX0nkqr4TX";
pub const TEST_MM_WHITELIST_FILE: &str =
    "integration-tests/src/utils/test_whitelisted_market_makers.json";
pub const INTEGRATION_TEST_TIMEOUT_SECS: u64 = 10;

pub fn get_whitelist_file_path() -> String {
    // Convert relative path to absolute path from workspace root
    let mut current_dir = current_dir().expect("Should be able to get current directory");

    // If we're already in integration-tests, go up to workspace root
    if current_dir.file_name().and_then(|n| n.to_str()) == Some("integration-tests") {
        current_dir = current_dir.parent().unwrap().to_path_buf();
    }

    let whitelist_file_path = current_dir.join(TEST_MM_WHITELIST_FILE);
    whitelist_file_path.to_string_lossy().to_string()
}

pub async fn wait_for_otc_server_to_be_ready(otc_port: u16) {
    // Hit the otc server status endpoint every 100ms until it returns 200
    let client = reqwest::Client::new();
    let status_url = format!("http://127.0.0.1:{otc_port}/status");

    let start_time = std::time::Instant::now();
    let timeout = Duration::from_secs(INTEGRATION_TEST_TIMEOUT_SECS);

    loop {
        assert!(
            (start_time.elapsed() <= timeout),
            "Timeout waiting for OTC server to become ready"
        );

        tokio::time::sleep(Duration::from_millis(100)).await;

        if let Ok(response) = client.get(&status_url).send().await {
            if response.status() == 200 {
                println!("OTC server is ready!");
                break;
            }
        }
    }
}

pub fn build_bitcoin_wallet_descriptor(private_key: &bitcoin::PrivateKey) -> String {
    format!("wpkh({private_key})")
}

pub fn build_mm_test_args(
    otc_port: u16,
    multichain_account: &MultichainAccount,
    devnet: &devnet::RiftDevnet,
) -> MarketMakerArgs {
    MarketMakerArgs {
        market_maker_id: TEST_MARKET_MAKER_ID.to_string(),
        api_key_id: TEST_API_KEY_ID.to_string(),
        api_key: TEST_API_KEY.to_string(),
        otc_ws_url: format!("ws://127.0.0.1:{otc_port}/ws/mm"),
        auto_accept: true,
        log_level: "info".to_string(),
        bitcoin_wallet_db_file: format!("/tmp/bitcoin_wallet_{}.db", uuid::Uuid::new_v4()),
        bitcoin_wallet_descriptor: build_bitcoin_wallet_descriptor(
            &multichain_account.bitcoin_wallet.private_key,
        ),
        bitcoin_wallet_network: bitcoin::Network::Regtest,
        bitcoin_wallet_esplora_url: devnet.bitcoin.esplora_url.as_ref().unwrap().to_string(),
        ethereum_wallet_private_key: multichain_account.secret_bytes,
        ethereum_confirmations: 1,
        ethereum_rpc_ws_url: devnet.ethereum.anvil.ws_endpoint(),
    }
}

pub fn build_otc_server_test_args(
    otc_port: u16,
    devnet: &devnet::RiftDevnet,
    connect_options: &PgConnectOptions,
) -> OtcServerArgs {
    OtcServerArgs {
        port: otc_port,
        database_url: connect_options.to_database_url(),
        whitelist_file: get_whitelist_file_path(),
        host: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        log_level: "info".to_string(),
        ethereum_mainnet_rpc_url: devnet.ethereum.anvil.endpoint(),
        ethereum_mainnet_token_indexer_url: devnet
            .ethereum
            .token_indexer
            .as_ref()
            .unwrap()
            .api_server_url
            .clone(),
        ethereum_mainnet_chain_id: devnet.ethereum.anvil.chain_id(),
        bitcoin_rpc_url: devnet.bitcoin.rpc_url_with_cookie.clone(),
        esplora_http_server_url: devnet.bitcoin.esplora_url.as_ref().unwrap().to_string(),
        bitcoin_network: bitcoin::network::Network::Regtest,
    }
}

#[ctor]
fn init_test_tracing() {
    let has_nocapture = std::env::args().any(|arg| arg == "--nocapture" || arg == "--show-output");
    if has_nocapture {
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
            )
            .try_init()
            .ok();
    }
}
