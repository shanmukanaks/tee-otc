use chrono::Utc;
use otc_models::{Currency, Lot, Quote};
use otc_rfq_protocol::{ProtocolMessage, RFQRequest, RFQResponse};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::price_oracle::PriceOracle;

pub struct RFQMessageHandler {
    market_maker_id: Uuid,
    price_oracle: Option<PriceOracle>,
}

impl RFQMessageHandler {
    pub fn new(market_maker_id: Uuid) -> Self {
        Self {
            market_maker_id,
            price_oracle: None,
        }
    }

    pub fn with_price_oracle(market_maker_id: Uuid, price_oracle: PriceOracle) -> Self {
        Self {
            market_maker_id,
            price_oracle: Some(price_oracle),
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

                // For now, create a symmetric quote (same amount out as in)
                // This is just for testing the flow
                let quote = Quote {
                    id: Uuid::new_v4(),
                    market_maker_id: self.market_maker_id,
                    from: Lot {
                        currency: request.from.clone(),
                        amount: request.amount,
                    },
                    to: Lot {
                        currency: request.to.clone(),
                        amount: request.amount,
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
