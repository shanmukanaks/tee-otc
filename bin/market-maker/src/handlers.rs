use crate::config::Config;
use crate::strategy::ValidationStrategy;
use chrono::Utc;
use otc_mm_protocol::{MMRequest, MMResponse, MMStatus, ProtocolMessage};
use tracing::{info, warn};

pub struct MessageHandler {
    config: Config,
    strategy: ValidationStrategy,
}

impl MessageHandler {
    pub fn new(config: Config) -> Self {
        let strategy = ValidationStrategy::new(config.auto_accept);
        Self { config, strategy }
    }

    pub fn handle_request(
        &self,
        msg: &ProtocolMessage<MMRequest>,
    ) -> Option<ProtocolMessage<MMResponse>> {
        match &msg.payload {
            MMRequest::ValidateQuote {
                request_id,
                quote_id,
                user_id,
                timestamp,
            } => {
                info!(
                    "Received quote validation request for quote {} from user {}",
                    quote_id, user_id
                );

                let (accepted, rejection_reason) = self.strategy.validate_quote(quote_id);

                info!(
                    "Quote {} validation result: accepted={}, reason={:?}",
                    quote_id, accepted, &rejection_reason
                );

                let response = MMResponse::QuoteValidated {
                    request_id: *request_id,
                    quote_id: *quote_id,
                    accepted,
                    rejection_reason,
                    mm_destination_address: None, // TODO: Implement address generation
                    timestamp: Utc::now(),
                };

                Some(ProtocolMessage {
                    version: msg.version.clone(),
                    sequence: msg.sequence + 1,
                    payload: response,
                })
            }

            MMRequest::UserDeposited {
                
                swap_id,
                
                deposit_address,
                deposit_chain,
                deposit_amount,
                user_tx_hash,
                deposit_deadline,
                ..
            } => {
                info!(
                    "User deposited for swap {}: {} on {:?} chain to {}",
                    swap_id, deposit_amount, deposit_chain, deposit_address
                );
                info!("User tx hash: {}", user_tx_hash);
                info!("Deposit deadline: {}", deposit_deadline);

                // TODO: Implement actual deposit logic
                // For now, just acknowledge
                warn!("TODO: Implement actual deposit to {}", deposit_address);

                // In a real implementation, we would:
                // 1. Verify the user's deposit on-chain
                // 2. Initiate our deposit to the provided address
                // 3. Send DepositInitiated response with our tx hash

                None // For now, we don't respond to this
            }

            MMRequest::SwapComplete {
                request_id,
                swap_id,
                
                user_withdrawal_tx,
                ..
            } => {
                info!(
                    "Swap {} complete, received user's private key",
                    swap_id
                );
                info!("User withdrawal tx: {}", user_withdrawal_tx);
                
                // TODO: Implement claiming logic
                warn!("TODO: Implement claiming from user's wallet");

                let response = MMResponse::SwapCompleteAck {
                    request_id: *request_id,
                    swap_id: *swap_id,
                    timestamp: Utc::now(),
                };

                Some(ProtocolMessage {
                    version: msg.version.clone(),
                    sequence: msg.sequence + 1,
                    payload: response,
                })
            }

            MMRequest::Ping { request_id, .. } => {
                let response = MMResponse::Pong {
                    request_id: *request_id,
                    status: MMStatus::Active,
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    timestamp: Utc::now(),
                };

                Some(ProtocolMessage {
                    version: msg.version.clone(),
                    sequence: msg.sequence + 1,
                    payload: response,
                })
            }
        }
    }
}