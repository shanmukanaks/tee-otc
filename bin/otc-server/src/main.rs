use clap::Parser;
use otc_server::{run_server, Args, Result};
use snafu::prelude::*;
use std::net::SocketAddr;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

#[derive(Debug, Snafu)]
enum MainError {
    #[snafu(display("Failed to set global subscriber"))]
    SetGlobalSubscriber { source: tracing::subscriber::SetGlobalDefaultError },
    
    #[snafu(display("Server error: {source}"))]
    Server { source: otc_server::Error },
}

#[tokio::main]
async fn main() -> Result<(), MainError> {
    let args = Args::parse();
    
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    
    tracing::subscriber::set_global_default(subscriber)
        .context(SetGlobalSubscriberSnafu)?;
    
    let addr = SocketAddr::from((args.host, args.port));
    
    run_server(addr, &args.database_url).await.context(ServerSnafu)?;
    
    Ok(())
}