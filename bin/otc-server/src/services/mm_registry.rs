use dashmap::DashMap;
use otc_mm_protocol::{MMRequest, ProtocolMessage};
use snafu::Snafu;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Duration;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

#[derive(Debug, Snafu)]
pub enum MMRegistryError {
    #[snafu(display("Market maker '{}' not connected", market_maker_id))]
    MarketMakerNotConnected { market_maker_id: String },

    #[snafu(display("Validation request timed out for market maker '{}'", market_maker_id))]
    ValidationTimeout { market_maker_id: String },

    #[snafu(display("Failed to send message to market maker: {}", source))]
    MessageSendError {
        source: mpsc::error::SendError<ProtocolMessage<MMRequest>>,
    },

    #[snafu(display("Failed to receive validation response: {}", source))]
    ResponseReceiveError {
        source: oneshot::error::RecvError,
    },
}

type Result<T, E = MMRegistryError> = std::result::Result<T, E>;

pub struct MarketMakerConnection {
    pub id: Uuid,
    pub sender: mpsc::Sender<ProtocolMessage<MMRequest>>,
    pub protocol_version: String,
}

#[derive(Clone)]
pub struct MMRegistry {
    connections: Arc<DashMap<Uuid, MarketMakerConnection>>,
    pending_validations: Arc<DashMap<String, oneshot::Sender<Result<bool>>>>,
    validation_timeout: Duration,
}

impl MMRegistry {
    #[must_use] pub fn new(validation_timeout: Duration) -> Self {
        Self {
            connections: Arc::new(DashMap::new()),
            pending_validations: Arc::new(DashMap::new()),
            validation_timeout,
        }
    }

    pub fn register(
        &self,
        market_maker_id: Uuid,
        sender: mpsc::Sender<ProtocolMessage<MMRequest>>,
        protocol_version: String,
    ) {
        info!(
            market_maker_id = %market_maker_id,
            protocol_version = %protocol_version,
            "Registering market maker connection"
        );
        
        let connection = MarketMakerConnection {
            id: market_maker_id,
            sender,
            protocol_version,
        };
        
        self.connections.insert(market_maker_id, connection);
    }

    pub fn unregister(&self, market_maker_id: Uuid) {
        info!(market_maker_id = %market_maker_id, "Unregistering market maker connection");
        self.connections.remove(&market_maker_id);
    }

    #[must_use] pub fn is_connected(&self, market_maker_id: Uuid) -> bool {
        self.connections.contains_key(&market_maker_id)
    }

    pub async fn validate_quote(
        &self,
        market_maker_id: Uuid,
        quote_id: String,
        response_tx: oneshot::Sender<Result<bool>>,
    ) {
        debug!(
            market_maker_id = %market_maker_id,
            quote_id = %quote_id,
            "Validating quote with market maker"
        );

        let mm_connection = if let Some(conn) = self.connections.get(&market_maker_id) { conn } else {
            warn!(
                market_maker_id = %market_maker_id,
                "Market maker not connected"
            );
            let _ = response_tx.send(Err(MMRegistryError::MarketMakerNotConnected {
                market_maker_id: market_maker_id.to_string(),
            }));
            return;
        };

        let request = ProtocolMessage {
            version: mm_connection.protocol_version.clone(),
            sequence: 0, // TODO: Implement sequence tracking
            payload: MMRequest::ValidateQuote {
                request_id: Uuid::new_v4(),
                quote_id: Uuid::parse_str(&quote_id).unwrap_or_else(|_| Uuid::new_v4()),
                user_id: Uuid::new_v4(), // TODO: Get actual user ID
                timestamp: chrono::Utc::now(),
            },
        };

        // Send the validation request
        if let Err(e) = mm_connection.sender.send(request).await {
            error!(
                market_maker_id = %market_maker_id,
                error = %e,
                "Failed to send validation request"
            );
            let _ = response_tx.send(Err(MMRegistryError::MessageSendError { source: e }));
            return;
        }

        // Store the response channel for when we get the MM's response
        self.pending_validations.insert(quote_id, response_tx);
    }

    pub fn handle_validation_response(
        &self,
        market_maker_id: Uuid,
        quote_id: &str,
        accepted: bool,
    ) {
        debug!(
            market_maker_id = %market_maker_id,
            quote_id = %quote_id,
            accepted = %accepted,
            "Handling validation response"
        );

        // Find the pending validation for this quote
        if let Some((_, tx)) = self.pending_validations.remove(quote_id) {
            let _ = tx.send(Ok(accepted));
        } else {
            warn!(
                quote_id = %quote_id,
                "Received validation response for unknown quote"
            );
        }
    }

    #[must_use] pub fn get_connection_count(&self) -> usize {
        self.connections.len()
    }

    #[must_use] pub fn get_connected_market_makers(&self) -> Vec<Uuid> {
        self.connections
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_unregister() {
        let registry = MMRegistry::new(Duration::from_secs(5));
        let (tx, _rx) = mpsc::channel(10);
        let mm_id = Uuid::new_v4();
        
        // Register a market maker
        registry.register(mm_id, tx, "1.0.0".to_string());
        assert!(registry.is_connected(mm_id));
        assert_eq!(registry.get_connection_count(), 1);
        
        // Unregister
        registry.unregister(mm_id);
        assert!(!registry.is_connected(mm_id));
        assert_eq!(registry.get_connection_count(), 0);
    }

    #[tokio::test]
    async fn test_validate_quote_not_connected() {
        let registry = MMRegistry::new(Duration::from_secs(5));
        let (response_tx, response_rx) = oneshot::channel();
        let unknown_mm_id = Uuid::new_v4();

        
        let () = registry.validate_quote(unknown_mm_id, "quote123".to_string(), response_tx).await;
        
        let result = response_rx.await.unwrap();
        assert!(matches!(
            result,
            Err(MMRegistryError::MarketMakerNotConnected { .. })
        ));
    }
}