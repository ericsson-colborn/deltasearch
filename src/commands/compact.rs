use crate::compact::{CompactOptions, CompactWorker};
use crate::error::{Result, SearchDbError};
use crate::storage::Storage;

/// Run the librarian worker with the given CLI options.
///
/// Requires the index to already exist with a Delta source configured.
/// Use `dewey create --source` to set up the index first.
pub async fn run(
    storage: &Storage,
    name: &str,
    opts: CompactOptions,
    #[cfg(feature = "metrics")] metrics_port: u16,
) -> Result<()> {
    if !storage.exists(name) {
        return Err(SearchDbError::IndexNotFound(format!(
            "index '{name}' not found — use 'dewey create' first"
        )));
    }

    let config = storage.load_config(name)?;
    if config.delta_source.is_none() {
        return Err(SearchDbError::Delta(format!(
            "index '{name}' has no Delta source — use 'dewey create --source' first"
        )));
    }

    #[allow(unused_mut)]
    let mut worker = CompactWorker::new(storage, name, opts);

    // Set up Prometheus metrics server if enabled
    #[cfg(feature = "metrics")]
    let _metrics_handle = if metrics_port > 0 {
        let registry = prometheus::Registry::new();
        let metrics = crate::metrics::CompactMetrics::new(&registry);
        worker.set_metrics(metrics);
        let addr = format!("0.0.0.0:{metrics_port}");
        Some(tokio::spawn(crate::metrics::start_metrics_server(
            addr, registry,
        )))
    } else {
        None
    };

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
                    eprintln!("\n[dewey] librarian: received SIGINT, shutting down gracefully...");
                }
                _ = sigterm.recv() => {
                    eprintln!("[dewey] librarian: received SIGTERM, shutting down gracefully...");
                }
            }
        }
        #[cfg(not(unix))]
        {
            ctrl_c.await.ok();
            eprintln!("\n[dewey] librarian: received Ctrl+C, shutting down gracefully...");
        }
        let _ = shutdown_tx.send(true);
    });

    let result = worker.run(shutdown_rx).await;

    // Cancel signal handler if worker finished on its own (--once / --force-merge)
    signal_handle.abort();

    // Cancel metrics server if running
    #[cfg(feature = "metrics")]
    if let Some(handle) = _metrics_handle {
        handle.abort();
    }

    result
}
