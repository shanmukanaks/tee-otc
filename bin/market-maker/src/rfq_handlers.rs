use chrono::Utc;
use otc_models::{Currency, Quote};
use otc_rfq_protocol::{RFQRequest, RFQResponse, ProtocolMessage};
use tracing::{info, warn};
use uuid::Uuid;

pub struct RfqMessageHandler {
    market_maker_id: String,
}

impl RfqMessageHandler {
    pub fn new(market_maker_id: String) -> Self {
        Self { market_maker_id }
    }

    pub async fn handle_request(
        &self,
        msg: &ProtocolMessage<RFQRequest>,
    ) -> Option<ProtocolMessage<RFQResponse>> {
        match &msg.payload {
            RFQRequest::QuoteRequest {
                request_id,
                from,
                to,
                timestamp: _,
            } => {
                info!(
                    "Received RFQ quote request: request_id={}, from_chain={:?}, from_amount={}, to_chain={:?}",
                    request_id, from.chain, from.amount, to.chain
                );

                // Parse market maker ID as UUID
                let mm_uuid = match Uuid::parse_str(&self.market_maker_id) {
                    Ok(uuid) => uuid,
                    Err(e) => {
                        warn!("Invalid market maker UUID {}: {}", self.market_maker_id, e);
                        return None;
                    }
                };

                // For now, create a symmetric quote (same amount out as in)
                // This is just for testing the flow
                let quote = Quote {
                    id: Uuid::new_v4(),
                    market_maker_id: mm_uuid,
                    from: from.clone(),
                    to: Currency {
                        chain: to.chain.clone(),
                        token: to.token.clone(),
                        amount: from.amount, // Symmetric for now
                        decimals: to.decimals,
                    },
                    expires_at: Utc::now() + chrono::Duration::minutes(5),
                    created_at: Utc::now(),
                };

                info!(
                    "Generated quote: id={}, from_amount={}, to_amount={}",
                    quote.id, quote.from.amount, quote.to.amount
                );

                let response = RFQResponse::QuoteResponse {
                    request_id: *request_id,
                    quote: Some(quote),
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
                // For now, just acknowledge. In the future, this would trigger
                // preparation for the actual swap
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