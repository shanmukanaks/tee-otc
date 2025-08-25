use chrono::Utc;
use otc_models::{Currency, Lot, Quote};
use otc_rfq_protocol::{ProtocolMessage, RFQRequest, RFQResponse, RFQResult};
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::quote_storage::QuoteStorage;
use crate::wallet::WalletManager;
use crate::wrapped_bitcoin_quoter::WrappedBitcoinQuoter;

pub struct RFQMessageHandler {
    market_maker_id: Uuid,
    wrapped_bitcoin_quoter: WrappedBitcoinQuoter,
    quote_storage: Arc<QuoteStorage>,
    wallet_manager: WalletManager,
}

impl RFQMessageHandler {
    pub fn new(
        market_maker_id: Uuid,
        wrapped_bitcoin_quoter: WrappedBitcoinQuoter,
        quote_storage: Arc<QuoteStorage>,
        wallet_manager: WalletManager,
    ) -> Self {
        Self {
            market_maker_id,
            wrapped_bitcoin_quoter,
            quote_storage,
            wallet_manager,
        }
    }

    pub async fn handle_request(
        &self,
        msg: &ProtocolMessage<RFQRequest>,
    ) -> Option<ProtocolMessage<RFQResponse>> {
        match &msg.payload {
            RFQRequest::QuoteRequested {
                request_id,
                request,
                timestamp: _,
            } => {
                info!(
                    "Received RFQ quote request: request_id={}, mode={:?}, from_chain={:?}, amount={}, to_chain={:?}",
                    request_id, request.mode, request.from.chain, request.amount, request.to.chain
                );

                let quote = self
                    .wrapped_bitcoin_quoter
                    .compute_quote(self.market_maker_id, request)
                    .await;
                if quote.is_err() {
                    tracing::error!("Failed to compute quote: {:?}", quote.err());
                    return None;
                }
                let mut rfq_result = quote.unwrap();

                // Check if we have sufficient balance to fulfill the quote
                if let RFQResult::Success(ref quote_with_fees) = rfq_result {
                    let wallet = self.wallet_manager.get(quote_with_fees.quote.to.currency.chain);
                    
                    let can_fill = if let Some(wallet) = wallet {
                        match wallet.can_fill(&quote_with_fees.quote.to).await {
                            Ok(can_fill) => can_fill,
                            Err(e) => {
                                warn!("Failed to check wallet balance: {}", e);
                                false
                            }
                        }
                    } else {
                        warn!("No wallet configured for chain {:?}", quote_with_fees.quote.to.currency.chain);
                        false
                    };
                    
                    if !can_fill {
                        info!(
                            "Insufficient balance to fulfill quote {}: need {} on {:?}",
                            quote_with_fees.quote.id,
                            quote_with_fees.quote.to.amount,
                            quote_with_fees.quote.to.currency.chain
                        );
                        rfq_result = RFQResult::MakerUnavailable(
                            "Insufficient balance to fulfill quote".to_string()
                        );
                    }
                }

                let quote = match &rfq_result {
                    RFQResult::Success(quote) => Some(quote.quote.clone()),
                    RFQResult::MakerUnavailable(_) => None,
                    RFQResult::InvalidRequest(_) => None,
                };

                if let Some(quote) = quote {
                    info!(
                        "Generated quote: id={}, from_chain={:?}, from_amount={}, to_chain={:?}, to_amount={}",
                        quote.id, quote.from.currency.chain, quote.from.amount, quote.to.currency.chain , quote.to.amount
                    );
                    if let Err(e) = self.quote_storage.store_quote(&quote).await {
                        error!("Failed to store quote {}: {}", quote.id, e);
                    } else {
                        info!("Stored quote {} in database", quote.id);
                        if let Err(e) = self.quote_storage.mark_sent_to_rfq(quote.id).await {
                            error!("Failed to mark quote {} as sent to RFQ: {}", quote.id, e);
                        }
                    }
                }

                let response = RFQResponse::QuoteResponse {
                    request_id: *request_id,
                    quote: rfq_result,
                    timestamp: Utc::now(),
                };

                Some(ProtocolMessage {
                    version: msg.version.clone(),
                    sequence: msg.sequence,
                    payload: response,
                })
            }
            RFQRequest::QuoteSelected {
                request_id,
                quote_id,
                timestamp: _,
            } => {
                info!(
                    "Our quote {} was selected! Request ID: {}",
                    quote_id, request_id
                );
                // Mark the quote as sent to OTC since it was selected
                if let Err(e) = self.quote_storage.mark_sent_to_otc(*quote_id).await {
                    error!("Failed to mark quote {} as sent to OTC: {}", quote_id, e);
                }
                None
            }
            RFQRequest::Ping {
                request_id,
                timestamp: _,
            } => {
                let response = RFQResponse::Pong {
                    request_id: *request_id,
                    timestamp: Utc::now(),
                };

                Some(ProtocolMessage {
                    version: msg.version.clone(),
                    sequence: msg.sequence,
                    payload: response,
                })
            }
        }
    }
}
