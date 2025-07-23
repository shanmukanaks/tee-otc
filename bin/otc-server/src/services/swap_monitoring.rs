use crate::config::Settings;
use crate::db::{Database, DbError};
use chrono::Utc;
use otc_chains::ChainRegistry;
use otc_models::{
    Swap, SwapStatus, UserDepositStatus, MMDepositStatus
};
use snafu::prelude::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tracing::{info, warn, error};

#[derive(Debug, Snafu)]
pub enum MonitoringError {
    #[snafu(display("Database error: {}", source))]
    Database { source: DbError },
    
    #[snafu(display("Chain operation error: {}", source))]
    ChainOperation { source: otc_chains::Error },
    
    #[snafu(display("Invalid state transition from {:?}", current_state))]
    InvalidTransition { current_state: SwapStatus },
}

pub type MonitoringResult<T> = Result<T, MonitoringError>;

/// Background service that monitors all active swaps for:
/// - Incoming deposits (user and MM)
/// - Confirmation tracking
/// - Timeouts and refund triggers
/// - Settlement completion
pub struct SwapMonitoringService {
    db: Database,
    settings: Arc<Settings>,
    chain_registry: Arc<ChainRegistry>,
    // TODO: Add market_maker_registry for notifications
}

impl SwapMonitoringService {
    pub fn new(
        db: Database,
        settings: Arc<Settings>,
        chain_registry: Arc<ChainRegistry>,
    ) -> Self {
        Self {
            db,
            settings,
            chain_registry,
        }
    }
    
    /// Start the monitoring service
    pub async fn run(self: Arc<Self>) {
        info!("Starting swap monitoring service");
        
        // Check every 10 seconds
        let mut interval = time::interval(Duration::from_secs(10));
        
        loop {
            interval.tick().await;
            
            if let Err(e) = self.monitor_all_swaps().await {
                error!("Error monitoring swaps: {}", e);
            }
        }
    }
    
    /// Monitor all active swaps
    async fn monitor_all_swaps(&self) -> MonitoringResult<()> {
        // Get all active swaps
        let active_swaps = self.db.swaps()
            .get_active()
            .await
            .context(DatabaseSnafu)?;
        
        info!("Monitoring {} active swaps", active_swaps.len());
        
        for swap in active_swaps {
            if let Err(e) = self.monitor_swap(&swap).await {
                error!("Error monitoring swap {}: {}", swap.id, e);
            }
        }
        
        Ok(())
    }
    
    /// Monitor a single swap based on its current state
    async fn monitor_swap(&self, swap: &Swap) -> MonitoringResult<()> {
        // Check for timeout first
        if swap.timeout_at < Utc::now() {
            return self.handle_timeout(swap).await;
        }
        
        match swap.status {
            SwapStatus::WaitingUserDeposit => {
                self.check_user_deposit(swap).await?;
            }
            SwapStatus::WaitingMMDeposit => {
                self.check_mm_deposit(swap).await?;
            }
            SwapStatus::WaitingConfirmations => {
                self.check_confirmations(swap).await?;
            }
            SwapStatus::Settling => {
                self.check_settlement_completion(swap).await?;
            }
            _ => {
                // Other states don't need monitoring
            }
        }
        
        Ok(())
    }
    
    /// Check for user deposit
    async fn check_user_deposit(&self, swap: &Swap) -> MonitoringResult<()> {
        // Get the quote to know what token/chain to check
        let quote = self.db.quotes()
            .get(swap.quote_id)
            .await
            .context(DatabaseSnafu)?;
        
        // Get the chain operations for the user's deposit chain (from = user sends)
        let chain_ops = self.chain_registry
            .get(&quote.from.chain)
            .ok_or(MonitoringError::ChainOperation {
                source: otc_chains::Error::ChainNotSupported { 
                    chain: format!("{:?}", quote.from.chain) 
                }
            })?;
        
        // Derive the user deposit address
        let master_key = self.settings.master_key_bytes();
        let user_wallet = chain_ops
            .derive_wallet(&master_key, &swap.user_deposit_salt)
            .context(ChainOperationSnafu)?;
        
        // Check for deposit
        let deposit_info = chain_ops
            .check_deposit(
                &user_wallet.address,
                quote.from.amount,
                0, // Accept 0 confirmations for initial detection
            )
            .await
            .context(ChainOperationSnafu)?;
        
        if let Some(deposit) = deposit_info {
            info!(
                "User deposit detected for swap {}: {} on chain {:?}",
                swap.id, deposit.tx_hash, quote.from.chain
            );
            
            // Update swap state
            let user_deposit_status = UserDepositStatus {
                tx_hash: deposit.tx_hash.clone(),
                amount: deposit.amount,
                detected_at: Utc::now(),
                confirmations: 0, // Initial detection
                last_checked: Utc::now(),
            };
            
            self.db.swaps()
                .user_deposit_detected(swap.id, user_deposit_status)
                .await
                .context(DatabaseSnafu)?;
            
            // TODO: Notify market maker about user deposit
            info!("TODO: Notify market maker {} about user deposit", swap.market_maker);
        }
        
        Ok(())
    }
    
    /// Check for market maker deposit
    async fn check_mm_deposit(&self, swap: &Swap) -> MonitoringResult<()> {
        // Get the quote to know what token/chain to check
        let quote = self.db.quotes()
            .get(swap.quote_id)
            .await
            .context(DatabaseSnafu)?;
        
        // Get the chain operations for the MM's deposit chain (to = MM sends)
        let chain_ops = self.chain_registry
            .get(&quote.to.chain)
            .ok_or(MonitoringError::ChainOperation {
                source: otc_chains::Error::ChainNotSupported { 
                    chain: format!("{:?}", quote.to.chain) 
                }
            })?;
        
        // Derive the MM deposit address
        let master_key = self.settings.master_key_bytes();
        let mm_wallet = chain_ops
            .derive_wallet(&master_key, &swap.mm_deposit_salt)
            .context(ChainOperationSnafu)?;
        
        // Check for deposit
        let deposit_info = chain_ops
            .check_deposit(
                &mm_wallet.address,
                quote.to.amount,
                0, // Accept 0 confirmations for initial detection
            )
            .await
            .context(ChainOperationSnafu)?;
        
        if let Some(deposit) = deposit_info {
            info!(
                "MM deposit detected for swap {}: {} on chain {:?}",
                swap.id, deposit.tx_hash, quote.to.chain
            );
            
            // Update swap state
            let mm_deposit_status = MMDepositStatus {
                tx_hash: deposit.tx_hash.clone(),
                amount: deposit.amount,
                detected_at: Utc::now(),
                confirmations: 0, // Initial detection
                last_checked: Utc::now(),
            };
            
            self.db.swaps()
                .mm_deposit_detected(swap.id, mm_deposit_status)
                .await
                .context(DatabaseSnafu)?;
        }
        
        Ok(())
    }
    
    /// Check and update confirmation counts
    async fn check_confirmations(&self, swap: &Swap) -> MonitoringResult<()> {
        let quote = self.db.quotes()
            .get(swap.quote_id)
            .await
            .context(DatabaseSnafu)?;
        
        // Check both deposits for confirmations
        let mut user_confirmations = 0u32;
        let mut mm_confirmations = 0u32;
        
        // Check user deposit confirmations
        if let Some(user_deposit) = &swap.user_deposit_status {
            let chain_ops = self.chain_registry
                .get(&quote.from.chain)
                .ok_or(MonitoringError::ChainOperation {
                    source: otc_chains::Error::ChainNotSupported { 
                        chain: format!("{:?}", quote.from.chain) 
                    }
                })?;
            
            let tx_status = chain_ops
                .get_tx_status(&user_deposit.tx_hash)
                .await
                .context(ChainOperationSnafu)?;
            
            user_confirmations = match tx_status {
                otc_models::TxStatus::NotFound => 0,
                otc_models::TxStatus::Confirmed(n) => n,
            };
            
            // Update confirmation count if changed
            if user_confirmations != user_deposit.confirmations {
                self.db.swaps()
                    .update_user_confirmations(swap.id, user_confirmations)
                    .await
                    .context(DatabaseSnafu)?;
            }
        }
        
        // Check MM deposit confirmations
        if let Some(mm_deposit) = &swap.mm_deposit_status {
            let chain_ops = self.chain_registry
                .get(&quote.to.chain)
                .ok_or(MonitoringError::ChainOperation {
                    source: otc_chains::Error::ChainNotSupported { 
                        chain: format!("{:?}", quote.to.chain) 
                    }
                })?;
            
            let tx_status = chain_ops
                .get_tx_status(&mm_deposit.tx_hash)
                .await
                .context(ChainOperationSnafu)?;
            
            mm_confirmations = match tx_status {
                otc_models::TxStatus::NotFound => 0,
                otc_models::TxStatus::Confirmed(n) => n,
            };
            
            // Update confirmation count if changed
            if mm_confirmations != mm_deposit.confirmations {
                self.db.swaps()
                    .update_mm_confirmations(swap.id, mm_confirmations)
                    .await
                    .context(DatabaseSnafu)?;
            }
        }
        
        // Check if we have enough confirmations
        // TODO: Make this dynamic based on chain and amount
        const REQUIRED_CONFIRMATIONS: u32 = 3;
        
        if user_confirmations >= REQUIRED_CONFIRMATIONS && mm_confirmations >= REQUIRED_CONFIRMATIONS {
            info!(
                "Swap {} has sufficient confirmations (user: {}, mm: {})",
                swap.id, user_confirmations, mm_confirmations
            );
            
            self.db.swaps()
                .confirmations_reached(swap.id)
                .await
                .context(DatabaseSnafu)?;
            
            // TODO: Send private key to market maker
            info!("TODO: Send user private key to market maker {}", swap.market_maker);
        }
        
        Ok(())
    }
    
    /// Check if settlement transaction has completed
    async fn check_settlement_completion(&self, swap: &Swap) -> MonitoringResult<()> {
        if let Some(settlement) = &swap.settlement_status {
            let quote = self.db.quotes()
                .get(swap.quote_id)
                .await
                .context(DatabaseSnafu)?;
            
            // Check the chain where we sent the settlement
            let chain_ops = self.chain_registry
                .get(&quote.to.chain)
                .ok_or(MonitoringError::ChainOperation {
                    source: otc_chains::Error::ChainNotSupported { 
                        chain: format!("{:?}", quote.to.chain) 
                    }
                })?;
            
            let tx_status = chain_ops
                .get_tx_status(&settlement.tx_hash)
                .await
                .context(ChainOperationSnafu)?;
            
            // Consider settlement complete after 1 confirmation
            if matches!(tx_status, otc_models::TxStatus::Confirmed(n) if n >= 1) {
                info!("Settlement completed for swap {}", swap.id);
                
                self.db.swaps()
                    .settlement_completed(swap.id)
                    .await
                    .context(DatabaseSnafu)?;
            }
        }
        
        Ok(())
    }
    
    /// Handle swap timeout
    async fn handle_timeout(&self, swap: &Swap) -> MonitoringResult<()> {
        warn!("Swap {} has timed out in state {:?}", swap.id, swap.status);
        
        match swap.status {
            SwapStatus::WaitingUserDeposit => {
                // No deposits yet, just mark as failed
                self.db.swaps()
                    .mark_failed(swap.id, "Timeout waiting for user deposit")
                    .await
                    .context(DatabaseSnafu)?;
            }
            SwapStatus::WaitingMMDeposit => {
                // User deposited but MM didn't, refund user
                self.db.swaps()
                    .initiate_user_refund(swap.id, "Timeout waiting for MM deposit")
                    .await
                    .context(DatabaseSnafu)?;
                
                // TODO: Actually execute the refund
                info!("TODO: Execute user refund for swap {}", swap.id);
            }
            SwapStatus::WaitingConfirmations | SwapStatus::Settling => {
                // Both deposited, refund both
                self.db.swaps()
                    .initiate_both_refunds(swap.id, "Timeout during settlement")
                    .await
                    .context(DatabaseSnafu)?;
                
                // TODO: Actually execute the refunds
                info!("TODO: Execute refunds for both parties in swap {}", swap.id);
            }
            _ => {
                // Other states don't need timeout handling
            }
        }
        
        Ok(())
    }
}