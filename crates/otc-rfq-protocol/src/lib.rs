use chrono::{DateTime, Utc};
use otc_models::{Lot, Quote};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Protocol wrapper for RFQ messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolMessage<T> {
    pub version: String,
    pub sequence: u64,
    pub payload: T,
}

/// Response from RFQ server confirming connection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connected {
    pub session_id: Uuid,
    pub server_version: String,
    pub timestamp: DateTime<Utc>,
}

/// Messages sent from RFQ server to Market Maker
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RFQRequest {
    /// Broadcast to all MMs when user requests quotes
    QuoteRequest {
        request_id: Uuid,
        from: Lot,
        to: Lot,
        timestamp: DateTime<Utc>,
    },
    
    /// Notify winning MM their quote was selected
    QuoteSelected {
        request_id: Uuid,
        quote_id: Uuid,
        timestamp: DateTime<Utc>,
    },
    
    /// Ping for keepalive
    Ping {
        request_id: Uuid,
        timestamp: DateTime<Utc>,
    },
}

/// Messages sent from Market Maker to RFQ server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RFQResponse {
    /// MM's response with their quote (or None if they can't quote)
    QuoteResponse {
        request_id: Uuid,
        quote: Option<Quote>,
        timestamp: DateTime<Utc>,
    },
    
    /// Pong response
    Pong {
        request_id: Uuid,
        timestamp: DateTime<Utc>,
    },
    
    /// Error response
    Error {
        request_id: Uuid,
        error_code: RFQErrorCode,
        message: String,
        timestamp: DateTime<Utc>,
    },
}

/// Standard error codes for RFQ protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RFQErrorCode {
    /// Cannot provide quote for this pair
    PairNotSupported,
    /// Insufficient liquidity
    InsufficientLiquidity,
    /// Invalid request format
    InvalidRequest,
    /// Internal MM error
    InternalError,
    /// Request timeout
    Timeout,
}