use crate::compact::{CompactOptions, CompactWorker};
use crate::error::{Result, SearchDbError};
use crate::storage::Storage;

/// Run the compact worker with the given CLI options.
///
/// If the index doesn't exist and `source` is provided, creates the index
/// and performs an initial full load before starting the compaction loop.
/// This is the "zero to hero" path: one command to go from nothing to searching.
pub async fn run(
    storage: &Storage,
    name: &str,
    opts: CompactOptions,
    source: Option<&str>,
    schema_json: Option<&str>,
) -> Result<()> {
    // Auto-setup: if index doesn't exist, create it from --source
    if !storage.exists(name) {
        let source = source.ok_or_else(|| {
            SearchDbError::IndexNotFound(format!(
                "index '{name}' not found. Use --source <delta-uri> to create it"
            ))
        })?;
        eprintln!("[dsrch] compact: index '{name}' not found, creating from Delta source...");
        super::connect_delta::run(storage, name, source, schema_json, false).await?;
    } else if let Some(source) = source {
        // Index exists but --source provided: update the Delta source
        let mut config = storage.load_config(name)?;
        if config.delta_source.as_deref() != Some(source) {
            config.delta_source = Some(source.to_string());
            storage.save_config(name, &config)?;
            eprintln!("[dsrch] compact: updated Delta source to {source}");
        }
    }

    let worker = CompactWorker::new(storage, name, opts);

    // Set up shutdown signal handler
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Spawn signal handler
    let signal_handle = tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("failed to install SIGTERM handler");
            tokio::select! {
                _ = ctrl_c => {
                    eprintln!("\n[dsrch] compact: received SIGINT, shutting down gracefully...");
                }
                _ = sigterm.recv() => {
                    eprintln!("[dsrch] compact: received SIGTERM, shutting down gracefully...");
                }
            }
        }
        #[cfg(not(unix))]
        {
            ctrl_c.await.ok();
            eprintln!("\n[dsrch] compact: received Ctrl+C, shutting down gracefully...");
        }
        let _ = shutdown_tx.send(true);
    });

    let result = worker.run(shutdown_rx).await;

    // Cancel signal handler if worker finished on its own (--once / --force-merge)
    signal_handle.abort();

    result
}
