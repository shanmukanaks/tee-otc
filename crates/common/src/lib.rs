
pub fn handle_background_thread_result<T, E>(
    result: Option<Result<Result<T, E>, tokio::task::JoinError>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    match result {
        Some(Ok(thread_result)) => match thread_result {
            Ok(_) => Err("Background thread completed unexpectedly".into()),
            Err(e) => Err(format!("Background thread panicked: {}", e).into()),
        },
        Some(Err(e)) => Err(format!("Join set failed: {}", e).into()),
        None => Err("Join set panicked with no result".into()),
    }
}

pub fn init_logger(log_level: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(log_level)
        .try_init()
        .map_err(|e| format!("Failed to initialize logger: {}", e))?;
    
    Ok(())
}