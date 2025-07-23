use std::net::IpAddr;

use snafu::{prelude::*, Whatever};
use clap::Parser;

pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod server;
pub mod services;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to set global subscriber"))]
    SetGlobalSubscriber { source: tracing::subscriber::SetGlobalDefaultError },
    
    #[snafu(display("Failed to bind server"))]
    ServerBind { source: std::io::Error },
    
    #[snafu(display("Server failed to start"))]
    ServerStart { source: std::io::Error },
    
    #[snafu(display("Failed to connect to database"))]
    DatabaseConnection { source: sqlx::Error },
    
    #[snafu(display("Database query failed"))]
    DatabaseQuery { source: sqlx::Error },
    
    #[snafu(display("Database initialization failed: {}", source))]
    DatabaseInit { source: db::DbError },

    #[snafu(display("Generic error: {}", source))]
    Generic { source: Whatever },
}

impl From<Whatever> for Error {
    fn from(err: Whatever) -> Self {
        Error::Generic { source: err }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Parser, Debug)]
#[command(name = "otc-server")]
#[command(about = "TEE-OTC server for cross-chain swaps")]
pub struct OtcServerArgs {
    /// Host to bind to
    #[arg(short = 'H', long, default_value = "127.0.0.1")]
    pub host: IpAddr,
    
    /// Port to bind to
    #[arg(short, long, default_value = "3000")]
    pub port: u16,
    
    /// Database URL
    #[arg(long, env = "DATABASE_URL", default_value = "postgres://otc_user:otc_password@localhost:5432/otc_db")]
    pub database_url: String,

    /// Log level
    #[arg(long, env = "RUST_LOG", default_value = "info")]
    pub log_level: String,

    /// API keys file
    #[arg(long, env = "WHITELISTED_MM_FILE", default_value = "prod_whitelisted_market_makers.json")]
    pub whitelist_file: String,
}

impl Default for OtcServerArgs {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".parse().unwrap(),
            port: 3000,
            database_url: "postgres://otc_user:otc_password@localhost:5432/otc_db".to_string(),
            log_level: "info".to_string(),
            whitelist_file: "prod_whitelisted_market_makers.json".to_string(),
        }
    }
}

