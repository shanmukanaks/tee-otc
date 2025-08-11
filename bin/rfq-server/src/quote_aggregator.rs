use crate::mm_registry::RfqMMRegistry;
use futures_util::future;
use otc_models::{Currency, Lot, Quote, QuoteRequest};
use otc_rfq_protocol::RFQResponse;
use snafu::Snafu;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn};
use uuid::Uuid;

#[derive(Debug, Snafu)]
pub enum QuoteAggregatorError {
    #[snafu(display("No market makers connected"))]
    NoMarketMakersConnected,

    #[snafu(display("No quotes received from market makers"))]
    NoQuotesReceived,

    #[snafu(display("Quote aggregation timeout"))]
    AggregationTimeout,
}

type Result<T, E = QuoteAggregatorError> = std::result::Result<T, E>;

pub struct QuoteAggregator {
    mm_registry: Arc<RfqMMRegistry>,
    timeout_duration: Duration,
}

#[derive(Debug, Clone)]
pub struct QuoteRequestResult {
    pub request_id: Uuid,
    pub best_quote: Quote,
    pub total_quotes_received: usize,
    pub market_makers_contacted: usize,
}

impl QuoteAggregator {
    #[must_use]
    pub fn new(mm_registry: Arc<RfqMMRegistry>, timeout_milliseconds: u64) -> Self {
        Self {
            mm_registry,
            timeout_duration: Duration::from_millis(timeout_milliseconds),
        }
    }

    /// Request quotes from all connected market makers and return the best one
    pub async fn request_quotes(&self, request: QuoteRequest) -> Result<QuoteRequestResult> {
        let request_id = Uuid::new_v4();

        info!(
            request_id = %request_id,
            mode = ?request.mode,
            from_chain = ?request.from.chain,
            from_amount = %request.amount,
            to_chain = ?request.to.chain,
            "Starting quote aggregation"
        );

        // Broadcast quote request to all connected MMs
        let receivers = self
            .mm_registry
            .broadcast_quote_request(&request_id, &request)
            .await;

        if receivers.is_empty() {
            return Err(QuoteAggregatorError::NoMarketMakersConnected);
        }

        let market_makers_contacted = receivers.len();
        let mut quotes = Vec::new();

        // Collect quotes with timeout
        let collection_result = timeout(
            self.timeout_duration,
            self.collect_quotes(receivers, request_id),
        )
        .await;

        match collection_result {
            Ok(collected_quotes) => quotes = collected_quotes,
            Err(_) => {
                debug!(
                    request_id = %request_id,
                    "Quote collection timed out, proceeding with quotes received so far"
                );
            }
        }

        if quotes.is_empty() {
            return Err(QuoteAggregatorError::NoQuotesReceived);
        }

        let total_quotes = quotes.len();

        info!(
            request_id = %request_id,
            quotes_received = total_quotes,
            market_makers_contacted = market_makers_contacted,
            "Collected quotes from market makers"
        );

        // Select the best quote (highest output amount)
        let best_quote = quotes
            .into_iter()
            .max_by_key(|q| q.to.amount)
            .expect("quotes vector is not empty");

        // Notify the winning market maker
        if let Err(e) = self
            .mm_registry
            .notify_quote_selected(best_quote.market_maker_id, request_id, best_quote.id)
            .await
        {
            warn!(
                market_maker_id = %best_quote.market_maker_id,
                quote_id = %best_quote.id,
                error = %e,
                "Failed to notify market maker of quote selection"
            );
        }

        Ok(QuoteRequestResult {
            request_id,
            best_quote,
            total_quotes_received: total_quotes,
            market_makers_contacted,
        })
    }

    /// Collect quotes from market makers
    async fn collect_quotes(
        &self,
        receivers: Vec<(Uuid, mpsc::Receiver<RFQResponse>)>,
        _request_id: Uuid,
    ) -> Vec<Quote> {
        let mut quotes = Vec::new();

        // Convert receivers into futures
        let mut futures = Vec::new();
        for (mm_id, mut rx) in receivers {
            let future = async move {
                match rx.recv().await {
                    Some(response) => match response {
                        RFQResponse::QuoteResponse { quote, .. } => {
                            // We don't check request_id since each MM gets a unique ID
                            quote.map(|q| (mm_id, q))
                        }
                        _ => None,
                    },
                    None => {
                        warn!(
                            market_maker_id = %mm_id,
                            "Market maker channel closed without response"
                        );
                        None
                    }
                }
            };
            futures.push(future);
        }

        // Wait for all futures to complete
        let results = future::join_all(futures).await;

        for (mm_id, quote) in results.into_iter().flatten() {
            debug!(
                market_maker_id = %mm_id,
                quote_id = %quote.id,
                to_amount = %quote.to.amount,
                "Received quote from market maker"
            );
            quotes.push(quote);
        }

        quotes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;
    use otc_models::{ChainType, QuoteMode, TokenIdentifier};

    #[tokio::test]
    async fn test_no_market_makers() {
        let registry = Arc::new(RfqMMRegistry::new());
        let aggregator = QuoteAggregator::new(registry, 5);

        let request = QuoteRequest {
            mode: QuoteMode::ExactInput,
            from: Currency {
                chain: ChainType::Bitcoin,
                token: TokenIdentifier::Native,
                decimals: 8,
            },
            to: Currency {
                chain: ChainType::Ethereum,
                token: TokenIdentifier::Native,
                decimals: 18,
            },
            amount: U256::from(100_000u64),
        };

        let result = aggregator.request_quotes(request).await;
        assert!(matches!(
            result,
            Err(QuoteAggregatorError::NoMarketMakersConnected)
        ));
    }
}
