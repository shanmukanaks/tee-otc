use crate::traits::MarketMakerPaymentValidation;
use crate::{key_derivation, ChainOperations, Result};
use alloy::primitives::{Address, Log, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionReceipt;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use async_trait::async_trait;
use common::inverse_compute_protocol_fee;
use evm_token_indexer_client::TokenIndexerClient;
use otc_models::{ChainType, Lot, TokenIdentifier, TransferInfo, TxStatus, Wallet};
use std::str::FromStr;
use std::time::Duration;
use tracing::{debug, info};

sol! {
    #[derive(Debug)]
    event Transfer(address indexed from, address indexed to, uint256 value);
}

const ALLOWED_TOKEN: &str = "0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf";

pub struct EthereumChain {
    provider: DynProvider,
    evm_indexer_client: TokenIndexerClient,
    chain_id: u64,
    allowed_token: Address,
}

impl EthereumChain {
    pub async fn new(rpc_url: &str, evm_indexer_url: &str, chain_id: u64) -> Result<Self> {
        let provider = ProviderBuilder::new()
            .connect_http(rpc_url.parse().map_err(|_| crate::Error::Serialization {
                message: "Invalid RPC URL".to_string(),
            })?)
            .erased();

        let evm_indexer_client = TokenIndexerClient::new(evm_indexer_url)?;
        let allowed_token =
            Address::from_str(ALLOWED_TOKEN).map_err(|_| crate::Error::Serialization {
                message: "Invalid allowed token address".to_string(),
            })?;

        Ok(Self {
            provider,
            evm_indexer_client,
            chain_id,
            allowed_token,
        })
    }
}

#[async_trait]
impl ChainOperations for EthereumChain {
    fn create_wallet(&self) -> Result<(Wallet, [u8; 32])> {
        // Generate a random salt
        let mut salt = [0u8; 32];
        getrandom::getrandom(&mut salt).map_err(|_| crate::Error::Serialization {
            message: "Failed to generate random salt".to_string(),
        })?;

        // Create a new random signer
        let signer = PrivateKeySigner::random();
        let address = signer.address();
        let private_key = alloy::primitives::hex::encode(signer.to_bytes());

        info!("Created new Ethereum wallet: {}", address);

        let wallet = Wallet::new(format!("{address:?}"), format!("0x{private_key}"));
        Ok((wallet, salt))
    }

    fn derive_wallet(&self, master_key: &[u8], salt: &[u8; 32]) -> Result<Wallet> {
        // Derive private key using HKDF
        let private_key_bytes =
            key_derivation::derive_private_key(master_key, salt, b"ethereum-wallet")?;

        // Create signer from derived key
        let signer = PrivateKeySigner::from_bytes(&private_key_bytes.into()).map_err(|_| {
            crate::Error::Serialization {
                message: "Failed to create signer from derived key".to_string(),
            }
        })?;

        let address = format!("{:?}", signer.address());
        let private_key = format!("0x{}", alloy::hex::encode(private_key_bytes));

        debug!("Derived Ethereum wallet: {}", address);

        Ok(Wallet::new(address, private_key))
    }

    async fn get_tx_status(&self, tx_hash: &str) -> Result<TxStatus> {
        let tx_hash_parsed = tx_hash.parse().map_err(|_| crate::Error::Serialization {
            message: "Invalid transaction hash".to_string(),
        })?;
        let tx = self
            .provider
            .get_transaction_receipt(tx_hash_parsed)
            .await?;

        if tx.is_some() {
            let current_block_height = self.provider.get_block_number().await?;
            Ok(TxStatus::Confirmed(
                current_block_height - tx.unwrap().block_number.unwrap(),
            ))
        } else {
            Ok(TxStatus::NotFound)
        }
    }
    async fn search_for_transfer(
        &self,
        recipient_address: &str,
        lot: &Lot,
        mm_payment: Option<MarketMakerPaymentValidation>,
        _from_block_height: Option<u64>,
    ) -> Result<Option<TransferInfo>> {
        let token_address = match &lot.currency.token {
            TokenIdentifier::Address(address) => address,
            TokenIdentifier::Native => return Ok(None),
        };
        let token_address =
            Address::from_str(token_address).map_err(|_| crate::Error::Serialization {
                message: "Invalid token address".to_string(),
            })?;

        if token_address != self.allowed_token {
            debug!("Token address {} is not allowed", token_address);
            return Ok(None);
        }

        let recipient_address =
            Address::from_str(recipient_address).map_err(|_| crate::Error::Serialization {
                message: "Invalid address".to_string(),
            })?;

        let transfer_hint = self
            .get_transfer(&recipient_address, &lot.amount, mm_payment)
            .await?;
        if transfer_hint.is_none() {
            return Ok(None);
        }

        Ok(Some(transfer_hint.unwrap()))
    }

    fn validate_address(&self, address: &str) -> bool {
        Address::from_str(address).is_ok()
    }

    fn minimum_block_confirmations(&self) -> u32 {
        4 // Standard for Ethereum
    }

    fn estimated_block_time(&self) -> Duration {
        Duration::from_secs(12) // ~12 seconds
    }
}

impl EthereumChain {
    // Note this function's response is safe to trust, b/c it will validate the responses from the untrusted evm_indexer_client
    async fn get_transfer(
        &self,
        recipient_address: &Address,
        amount: &U256,
        mm_payment: Option<MarketMakerPaymentValidation>,
    ) -> Result<Option<TransferInfo>> {
        info!(
            "Searching for transfer for address: {}, amount: {}, mm_payment: {:?}",
            recipient_address, amount, mm_payment
        );

        // use the untrusted evm_indexer_client to get the transfer hint - this will only return 50 latest transfers (TODO: how to handle this?)
        let transfers = self
            .evm_indexer_client
            .get_transfers_to(*recipient_address, None, Some(*amount))
            .await?;

        if transfers.transfers.is_empty() {
            info!("No transfers found");
            return Ok(None);
        }

        debug!("TransfersResponse from evm_indexer_client: {:?}", transfers);

        let mut transfer_hint: Option<TransferInfo> = None;
        for transfer in transfers.transfers {
            let transaction_receipt = self
                .provider
                .get_transaction_receipt(transfer.transaction_hash)
                .await?;

            if transaction_receipt.is_none() {
                debug!("Transaction receipt not found for transfer: {:?}", transfer);
                continue;
            }
            let transaction_receipt = transaction_receipt.unwrap();
            if !transaction_receipt.status() {
                debug!(
                    "Transaction receipt not successful for transfer: {:?}",
                    transfer
                );
                continue;
            }

            let intra_tx_transfers =
                extract_all_transfers_from_transaction_receipt(&transaction_receipt);

            // TODO: There's a security issue with handling more than 1 swap per tx, so for now we force there to be no more than 2 transfers per tx (1 for the swap, 1 for the fee)
            if intra_tx_transfers.len() > 2 {
                debug!("More than 2 transfers in transaction",);
                for transfer_log in intra_tx_transfers {
                    debug!("Transfer: {:?}", transfer_log);
                }
                continue;
            }
            for (index, transfer_log) in intra_tx_transfers.iter().enumerate() {
                // validate the recipient
                if transfer_log.to != *recipient_address {
                    debug!(
                        "Transfer recipient is not the expected address: {:?}",
                        transfer
                    );
                    continue;
                }
                // validate the amount
                if transfer_log.value < *amount {
                    debug!("Transfer amount is less than expected: {:?}", transfer);
                    continue;
                }
                // validate the embedded nonce
                if let Some(mm_payment) = &mm_payment {
                    let embedded_nonce = mm_payment.embedded_nonce;
                    let transaction = self
                        .provider
                        .get_raw_transaction_by_hash(transfer.transaction_hash)
                        .await?;
                    if transaction.is_none() {
                        debug!("Transaction not found for transfer: {:?}", transfer);
                        continue;
                    }
                    let transaction = transaction.unwrap();
                    let tx_hex = alloy::hex::encode(transaction);
                    let nonce_hex = alloy::hex::encode(embedded_nonce);
                    if !tx_hex.contains(&nonce_hex) {
                        debug!(
                            "Transaction does not contain the expected nonce: {:?}",
                            transfer
                        );
                        continue;
                    }

                    let fee_address = Address::from_str(
                        &otc_models::FEE_ADDRESSES_BY_CHAIN[&ChainType::Ethereum],
                    )
                    .map_err(|_| crate::Error::Serialization {
                        message: "Invalid fee address".to_string(),
                    })?;

                    // NOTE: This is only works b/c we force there to be 2 transfers IF it's a MM payment
                    let fee_log_index = if index == 0 { 1 } else { 0 };
                    let fee_log = intra_tx_transfers[fee_log_index].clone();
                    if fee_log.to != fee_address {
                        info!("Fee address is not the expected address");
                        continue;
                    }
                    if fee_log.value < mm_payment.fee_amount {
                        info!("Fee amount is less than expected");
                        continue;
                    }
                }
                // get the current block height
                let current_block_height = self.provider.get_block_number().await?;
                let confirmations =
                    current_block_height - transaction_receipt.block_number.unwrap();

                // only return the transfer if it has more confirmations than the previous transfer hint
                if transfer_hint.is_some()
                    && transfer_hint.as_ref().unwrap().confirmations > confirmations
                {
                    debug!(
                        "Transfer has more confirmations than the previous transfer hint: {:?}",
                        transfer
                    );
                    continue;
                }

                transfer_hint = Some(TransferInfo {
                    tx_hash: alloy::hex::encode(transfer.transaction_hash),
                    detected_at: chrono::Utc::now(),
                    confirmations,
                    amount: transfer_log.value,
                });
            }
        }

        Ok(transfer_hint)
    }
}

fn extract_all_transfers_from_transaction_receipt(
    transaction_receipt: &TransactionReceipt,
) -> Vec<Log<Transfer>> {
    let mut transfers = Vec::new();
    for log in transaction_receipt.logs() {
        let transfer_log = log.log_decode::<Transfer>();
        if transfer_log.is_err() {
            // This log is not a transfer log, so skip it
            continue;
        }
        let transfer_log = transfer_log.unwrap().inner;
        transfers.push(transfer_log);
    }
    transfers
}
