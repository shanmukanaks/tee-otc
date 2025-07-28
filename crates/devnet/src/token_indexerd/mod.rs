use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::{net::TcpListener, process::{Child, Command}, time::{sleep, timeout}};
use tracing::{info, warn};


const HOST: &str = "127.0.0.1";

pub struct TokenIndexerInstance {
    // Handle to the spawned pnpm dev process
    pub child: Child,
    pub api_server_url: String
}


impl TokenIndexerInstance {
    pub async fn new(rpc_url: &str, ws_url: &str, pipe_output: bool, chain_id: u64) -> std::io::Result<Self> {
        let workspace_root = std::env::var("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .map(|p| p.ancestors().nth(1).unwrap().to_path_buf()) // Go up 2 levels from crates/devnet
            .unwrap_or_else(|_| PathBuf::from("."));


        let token_indexer_dir = workspace_root.join("evm-token-indexer");

        let listener = TcpListener::bind((HOST, 0)).await.expect("Should be able to bind to port");

        let ponder_port =  listener.local_addr().expect("Should have a local address").port();

        
        let mut cmd = Command::new("pnpm");
        cmd.args(&["dev", "--disable-ui", "--port", ponder_port.to_string().as_str()])
            .current_dir(token_indexer_dir)
            .env("DATABASE_URL", "")
            .env("PONDER_CHAIN_ID", chain_id.to_string())
            .env("PONDER_RPC_URL_HTTP", rpc_url)
            .env("PONDER_WS_URL_HTTP", ws_url)
            .env("PONDER_DISABLE_CACHE", "true")
            .env("PONDER_CONTRACT_START_BLOCK", "0")
            .env("PONDER_LOG_LEVEL", "trace");

        if pipe_output {
            cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        } else {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        }

        let child = cmd.spawn().expect("Failed to spawn token indexer process");
 
        let api_server_url = format!("http://{HOST}:{ponder_port}");
        info!("Indexer API server URL: {api_server_url}");
        
        Ok(Self { child, api_server_url })
    }

    /// Check if the process is still running
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Kill the process
    pub async fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill().await
    }

    /// Wait for the process to finish
    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }
}

impl Drop for TokenIndexerInstance {
    fn drop(&mut self) {
        // Kill the child process when the instance is dropped
        let _ = self.child.start_kill();
    }
}