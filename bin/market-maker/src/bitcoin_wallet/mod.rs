pub mod transaction_broadcaster;

use std::sync::Arc;

use async_trait::async_trait;
use bdk_esplora::esplora_client;
use bdk_wallet::rusqlite::Connection;
use bdk_wallet::{
    bitcoin::{self, Network},
    error::CreateTxError,
    signer::SignerError,
    CreateParams, KeychainKind, LoadParams, LoadWithPersistError, PersistedWallet,
};
use otc_models::{ChainType, Currency, Lot, TokenIdentifier};
use snafu::{ResultExt, Snafu};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tracing::info;

use crate::wallet::{self, Wallet as WalletTrait, WalletError};

const STOP_GAP: usize = 50;
const PARALLEL_REQUESTS: usize = 5;
const BALANCE_BUFFER_PERCENT: u64 = 25; // 25% buffer

#[derive(Debug, Snafu)]
pub enum BitcoinWalletError {
    #[snafu(display("Failed to open database: {}", source))]
    OpenDatabase { source: bdk_wallet::rusqlite::Error },

    #[snafu(display("Failed to load wallet: {}", source))]
    LoadWallet {
        source: Box<LoadWithPersistError<bdk_wallet::rusqlite::Error>>,
    },

    #[snafu(display("Failed to create wallet: {}", source))]
    CreateWallet {
        source: Box<bdk_wallet::CreateWithPersistError<bdk_wallet::rusqlite::Error>>,
    },

    #[snafu(display("Failed to persist wallet: {}", source))]
    PersistWallet { source: bdk_wallet::rusqlite::Error },

    #[snafu(display("Failed to build Esplora client: {}", source))]
    BuildEsploraClient {
        source: bdk_esplora::esplora_client::Error,
    },

    #[snafu(display("Failed to sync wallet: {}", source))]
    SyncWallet {
        source: Box<bdk_esplora::esplora_client::Error>,
    },

    #[snafu(display("Failed to apply update"))]
    ApplyUpdate,

    #[snafu(display("Failed to build transaction: {}", source))]
    BuildTransaction { source: CreateTxError },

    #[snafu(display("Failed to sign transaction: {}", source))]
    SignTransaction { source: SignerError },

    #[snafu(display("Failed to extract transaction: {}", source))]
    ExtractTransaction {
        source: bdk_wallet::bitcoin::psbt::ExtractTxError,
    },

    #[snafu(display("Failed to broadcast transaction: {}", source))]
    BroadcastTransaction {
        source: bdk_esplora::esplora_client::Error,
    },

    #[snafu(display("Invalid Bitcoin address: {}", address))]
    InvalidAddress { address: String },

    #[snafu(display("Failed to parse address: {}", source))]
    ParseAddress {
        source: bitcoin::address::ParseError,
    },

    #[snafu(display("Insufficient balance"))]
    InsufficientBalance,
}

pub struct BitcoinWallet {
    pub tx_broadcaster: transaction_broadcaster::BitcoinTransactionBroadcaster,
    wallet: Arc<Mutex<PersistedWallet<Connection>>>,
}

impl BitcoinWallet {
    pub async fn new(
        db_file: &str,
        external_descriptor: &str,
        network: Network,
        esplora_url: &str,
        join_set: &mut JoinSet<crate::Result<()>>,
    ) -> Result<Self, BitcoinWalletError> {
        let mut conn = Connection::open(db_file).context(OpenDatabaseSnafu)?;

        // Try to load existing wallet
        let load_params = LoadParams::new()
            .descriptor(
                KeychainKind::External,
                Some(external_descriptor.to_string()),
            )
            .extract_keys()
            .check_network(network);

        let wallet_opt = PersistedWallet::load(&mut conn, load_params).map_err(|e| {
            BitcoinWalletError::LoadWallet {
                source: Box::new(e),
            }
        })?;

        let wallet = match wallet_opt {
            Some(wallet) => wallet,
            None => {
                // Create new wallet
                let create_params =
                    CreateParams::new_single(external_descriptor.to_string()).network(network);

                PersistedWallet::create(&mut conn, create_params).map_err(|e| {
                    BitcoinWalletError::CreateWallet {
                        source: Box::new(e),
                    }
                })?
            }
        };

        let esplora_client = esplora_client::Builder::new(esplora_url)
            .build_async()
            .context(BuildEsploraClientSnafu)?;

        let wallet = Arc::new(Mutex::new(wallet));
        let connection = Arc::new(Mutex::new(conn));
        let esplora_client = Arc::new(esplora_client);

        let tx_broadcaster = transaction_broadcaster::BitcoinTransactionBroadcaster::new(
            wallet.clone(),
            connection.clone(),
            esplora_client.clone(),
            network,
            join_set,
        );

        Ok(Self {
            tx_broadcaster,
            wallet,
        })
    }

    async fn check_balance(&self, lot: &Lot) -> Result<bool, BitcoinWalletError> {
        let wallet = self.wallet.lock().await;
        let balance = wallet.balance();

        let amount_sats = lot.amount.to::<u64>();
        let required_balance = balance_with_buffer(amount_sats);

        Ok(balance.total().to_sat() > required_balance)
    }
}

#[async_trait]
impl WalletTrait for BitcoinWallet {
    async fn create_transaction(
        &self,
        lot: &Lot,
        to_address: &str,
        nonce: Option<[u8; 16]>,
    ) -> wallet::Result<String> {
        ensure_valid_lot(lot)?;

        info!(
            "Queueing Bitcoin transaction to {} for {:?}",
            to_address, lot
        );

        // Send transaction request to the broadcaster
        self.tx_broadcaster
            .broadcast_transaction(lot.clone(), to_address.to_string(), nonce)
            .await
            .map_err(|e| match e {
                transaction_broadcaster::TransactionBroadcasterError::InvalidCurrency => {
                    WalletError::UnsupportedLot {
                        lot: lot.clone(),
                    }
                }
                transaction_broadcaster::TransactionBroadcasterError::InsufficientBalance => {
                    WalletError::InsufficientBalance {
                        required: lot.amount.to_string(),
                        available: "unknown".to_string(),
                    }
                }
                transaction_broadcaster::TransactionBroadcasterError::ParseAddress { reason } => {
                    WalletError::ParseAddressFailed { context: reason }
                }
                _ => WalletError::TransactionCreationFailed {
                    reason: e.to_string(),
                },
            })
    }

    async fn can_fill(&self, lot: &Lot) -> wallet::Result<bool> {
        if ensure_valid_lot(lot).is_err() {
            return Ok(false);
        }

        self.check_balance(lot)
            .await
            .map_err(|e| WalletError::BalanceCheckFailed {
                reason: e.to_string(),
            })
    }
}

fn ensure_valid_lot(lot: &Lot) -> Result<(), WalletError> {
    if !matches!(lot.currency.chain, ChainType::Bitcoin)
        || !matches!(lot.currency.token, TokenIdentifier::Native)
    {
        return Err(WalletError::UnsupportedLot {
            lot: lot.clone(),
        });
    }

    // Bitcoin has 8 decimals
    if lot.currency.decimals != 8 {
        return Err(WalletError::UnsupportedLot {
            lot: lot.clone(),
        });
    }

    info!("Bitcoin lot is valid: {:?}", lot);
    Ok(())
}

fn balance_with_buffer(balance_sats: u64) -> u64 {
    balance_sats + (balance_sats * BALANCE_BUFFER_PERCENT) / 100
}
