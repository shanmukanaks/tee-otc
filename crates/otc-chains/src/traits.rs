use crate::Result;
use async_trait::async_trait;
use otc_models::{DepositInfo, TxStatus};
use rust_decimal::Decimal;
use std::time::Duration;

#[async_trait]
pub trait ChainOperations: Send + Sync {
    /// Create a new wallet, returning (address, private_key)
    async fn create_wallet(&self) -> Result<(String, String)>;
    
    /// Get the balance of an address
    async fn get_balance(&self, address: &str) -> Result<Decimal>;
    
    /// Get transaction status
    async fn get_tx_status(&self, tx_hash: &str) -> Result<TxStatus>;
    
    /// Check for deposits to an address
    async fn check_deposit(
        &self,
        address: &str,
        expected_amount: Decimal,
        min_confirmations: u32,
    ) -> Result<Option<DepositInfo>>;
    
    /// Send funds from a wallet
    async fn send_funds(
        &self,
        private_key: &str,
        to_address: &str,
        amount: Decimal,
    ) -> Result<String>; // Returns tx hash
    
    /// Validate an address format
    fn validate_address(&self, address: &str) -> bool;
    
    /// Get minimum recommended confirmations
    fn minimum_confirmations(&self) -> u32;
    
    /// Get estimated block time
    fn estimated_block_time(&self) -> Duration;
}