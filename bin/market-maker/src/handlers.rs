use crate::strategy::ValidationStrategy;
use crate::{config::Config, wallet::WalletManager};
use chrono::Utc;
use otc_mm_protocol::{MMErrorCode, MMRequest, MMResponse, MMStatus, ProtocolMessage};
use tracing::{info, warn};

pub struct MessageHandler {
    config: Config,
    strategy: ValidationStrategy,
    wallet_manager: WalletManager,
}

impl MessageHandler {
    pub fn new(config: Config, wallet_manager: WalletManager) -> Self {
        let strategy = ValidationStrategy::new(config.auto_accept);
        Self {
            config,
            strategy,
            wallet_manager,
        }
    }

    pub async fn handle_request(
        &self,
        msg: &ProtocolMessage<MMRequest>,
    ) -> Option<ProtocolMessage<MMResponse>> {
        match &msg.payload {
            MMRequest::ValidateQuote {
                request_id,
                quote_id,
                quote_hash,
                user_destination_address,
                timestamp,
            } => {
                info!(
                    "Received quote validation request for quote {} from user {}",
                    quote_id, user_destination_address
                );

                let (accepted, rejection_reason) =
                    self.strategy
                        .validate_quote(quote_id, quote_hash, user_destination_address);

                info!(
                    "Quote {} validation result: accepted={}, reason={:?}",
                    quote_id, accepted, &rejection_reason
                );

                let response = MMResponse::QuoteValidated {
                    request_id: *request_id,
                    quote_id: *quote_id,
                    accepted,
                    rejection_reason,
                    timestamp: Utc::now(),
                };

                Some(ProtocolMessage {
                    version: msg.version.clone(),
                    sequence: msg.sequence + 1,
                    payload: response,
                })
            }

            MMRequest::UserDeposited {
                request_id,
                swap_id,
                quote_id,
                deposit_address,
                user_tx_hash,
                ..
            } => {
                info!(
                    "User deposited for swap {}: to address {}",
                    swap_id, deposit_address
                );
                info!("Quote ID: {}", quote_id);
                info!("User tx hash: {}", user_tx_hash);

                // TODO: Implement actual deposit logic
                // For now, just acknowledge
                warn!("TODO: Implement actual deposit to {}", deposit_address);

                // In a real implementation, we would:
                // 1. Verify the user's deposit on-chain
                // 2. Initiate our deposit to the provided address
                // 3. Send DepositInitiated response with our tx hash

                None // For now, we don't respond to this
            }

            MMRequest::UserDepositConfirmed {
                request_id,
                swap_id,
                quote_id,
                user_destination_address,
                mm_nonce,
                expected_currency,
                ..
            } => {
                info!(
                    message = "User deposit confirmed for swap {swap_id}: MM should send {expected_currency:?} to {user_destination_address}",
                    quote_id = quote_id.to_string(),
                );

                // TODO: We should have additional safety checks here to ensure the user's deposit is valid
                // instead of trusting the TEE
                let wallet = self.wallet_manager.get(expected_currency.chain);
                let response: MMResponse = {
                    if let Some(wallet) = wallet {
                        let tx_result = wallet
                            .create_transaction(
                                expected_currency,
                                user_destination_address,
                                Some(*mm_nonce),
                            )
                            .await;

                        match tx_result {
                            Ok(txid) => MMResponse::DepositInitiated {
                                request_id: *request_id,
                                swap_id: *swap_id,
                                tx_hash: txid,
                                amount_sent: expected_currency.amount,
                                timestamp: Utc::now(),
                            },
                            Err(e) => MMResponse::Error {
                                request_id: *request_id,
                                error_code: MMErrorCode::InternalError,
                                message: e.to_string(),
                                timestamp: Utc::now(),
                            },
                        }
                    } else {
                        MMResponse::Error {
                            request_id: *request_id,
                            error_code: MMErrorCode::UnsupportedChain,
                            message: "No wallet found for chain".to_string(),
                            timestamp: Utc::now(),
                        }
                    }
                };

                Some(ProtocolMessage {
                    version: msg.version.clone(),
                    sequence: msg.sequence + 1,
                    payload: response,
                })
            }

            MMRequest::SwapComplete {
                request_id,
                swap_id,

                user_withdrawal_tx,
                ..
            } => {
                info!("Swap {} complete, received user's private key", swap_id);
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
