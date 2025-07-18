use crate::{ChainOperations, Result};
use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::network::{TransactionBuilder, EthereumWallet};
use alloy::rpc::types::TransactionRequest;
use async_trait::async_trait;
use otc_models::{DepositInfo, TxStatus};
use rust_decimal::prelude::*;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};

pub struct EthereumChain {
    provider: Arc<dyn Provider<alloy::network::Ethereum>>,
    chain_id: u64,
}

impl EthereumChain {
    pub async fn new(rpc_url: &str, chain_id: u64) -> Result<Self> {
        let provider = ProviderBuilder::new()
            .connect_http(rpc_url.parse().map_err(|_| crate::Error::Serialization {
                message: "Invalid RPC URL".to_string(),
            })?);
            
        Ok(Self {
            provider: Arc::new(provider),
            chain_id,
        })
    }
}

#[async_trait]
impl ChainOperations for EthereumChain {
    async fn create_wallet(&self) -> Result<(String, String)> {
        // Create a new random signer
        let signer = PrivateKeySigner::random();
        let address = signer.address();
        let private_key = alloy::primitives::hex::encode(signer.to_bytes());
        
        info!("Created new Ethereum wallet: {}", address);
        
        Ok((format!("{:?}", address), format!("0x{}", private_key)))
    }
    
    async fn get_balance(&self, address: &str) -> Result<Decimal> {
        let addr = Address::from_str(address)
            .map_err(|_| crate::Error::InvalidAddress)?;
        
        let balance = self.provider
            .get_balance(addr)
            .await
            .map_err(|_| crate::Error::Rpc {
                message: "Failed to get balance".to_string(),
            })?;
        
        // Convert from wei to ETH
        let eth_balance = Decimal::from_str(&balance.to_string())
            .unwrap_or_default()
            / Decimal::from_str("1000000000000000000").unwrap();
        
        debug!("Balance for {}: {} ETH", address, eth_balance);
        Ok(eth_balance)
    }
    
    async fn get_tx_status(&self, tx_hash: &str) -> Result<TxStatus> {
        let hash = tx_hash.parse()
            .map_err(|_| crate::Error::Serialization {
                message: "Invalid transaction hash".to_string(),
            })?;
        
        // Get transaction receipt
        match self.provider.get_transaction_receipt(hash).await {
            Ok(Some(receipt)) => {
                // Get current block number
                let current_block = self.provider
                    .get_block_number()
                    .await
                    .map_err(|_| crate::Error::Rpc {
                        message: "Failed to get block number".to_string(),
                    })?;
                
                let confirmations = current_block
                    .saturating_sub(receipt.block_number.unwrap_or_default()) as u32;
                
                Ok(TxStatus::Confirmed(confirmations))
            }
            Ok(None) => {
                // Check if transaction is in mempool
                match self.provider.get_transaction_by_hash(hash).await {
                    Ok(Some(_)) => Ok(TxStatus::Confirmed(0)), // In mempool
                    _ => Ok(TxStatus::NotFound),
                }
            }
            Err(_) => Ok(TxStatus::NotFound),
        }
    }
    
    async fn check_deposit(
        &self,
        address: &str,
        _expected_amount: Decimal,
        _min_confirmations: u32,
    ) -> Result<Option<DepositInfo>> {
        // In production, would:
        // 1. Get recent blocks
        // 2. Filter for transactions to this address
        // 3. Check amounts and confirmations
        
        debug!("Checking deposits for address: {}", address);
        Ok(None)
    }
    
    async fn send_funds(
        &self,
        private_key: &str,
        to_address: &str,
        amount: Decimal,
    ) -> Result<String> {
        let signer = PrivateKeySigner::from_str(private_key.trim_start_matches("0x"))
            .map_err(|_| crate::Error::Serialization {
                message: "Invalid private key".to_string(),
            })?;
            
        let to = Address::from_str(to_address)
            .map_err(|_| crate::Error::InvalidAddress)?;
        
        // Convert ETH to wei
        let wei_amount = (amount * Decimal::from_str("1000000000000000000").unwrap())
            .to_u256()
            .ok_or_else(|| crate::Error::Serialization {
                message: "Amount overflow".to_string(),
            })?;
        
        let _wallet = EthereumWallet::from(signer);
        
        // Create transaction
        let _tx = TransactionRequest::default()
            .with_to(to)
            .with_value(U256::from_str(&wei_amount.to_string()).unwrap())
            .with_chain_id(self.chain_id);
        
        // In production, would:
        // 1. Estimate gas
        // 2. Set gas price
        // 3. Sign and send transaction
        
        info!("Sending {} ETH to {}", amount, to_address);
        Ok("0x0000000000000000000000000000000000000000000000000000000000000000".to_string())
    }
    
    fn validate_address(&self, address: &str) -> bool {
        Address::from_str(address).is_ok()
    }
    
    fn minimum_confirmations(&self) -> u32 {
        12 // Standard for Ethereum
    }
    
    fn estimated_block_time(&self) -> Duration {
        Duration::from_secs(12) // ~12 seconds
    }
}

// Helper to convert Decimal to U256
trait DecimalExt {
    fn to_u256(&self) -> Option<U256>;
}

impl DecimalExt for Decimal {
    fn to_u256(&self) -> Option<U256> {
        self.to_string().parse().ok()
    }
}