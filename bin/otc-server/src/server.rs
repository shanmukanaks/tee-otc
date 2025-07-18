use crate::Result;
use axum::{
    extract::ws::{WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::{get, Router},
    Json,
};
use clap::Parser;
use serde::{Deserialize, Serialize};
use snafu::prelude::*;
use sqlx::PgPool;
use std::net::{IpAddr, SocketAddr};
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "otc-server")]
#[command(about = "TEE-OTC server for cross-chain swaps")]
pub struct Args {
    /// Host to bind to
    #[arg(short = 'H', long, default_value = "127.0.0.1")]
    pub host: IpAddr,
    
    /// Port to bind to
    #[arg(short, long, default_value = "3000")]
    pub port: u16,
    
    /// Database URL
    #[arg(long, env = "DATABASE_URL", default_value = "postgres://otc_user:otc_password@localhost:5432/otc_db")]
    pub database_url: String,
}

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
}

#[derive(Serialize, Deserialize)]
struct Status {
    status: String,
    version: String,
}

pub async fn run_server(addr: SocketAddr, database_url: &str) -> Result<()> {
    info!("Starting OTC server...");
    
    info!("Connecting to database...");
    let db = PgPool::connect(database_url)
        .await
        .context(crate::DatabaseConnectionSnafu)?;
    
    info!("Checking database schema...");
    initialize_database(&db).await?;
    
    let state = AppState { db };
    
    let app = Router::new()
        .route("/status", get(status_handler))
        .route("/ws", get(websocket_handler))
        .with_state(state);
    
    info!("Listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context(crate::ServerBindSnafu)?;
    
    axum::serve(listener, app)
        .await
        .context(crate::ServerStartSnafu)?;
    
    Ok(())
}

async fn status_handler() -> impl IntoResponse {
    Json(Status {
        status: "online".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn websocket_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    info!("WebSocket connection established");
    
    while let Some(msg) = socket.recv().await {
        if let Ok(msg) = msg {
            match msg {
                axum::extract::ws::Message::Text(text) => {
                    info!("Received: {}", text);
                    
                    if socket
                        .send(axum::extract::ws::Message::Text(format!("Echo: {}", text)))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                axum::extract::ws::Message::Close(_) => {
                    info!("WebSocket connection closed");
                    break;
                }
                _ => {}
            }
        } else {
            break;
        }
    }
}

async fn initialize_database(db: &PgPool) -> Result<()> {
    // Check if tables exist
    let tables_exist: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT FROM information_schema.tables 
            WHERE table_schema = 'public' 
            AND table_name = 'quotes'
        )"
    )
    .fetch_one(db)
    .await
    .context(crate::DatabaseQuerySnafu)?;

    if !tables_exist {
        info!("Creating database schema...");
        sqlx::query(include_str!("schema.sql"))
            .execute(db)
            .await
            .context(crate::DatabaseQuerySnafu)?;
        info!("Database schema created successfully");
    } else {
        info!("Database schema already exists");
    }

    Ok(())
}