use crate::{ChainType, SwapStatus};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Public types - safe to send to clients
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapPublic {
    pub id: Uuid,
    pub quote_id: Uuid,
    pub market_maker: String,
    
    // Public wallet info
    pub user_deposit: WalletPublic,
    pub mm_deposit: WalletPublic,
    
    // User's addresses
    pub user_destination_address: String,
    pub user_refund_address: String,
    
    // Status tracking
    pub status: SwapStatus,
    pub user_deposit_status: Option<DepositInfo>,
    pub mm_deposit_status: Option<DepositInfo>,
    
    // Only one withdrawal tx (MM deposit -> user destination)
    pub user_withdrawal_tx: Option<String>,
    
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletPublic {
    pub chain: ChainType,
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositInfo {
    pub tx_hash: String,
    pub amount: Decimal,
    pub detected_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementInfo {
    pub tx_hash: String,
    pub fee_rate: String, // gwei or sats/vB for example
    pub settled_at: DateTime<Utc>,
}

// Internal types - never leave the TEE
#[derive(Debug, Clone)]
pub struct SwapPrivate {
    pub public: SwapPublic,
    pub secrets: SwapSecrets,
}

#[derive(Debug, Clone)]
pub struct SwapSecrets {
    pub user_deposit_private_key: String,
    pub mm_deposit_private_key: String,
}

// Convenience methods
impl SwapPrivate {
    pub fn to_public(&self) -> SwapPublic {
        self.public.clone()
    }
}

// Product type conversions
impl From<SwapPrivate> for (SwapPublic, SwapSecrets) {
    fn from(swap: SwapPrivate) -> Self {
        (swap.public, swap.secrets)
    }
}

impl From<(SwapPublic, SwapSecrets)> for SwapPrivate {
    fn from((public, secrets): (SwapPublic, SwapSecrets)) -> Self {
        SwapPrivate { public, secrets }
    }
}