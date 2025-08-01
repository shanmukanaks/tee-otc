use std::sync::Arc;
use std::time::Duration;

use bdk_esplora::{esplora_client, EsploraAsyncExt};
use bdk_wallet::{
    bitcoin::{self, Address, Amount, ScriptBuf},
    signer::SignOptions,
    KeychainKind, PersistedWallet,
};
use otc_models::Currency;
use snafu::Snafu;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio::task::JoinSet;
use tokio::time::Instant;
use tracing::{error, info};

use super::{BitcoinWalletError, PARALLEL_REQUESTS, STOP_GAP};

const SYNC_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Snafu)]
pub enum TransactionBroadcasterError {
    #[snafu(display("Failed to sync wallet: {}", source))]
    SyncWallet { source: BitcoinWalletError },

    #[snafu(display("Failed to build transaction: {}", source))]
    BuildTransaction { source: BitcoinWalletError },

    #[snafu(display("Failed to sign transaction: {}", source))]
    SignTransaction { source: BitcoinWalletError },

    #[snafu(display("Failed to broadcast transaction: {}", source))]
    BroadcastTransaction { source: BitcoinWalletError },

    #[snafu(display("Transaction broadcaster stopped"))]
    BroadcasterStopped,

    #[snafu(display("Invalid currency"))]
    InvalidCurrency,

    #[snafu(display("Insufficient balance"))]
    InsufficientBalance,

    #[snafu(display("Failed to parse address: {}", reason))]
    ParseAddress { reason: String },
}

pub type Result<T, E = TransactionBroadcasterError> = std::result::Result<T, E>;

pub struct TransactionRequest {
    pub currency: Currency,
    pub to_address: String,
    pub nonce: Option<[u8; 16]>,
    pub response_tx: oneshot::Sender<Result<String>>,
}

pub struct BitcoinTransactionBroadcaster {
    request_tx: mpsc::UnboundedSender<TransactionRequest>,
}

impl BitcoinTransactionBroadcaster {
    pub fn new(
        wallet: Arc<Mutex<PersistedWallet<bdk_wallet::rusqlite::Connection>>>,
        connection: Arc<Mutex<bdk_wallet::rusqlite::Connection>>,
        esplora_client: Arc<esplora_client::AsyncClient>,
        network: bitcoin::Network,
        join_set: &mut JoinSet<crate::Result<()>>,
    ) -> Self {
        let (request_tx, mut request_rx) = mpsc::unbounded_channel::<TransactionRequest>();
        let last_sync = Arc::new(RwLock::new(Instant::now() - SYNC_INTERVAL));

        join_set.spawn(async move {
            info!("Bitcoin transaction broadcaster started");

            while let Some(request) = request_rx.recv().await {
                let result = process_transaction(
                    &wallet,
                    &connection,
                    &esplora_client,
                    network,
                    &last_sync,
                    request.currency,
                    request.to_address,
                    request.nonce,
                )
                .await;

                if let Err(e) = request.response_tx.send(result) {
                    error!("Failed to send transaction response: {:?}", e);
                }
            }

            info!("Bitcoin transaction broadcaster stopped");
            Ok(())
        });

        Self { request_tx }
    }

    pub async fn broadcast_transaction(
        &self,
        currency: Currency,
        to_address: String,
        nonce: Option<[u8; 16]>,
    ) -> Result<String> {
        let (response_tx, response_rx) = oneshot::channel();

        let request = TransactionRequest {
            currency,
            to_address,
            nonce,
            response_tx,
        };

        self.request_tx
            .send(request)
            .map_err(|_| TransactionBroadcasterError::BroadcasterStopped)?;

        response_rx
            .await
            .map_err(|_| TransactionBroadcasterError::BroadcasterStopped)?
    }
}

async fn process_transaction(
    wallet: &Arc<Mutex<PersistedWallet<bdk_wallet::rusqlite::Connection>>>,
    connection: &Arc<Mutex<bdk_wallet::rusqlite::Connection>>,
    esplora_client: &Arc<esplora_client::AsyncClient>,
    network: bitcoin::Network,
    last_sync: &Arc<RwLock<Instant>>,
    currency: Currency,
    to_address: String,
    nonce: Option<[u8; 16]>,
) -> Result<String> {
    let start_time = Instant::now();

    info!(
        "Processing Bitcoin transaction to {} for {:?}",
        to_address, currency
    );

    // Parse the recipient address
    let address = Address::from_str(&to_address)
        .map_err(|e| TransactionBroadcasterError::ParseAddress {
            reason: e.to_string(),
        })?
        .require_network(network)
        .map_err(|_| TransactionBroadcasterError::ParseAddress {
            reason: format!(
                "Address {} is not valid for network {:?}",
                to_address, network
            ),
        })?;

    sync_wallet(wallet, connection, esplora_client, last_sync).await?;

    // Lock wallet for transaction creation
    let mut wallet_guard = wallet.lock().await;

    let address = wallet_guard.peek_address(KeychainKind::External, 0);

    println!("address: {:?}", address);

    // Check balance
    let balance = wallet_guard.balance();
    let amount_sats = currency.amount.to::<u64>();
    let amount = Amount::from_sat(amount_sats);
    info!("balance: {:?}", balance);

    if balance.total() < amount {
        return Err(TransactionBroadcasterError::InsufficientBalance);
    }

    // Build transaction
    let mut tx_builder = wallet_guard.build_tx();
    tx_builder.add_recipient(address.script_pubkey(), amount);

    // Add OP_RETURN output with nonce if provided
    if let Some(nonce) = nonce {
        let op_return_script = create_op_return_script(&nonce);
        tx_builder.add_recipient(op_return_script, Amount::ZERO);
    }

    // Create and sign the transaction
    let build_start = Instant::now();
    let mut psbt =
        tx_builder
            .finish()
            .map_err(|e| TransactionBroadcasterError::BuildTransaction {
                source: BitcoinWalletError::BuildTransaction { source: e },
            })?;
    info!("Transaction built in {:?}", build_start.elapsed());

    let finalized = wallet_guard
        .sign(&mut psbt, SignOptions::default())
        .map_err(|e| TransactionBroadcasterError::SignTransaction {
            source: BitcoinWalletError::SignTransaction { source: e },
        })?;

    if !finalized {
        return Err(TransactionBroadcasterError::BuildTransaction {
            source: BitcoinWalletError::BuildTransaction {
                source: bdk_wallet::error::CreateTxError::NoRecipients,
            },
        });
    }

    // Extract transaction
    let tx = psbt
        .extract_tx()
        .map_err(|e| TransactionBroadcasterError::BuildTransaction {
            source: BitcoinWalletError::ExtractTransaction { source: e },
        })?;

    // Release wallet lock before broadcasting
    drop(wallet_guard);

    // Broadcast the transaction
    let broadcast_start = Instant::now();
    esplora_client.broadcast(&tx).await.map_err(|e| {
        // If broadcast fails, cancel the transaction
        let mut wallet_guard = wallet.blocking_lock();
        wallet_guard.cancel_tx(&tx);
        TransactionBroadcasterError::BroadcastTransaction {
            source: BitcoinWalletError::BroadcastTransaction { source: e },
        }
    })?;
    info!("Transaction broadcast in {:?}", broadcast_start.elapsed());

    let txid = tx.compute_txid().to_string();
    let total_duration = start_time.elapsed();
    info!(
        "Bitcoin transaction created and broadcast successfully: {} (total time: {:?})",
        txid, total_duration
    );

    Ok(txid)
}

async fn sync_wallet(
    wallet: &Arc<Mutex<PersistedWallet<bdk_wallet::rusqlite::Connection>>>,
    connection: &Arc<Mutex<bdk_wallet::rusqlite::Connection>>,
    esplora_client: &Arc<esplora_client::AsyncClient>,
    last_sync: &Arc<RwLock<Instant>>,
) -> Result<()> {
    let sync_start = Instant::now();
    info!("Starting wallet sync");

    let mut wallet_guard = wallet.lock().await;
    let address = wallet_guard.next_unused_address(KeychainKind::External);
    info!("next unused address: {:?}", address);
    let mut conn = connection.lock().await;
    wallet_guard.persist(&mut conn).unwrap();
    drop(conn);

    let request = wallet_guard.start_full_scan().build();
    let update = esplora_client
        .full_scan(request, STOP_GAP, PARALLEL_REQUESTS)
        .await
        .map_err(|e| TransactionBroadcasterError::SyncWallet {
            source: BitcoinWalletError::SyncWallet { source: e },
        })?;

    info!("update: {:?}", update);

    wallet_guard
        .apply_update(update)
        .map_err(|_| TransactionBroadcasterError::SyncWallet {
            source: BitcoinWalletError::ApplyUpdate,
        })?;

    let mut conn = connection.lock().await;
    wallet_guard
        .persist(&mut conn)
        .map_err(|e| TransactionBroadcasterError::SyncWallet {
            source: BitcoinWalletError::PersistWallet { source: e },
        })?;

    // Update last sync time
    *last_sync.write().await = Instant::now();

    info!("Wallet sync completed in {:?}", sync_start.elapsed());
    Ok(())
}

fn create_op_return_script(nonce: &[u8; 16]) -> ScriptBuf {
    bitcoin::blockdata::script::Builder::new()
        .push_opcode(bitcoin::opcodes::all::OP_RETURN)
        .push_slice(nonce)
        .into_script()
}

use std::str::FromStr;
