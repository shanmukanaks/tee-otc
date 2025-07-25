use crate::ChainType;
use alloy::primitives::U256;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", content = "data")]
pub enum TokenIdentifier {
    Native,
    Address(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Currency { 
    pub chain: ChainType,
    pub token: TokenIdentifier,
    pub amount: U256,
    pub decimals: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub id: Uuid,

    /// The market maker that created the quote
    pub market_maker_id: Uuid,
    
    /// The currency the user will send
    pub from: Currency,
    
    /// The currency the user will receive
    pub to: Currency,
    
    /// The expiration time of the quote
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}