use crate::ChainType;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "data")]
pub enum TokenIdentifier {
    Native,
    Address(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Currency { 
    pub chain: ChainType,
    pub token: TokenIdentifier,
    pub amount: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub id: Uuid,
    
    // Conversion details
    pub from: Currency,  // Amount user will send
    pub to: Currency,    // Amount user will receive
    
    pub market_maker_identifier: String,
    
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}