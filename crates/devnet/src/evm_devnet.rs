use std::sync::Arc;

use common::create_websocket_wallet_provider;
use eyre::{eyre, Result};
use log::info;
use tokio::time::Instant;

use alloy::{
    node_bindings::{Anvil, AnvilInstance}, primitives::{Address, U256}, providers::{ext::AnvilApi, DynProvider, Provider}
};

use crate::{
    get_new_temp_dir, RiftDevnetCache,
};


/// Holds all Ethereum-related devnet state.
pub struct EthDevnet {
    pub anvil:  Arc<AnvilInstance>,
    pub funded_provider: DynProvider,
    pub deploy_mode: Mode,
    pub anvil_datadir: Option<tempfile::TempDir>,
    pub anvil_dump_path: tempfile::TempDir,
}

#[derive(Clone, Debug)]
pub enum Mode {
    Fork(ForkConfig),
    Local,
}

impl EthDevnet {
    /// Spawns Anvil, deploys the EVM contracts, returns `(Self, deployment_block_number)`.
    pub async fn setup(
        deploy_mode: Mode,
        devnet_cache: Option<Arc<RiftDevnetCache>>,
    ) -> Result<Self> {
        let (anvil, anvil_datadir, anvil_dump_path) =
            spawn_anvil(deploy_mode.clone(), devnet_cache.clone()).await?;
        info!(
            "Anvil spawned at {}, chain_id={}",
            anvil.endpoint(),
            anvil.chain_id()
        );

        let private_key = anvil.keys()[0].clone().to_bytes().into();

        let funded_provider = create_websocket_wallet_provider(
            anvil.ws_endpoint_url().to_string().as_str(),
            private_key,
        )
        .await.map_err(|e| eyre!(e.to_string()))?
        .erased();

        let devnet = EthDevnet {
            anvil: anvil.into(),
            funded_provider,
            deploy_mode,
            anvil_datadir,
            anvil_dump_path,
        };

        Ok(devnet)
    }

    /// Gives `amount_wei` of Ether to `address` (via `anvil_set_balance`).
    pub async fn fund_eth_address(&self, address: Address, amount_wei: U256) -> Result<()> {
        self.funded_provider
            .anvil_set_balance(address, amount_wei)
            .await?;
        Ok(())
    }

    /*
    /// Mints the mock token for `address`.
    pub async fn mint_token(&self, address: Address, amount: U256) -> Result<()> {
        let impersonate_provider = ProviderBuilder::new()
            .connect_http(format!("http://localhost:{}", self.anvil.port()).parse()?);
        if matches!(self.deploy_mode, Mode::Fork(_)) {
            // 1. Get the master minter address
            let master_minter = self.token_contract.masterMinter().call().await?;

            // 2. Configure master minter with maximum minting allowance
            let max_allowance = U256::MAX;
            let configure_minter_calldata = self
                .token_contract
                .configureMinter(master_minter, max_allowance)
                .calldata()
                .clone();

            let tx = TransactionRequest::default()
                .with_from(master_minter)
                .with_to(*self.token_contract.address())
                .with_input(configure_minter_calldata.clone());

            impersonate_provider
                .anvil_impersonate_account(master_minter)
                .await?;

            impersonate_provider
                .send_transaction(tx)
                .await?
                .get_receipt()
                .await?;

            let mint_calldata = self.token_contract.mint(address, amount).calldata().clone();

            let tx = TransactionRequest::default()
                .with_from(master_minter)
                .with_to(*self.token_contract.address())
                .with_input(mint_calldata.clone());

            // 3. Mint tokens as master minter
            impersonate_provider
                .send_transaction(tx)
                .await?
                .get_receipt()
                .await?;
        } else {
            // For local devnet, directly mint tokens
            self.token_contract
                .mint(address, amount)
                .send()
                .await?
                .get_receipt()
                .await?;
        }
        Ok(())
    }

    */
}

#[derive(Clone, Debug)]
pub struct ForkConfig {
    pub url: String,
    pub block_number: Option<u64>,
}

/// Spawns Anvil in a blocking task.
async fn spawn_anvil(
    mode: Mode,
    devnet_cache: Option<Arc<RiftDevnetCache>>,
) -> Result<(AnvilInstance, Option<tempfile::TempDir>, tempfile::TempDir)> {
    let spawn_start = Instant::now();
    // Create or load anvil datafile
    let anvil_datadir = if devnet_cache.is_some() {
        let cache_start = Instant::now();
        let datadir = Some(
            devnet_cache
                .as_ref()
                .unwrap()
                .create_anvil_datadir()
                .await?,
        );
        info!("[Anvil] Created anvil datadir from cache in {:?}", cache_start.elapsed());
        datadir
    } else {
        None
    };

    let anvil_datadir_pathbuf = anvil_datadir.as_ref().map(|dir| dir.path().to_path_buf());

    // get a directory for the --dump-state flag
    let anvil_dump_path = get_new_temp_dir()?;
    let anvil_dump_pathbuf = anvil_dump_path.path().to_path_buf();

    let anvil_instance = tokio::task::spawn_blocking(move || {
        let mut anvil = Anvil::new()
            .arg("--host")
            .arg("0.0.0.0")
            .chain_id(1337)
            .arg("--steps-tracing")
            .arg("--timestamp")
            .arg((chrono::Utc::now().timestamp() - 9 * 60 * 60).to_string()) // 9 hours ago? TODO: do we need to do this?
            .arg("--dump-state")
            .arg(anvil_dump_pathbuf.to_string_lossy().to_string());

        // Load state if file exists and has content - Anvil can handle the file format directly
        if let Some(state_path) = anvil_datadir_pathbuf {
            info!("[Anvil] Loading state from {}", state_path.to_string_lossy());
            anvil = anvil
                .arg("--load-state")
                .arg(state_path.to_string_lossy().to_string());
        }

        match mode {
            Mode::Fork(fork_config) => {
                anvil = anvil.port(50101_u16);
                anvil = anvil.fork(fork_config.url);
                anvil = anvil.block_time(1);
                if let Some(block_number) = fork_config.block_number {
                    anvil = anvil.fork_block_number(block_number);
                }
            }
            Mode::Local => {}
        }
        anvil.try_spawn().map_err(|e| {
            eprintln!("Failed to spawn Anvil: {e:?}");
            eyre!(e)
        })
    })
    .await??;

    info!("[Anvil] Anvil spawned in {:?}", spawn_start.elapsed());

    // print the stdout of the anvil instance
    /*
    let anvil_child = anvil_instance.child_mut();
    let anvil_stdout = anvil_child.stdout.take().unwrap();

    tokio::task::spawn_blocking(move || {
        use std::io::{BufRead, BufReader};

        let stdout_reader = BufReader::new(anvil_stdout);
        for line in stdout_reader.lines().map_while(Result::ok) {
            println!("anvil stdout: {}", line);
        }
    });
    */

    Ok((anvil_instance, anvil_datadir, anvil_dump_path))
}

