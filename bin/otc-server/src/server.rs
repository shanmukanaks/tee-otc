use crate::{
    api::swaps::{CreateSwapRequest, CreateSwapResponse, SwapResponse}, auth::ApiKeyStore, config::Settings, db::Database, services::{MMRegistry, SwapManager, SwapMonitoringService}, OtcServerArgs, Result
};
use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, Path, State},
    http::{StatusCode, HeaderMap},
    response::IntoResponse,
    routing::{get, post, Router},
    Json,
};
use futures_util::{SinkExt, StreamExt};
use otc_chains::{bitcoin::BitcoinChain, ethereum::EthereumChain, ChainRegistry};
use otc_mm_protocol::{Connected, MMRequest, MMResponse, ProtocolMessage};
use serde::{Deserialize, Serialize};
use snafu::prelude::*;
use std::{net::SocketAddr, sync::Arc};
use tokio::{sync::mpsc, time::Duration};
use tracing::{error, info};
use uuid::Uuid;


#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub swap_manager: Arc<SwapManager>,
    pub mm_registry: Arc<MMRegistry>,
    pub api_key_store: Arc<ApiKeyStore>,
}

#[derive(Serialize, Deserialize)]
struct Status {
    status: String,
    version: String,
}

pub async fn run_server(args: OtcServerArgs) -> Result<()> {
    info!("Starting OTC server...");

    let addr = SocketAddr::from((args.host, args.port));
    
    // Load configuration
    let settings = Arc::new(Settings::load().map_err(|e| crate::Error::DatabaseInit { 
        source: crate::db::DbError::InvalidData { 
            message: format!("Failed to load settings: {}", e) 
        }
    })?);
    let db = Database::connect(&args.database_url)
        .await
        .context(crate::DatabaseInitSnafu)?;
    
    info!("Initializing chain registry...");
    let mut chain_registry = ChainRegistry::new();
    
    // Initialize Bitcoin chain (mock for now)
    let bitcoin_chain = BitcoinChain::new(
        "http://localhost:8332",
        bitcoincore_rpc::Auth::UserPass("user".to_string(), "pass".to_string()),
        bitcoin::Network::Testnet,
    ).map_err(|e| crate::Error::DatabaseInit { 
        source: crate::db::DbError::InvalidData { 
            message: format!("Failed to initialize Bitcoin chain: {}", e) 
        }
    })?;
    chain_registry.register(otc_models::ChainType::Bitcoin, Arc::new(bitcoin_chain));
    
    // Initialize Ethereum chain (mock for now)
    let ethereum_chain = EthereumChain::new(
        "http://localhost:8545",
        1, // mainnet chain ID
    ).await.map_err(|e| crate::Error::DatabaseInit { 
        source: crate::db::DbError::InvalidData { 
            message: format!("Failed to initialize Ethereum chain: {}", e) 
        }
    })?;
    chain_registry.register(otc_models::ChainType::Ethereum, Arc::new(ethereum_chain));
    
    let chain_registry = Arc::new(chain_registry);
    
    info!("Initializing services...");
    
    // Initialize API key store
    let api_key_store = Arc::new(ApiKeyStore::new(args.whitelist_file.into()).await?);
    
    // Initialize MM registry with 5-second validation timeout
    let mm_registry = Arc::new(MMRegistry::new(Duration::from_secs(5)));
    
    let swap_manager = Arc::new(SwapManager::new(
        db.clone(), 
        settings.clone(), 
        chain_registry.clone(),
        mm_registry.clone(),
    ));
    
    // Start the swap monitoring service
    let swap_monitoring_service = Arc::new(SwapMonitoringService::new(
        db.clone(), 
        settings.clone(), 
        chain_registry.clone()
    ));
    
    info!("Starting swap monitoring service...");
    tokio::spawn({
        let monitoring_service = swap_monitoring_service.clone();
        async move {
            monitoring_service.run().await;
        }
    });
    
    let state = AppState { 
        db,
        swap_manager,
        mm_registry,
        api_key_store,
    };
    
    let app = Router::new()
        // Health check
        .route("/status", get(status_handler))
        // WebSocket endpoints
        .route("/ws", get(websocket_handler))
        .route("/ws/mm", get(mm_websocket_handler))
        // API endpoints
        .route("/api/v1/swaps", post(create_swap))
        .route("/api/v1/swaps/:id", get(get_swap))
        .route("/api/v1/market-makers/connected", get(get_connected_market_makers))
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

async fn mm_websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Extract and validate authentication headers
    let api_key_id = match headers.get("x-api-key-id") {
        Some(value) => match value.to_str() {
            Ok(id_str) => match Uuid::parse_str(id_str) {
                Ok(id) => id,
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, "Invalid API key ID format").into_response();
                }
            },
            Err(_) => {
                return (StatusCode::BAD_REQUEST, "Invalid API key ID header").into_response();
            }
        },
        None => {
            return (StatusCode::UNAUTHORIZED, "Missing X-API-Key-ID header").into_response();
        }
    };
    
    let api_key = match headers.get("x-api-key") {
        Some(value) => match value.to_str() {
            Ok(key) => key,
            Err(_) => {
                return (StatusCode::BAD_REQUEST, "Invalid API key header").into_response();
            }
        },
        None => {
            return (StatusCode::UNAUTHORIZED, "Missing X-API-Key header").into_response();
        }
    };
    
    // Validate the API key
    match state.api_key_store.validate_by_id(&api_key_id, api_key) {
        Ok(market_maker_id) => {
            info!("Market maker {} authenticated via headers", market_maker_id);
            ws.on_upgrade(move |socket| handle_mm_socket(socket, state, market_maker_id))
        }
        Err(e) => {
            error!("API key validation failed: {}", e);
            (StatusCode::UNAUTHORIZED, "Invalid API key").into_response()
        }
    }
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

async fn create_swap(
    State(state): State<AppState>,
    Json(request): Json<CreateSwapRequest>,
) -> Result<Json<CreateSwapResponse>, (StatusCode, String)> {
    state.swap_manager
        .create_swap(request)
        .await
        .map(Json)
        .map_err(|e| {
            let status = match e {
                crate::services::swap_manager::SwapError::QuoteNotFound { .. } => StatusCode::NOT_FOUND,
                crate::services::swap_manager::SwapError::QuoteExpired => StatusCode::BAD_REQUEST,
                crate::services::swap_manager::SwapError::MarketMakerMismatch { .. } => StatusCode::BAD_REQUEST,
                crate::services::swap_manager::SwapError::MarketMakerRejected => StatusCode::CONFLICT,
                crate::services::swap_manager::SwapError::MarketMakerNotConnected { .. } => StatusCode::SERVICE_UNAVAILABLE,
                crate::services::swap_manager::SwapError::MarketMakerValidationTimeout => StatusCode::REQUEST_TIMEOUT,
                crate::services::swap_manager::SwapError::Database { .. } => StatusCode::INTERNAL_SERVER_ERROR,
                crate::services::swap_manager::SwapError::ChainNotSupported { .. } => StatusCode::BAD_REQUEST,
                crate::services::swap_manager::SwapError::WalletDerivation { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, e.to_string())
        })
}

async fn get_swap(
    State(state): State<AppState>,
    Path(swap_id): Path<Uuid>,
) -> Result<Json<SwapResponse>, (StatusCode, String)> {
    state.swap_manager
        .get_swap(swap_id)
        .await
        .map(Json)
        .map_err(|e| {
            let status = match e {
                crate::services::swap_manager::SwapError::QuoteNotFound { .. } => StatusCode::NOT_FOUND,
                crate::services::swap_manager::SwapError::Database { .. } => StatusCode::INTERNAL_SERVER_ERROR,
                crate::services::swap_manager::SwapError::ChainNotSupported { .. } => StatusCode::INTERNAL_SERVER_ERROR,
                crate::services::swap_manager::SwapError::WalletDerivation { .. } => StatusCode::INTERNAL_SERVER_ERROR,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, e.to_string())
        })
}

#[derive(Serialize)]
struct ConnectedMarketMakersResponse {
    market_makers: Vec<String>,
}

async fn get_connected_market_makers(
    State(state): State<AppState>,
) -> Json<ConnectedMarketMakersResponse> {
    let market_makers = state.mm_registry.get_connected_market_makers();
    Json(ConnectedMarketMakersResponse { market_makers })
}

async fn handle_mm_socket(socket: WebSocket, state: AppState, market_maker_id: String) {
    info!("Market maker {} WebSocket connection established", market_maker_id);
    
    // Channel for sending messages to the MM
    let (tx, mut rx) = mpsc::channel::<ProtocolMessage<MMRequest>>(100);
    
    // Split the socket for bidirectional communication
    let (sender, mut receiver) = socket.split();
    
    // Register the MM immediately (already authenticated via headers)
    state.mm_registry.register(
        market_maker_id.clone(),
        tx.clone(),
        "1.0.0".to_string(), // Default protocol version
    );
    
    let mm_id = market_maker_id;
    
    // Send Connected response
    let connected_response = Connected {
        session_id: Uuid::new_v4(),
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: chrono::Utc::now(),
    };
    
    let response = serde_json::json!({
        "Connected": connected_response
    });
    
    let (sender_tx, mut sender_rx) = mpsc::channel::<Message>(100);
    
    // Send initial connected response
    if sender_tx.send(Message::Text(response.to_string())).await.is_err() {
        error!("Failed to send Connected response");
        return;
    }
    
    // Spawn task to handle outgoing messages from the registry
    let mm_id_clone = mm_id.clone();
    let sender_tx_clone = sender_tx.clone();
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if sender_tx_clone.send(Message::Text(json)).await.is_err() {
                    error!("Failed to send message to market maker {}", mm_id_clone);
                    break;
                }
            }
        }
    });
    
    // Spawn task to forward messages to the socket
    let mm_id_clone = mm_id.clone();
    let mut sender = sender;
    tokio::spawn(async move {
        while let Some(msg) = sender_rx.recv().await {
            if sender.send(msg).await.is_err() {
                error!("Failed to send message to market maker {} socket", mm_id_clone);
                break;
            }
        }
    });
    
    // Handle incoming messages
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                match serde_json::from_str::<ProtocolMessage<MMResponse>>(&text) {
                    Ok(msg) => {
                        match &msg.payload {
                            MMResponse::QuoteValidated { quote_id, accepted, .. } => {
                                info!(
                                    "Market maker {} validated quote {}: accepted={}",
                                    mm_id, quote_id, accepted
                                );
                                state.mm_registry.handle_validation_response(
                                    &mm_id,
                                    &quote_id.to_string(),
                                    *accepted,
                                );
                            }
                            MMResponse::Pong { .. } => {
                                // Handle pong for keepalive
                            }
                            MMResponse::DepositInitiated { .. } => {
                                // Handle deposit notification - will be implemented when needed
                            }
                            MMResponse::SwapCompleteAck { .. } => {
                                // Handle swap complete acknowledgment
                            }
                            MMResponse::Error { .. } => {
                                // Handle error response
                                error!("Received error response from market maker {}", mm_id);
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to parse MM message: {}", e);
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!("Market maker {} disconnected", mm_id);
                break;
            }
            Err(e) => {
                error!("WebSocket error for market maker {}: {}", mm_id, e);
                break;
            }
            _ => {}
        }
    }
    
    // Unregister on disconnect
    state.mm_registry.unregister(&mm_id);
    info!("Market maker {} unregistered", mm_id);
}