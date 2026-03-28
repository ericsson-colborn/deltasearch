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
    #[cfg(feature = "metrics")] metrics_port: u16,
) -> Result<()> {
    // Auto-setup: if index doesn't exist, create it from --source
    if !storage.exists(name) {
        let source = source.ok_or_else(|| {
            SearchDbError::IndexNotFound(format!(
                "index '{name}' not found. Use --source <delta-uri> to create it"
            ))
        })?;
        eprintln!("[dewey] compact: index '{name}' not found, creating from Delta source...");
        super::connect_delta::run(storage, name, source, schema_json, false).await?;
    } else if let Some(source) = source {
        // Index exists but --source provided: update the Delta source
        let mut config = storage.load_config(name)?;
        if config.delta_source.as_deref() != Some(source) {
            config.delta_source = Some(source.to_string());
            storage.save_config(name, &config)?;
            eprintln!("[dewey] compact: updated Delta source to {source}");
        }
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
                    eprintln!("\n[dewey] compact: received SIGINT, shutting down gracefully...");
                }
                _ = sigterm.recv() => {
                    eprintln!("[dewey] compact: received SIGTERM, shutting down gracefully...");
                }
            }
        }
        #[cfg(not(unix))]
        {
            ctrl_c.await.ok();
            eprintln!("\n[dewey] compact: received Ctrl+C, shutting down gracefully...");
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
