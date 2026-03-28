use prometheus::{
    register_counter_with_registry, register_gauge_with_registry, register_histogram_with_registry,
    Counter, Encoder, Gauge, Histogram, Registry, TextEncoder,
};

use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

/// Compact worker metrics, exposed via Prometheus `/metrics` endpoint.
#[derive(Clone)]
pub struct CompactMetrics {
    pub polls_total: Counter,
    pub rows_indexed_total: Counter,
    pub segments_created_total: Counter,
    pub merges_total: Counter,
    pub merge_failures_total: Counter,
    pub documents_deleted_total: Counter,
    pub delta_version: Gauge,
    pub index_version: Gauge,
    pub gap_versions: Gauge,
    pub segments_count: Gauge,
    pub poll_duration_seconds: Histogram,
    pub merge_duration_seconds: Histogram,
}

impl CompactMetrics {
    /// Create a new set of compact metrics registered to the given registry.
    pub fn new(registry: &Registry) -> Self {
        Self {
            polls_total: register_counter_with_registry!(
                "dsrch_compact_polls_total",
                "Total number of Delta polls",
                registry
            )
            .expect("failed to register polls_total"),
            rows_indexed_total: register_counter_with_registry!(
                "dsrch_compact_rows_indexed_total",
                "Total rows indexed",
                registry
            )
            .expect("failed to register rows_indexed_total"),
            segments_created_total: register_counter_with_registry!(
                "dsrch_compact_segments_created_total",
                "Total segments committed",
                registry
            )
            .expect("failed to register segments_created_total"),
            merges_total: register_counter_with_registry!(
                "dsrch_compact_merges_total",
                "Total merge operations",
                registry
            )
            .expect("failed to register merges_total"),
            merge_failures_total: register_counter_with_registry!(
                "dsrch_compact_merge_failures_total",
                "Total failed merges",
                registry
            )
            .expect("failed to register merge_failures_total"),
            documents_deleted_total: register_counter_with_registry!(
                "dsrch_compact_documents_deleted_total",
                "Total documents deleted",
                registry
            )
            .expect("failed to register documents_deleted_total"),
            delta_version: register_gauge_with_registry!(
                "dsrch_compact_delta_version",
                "Current Delta HEAD version",
                registry
            )
            .expect("failed to register delta_version"),
            index_version: register_gauge_with_registry!(
                "dsrch_compact_index_version",
                "Current index version (watermark)",
                registry
            )
            .expect("failed to register index_version"),
            gap_versions: register_gauge_with_registry!(
                "dsrch_compact_gap_versions",
                "Version gap between Delta HEAD and index",
                registry
            )
            .expect("failed to register gap_versions"),
            segments_count: register_gauge_with_registry!(
                "dsrch_compact_segments_count",
                "Current number of segments",
                registry
            )
            .expect("failed to register segments_count"),
            poll_duration_seconds: register_histogram_with_registry!(
                "dsrch_compact_poll_duration_seconds",
                "Time spent per poll cycle in seconds",
                registry
            )
            .expect("failed to register poll_duration_seconds"),
            merge_duration_seconds: register_histogram_with_registry!(
                "dsrch_compact_merge_duration_seconds",
                "Time spent per merge operation in seconds",
                registry
            )
            .expect("failed to register merge_duration_seconds"),
        }
    }
}

/// Handler that renders all metrics in Prometheus text format.
async fn metrics_handler(registry: axum::extract::State<Registry>) -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = registry.gather();
    let mut buffer = Vec::new();
    encoder
        .encode(&metric_families, &mut buffer)
        .expect("failed to encode metrics");
    (
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        buffer,
    )
}

/// Start the Prometheus metrics HTTP server on the given address.
///
/// This function runs until the server is shut down (e.g. via task abort).
pub async fn start_metrics_server(addr: String, registry: Registry) {
    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(registry);

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[dewey] metrics: failed to bind {addr}: {e}");
            return;
        }
    };

    eprintln!("[dewey] metrics: serving on http://{addr}/metrics");
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("[dewey] metrics: server error: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_metrics_creation() {
        let registry = Registry::new();
        let metrics = CompactMetrics::new(&registry);

        metrics.polls_total.inc();
        metrics.rows_indexed_total.inc_by(100.0);
        metrics.delta_version.set(42.0);
        metrics.index_version.set(40.0);
        metrics.gap_versions.set(2.0);
        metrics.segments_count.set(5.0);

        let families = registry.gather();
        assert!(!families.is_empty(), "registry should have metric families");

        // Verify specific metrics exist by name
        let names: Vec<&str> = families.iter().map(|f| f.get_name()).collect();
        assert!(names.contains(&"dsrch_compact_polls_total"));
        assert!(names.contains(&"dsrch_compact_rows_indexed_total"));
        assert!(names.contains(&"dsrch_compact_delta_version"));
        assert!(names.contains(&"dsrch_compact_index_version"));
        assert!(names.contains(&"dsrch_compact_gap_versions"));
        assert!(names.contains(&"dsrch_compact_segments_count"));
        assert!(names.contains(&"dsrch_compact_poll_duration_seconds"));
        assert!(names.contains(&"dsrch_compact_merge_duration_seconds"));
    }

    #[test]
    fn test_compact_metrics_counter_values() {
        let registry = Registry::new();
        let metrics = CompactMetrics::new(&registry);

        metrics.polls_total.inc();
        metrics.polls_total.inc();
        metrics.rows_indexed_total.inc_by(50.0);
        metrics.segments_created_total.inc();
        metrics.merges_total.inc();
        metrics.merge_failures_total.inc();
        metrics.documents_deleted_total.inc_by(10.0);

        assert!((metrics.polls_total.get() - 2.0).abs() < f64::EPSILON);
        assert!((metrics.rows_indexed_total.get() - 50.0).abs() < f64::EPSILON);
        assert!((metrics.segments_created_total.get() - 1.0).abs() < f64::EPSILON);
        assert!((metrics.merges_total.get() - 1.0).abs() < f64::EPSILON);
        assert!((metrics.merge_failures_total.get() - 1.0).abs() < f64::EPSILON);
        assert!((metrics.documents_deleted_total.get() - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compact_metrics_gauge_values() {
        let registry = Registry::new();
        let metrics = CompactMetrics::new(&registry);

        metrics.delta_version.set(100.0);
        metrics.index_version.set(95.0);
        metrics.gap_versions.set(5.0);
        metrics.segments_count.set(3.0);

        assert!((metrics.delta_version.get() - 100.0).abs() < f64::EPSILON);
        assert!((metrics.index_version.get() - 95.0).abs() < f64::EPSILON);
        assert!((metrics.gap_versions.get() - 5.0).abs() < f64::EPSILON);
        assert!((metrics.segments_count.get() - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compact_metrics_histogram_observation() {
        let registry = Registry::new();
        let metrics = CompactMetrics::new(&registry);

        metrics.poll_duration_seconds.observe(0.5);
        metrics.poll_duration_seconds.observe(1.2);
        metrics.merge_duration_seconds.observe(3.0);

        assert_eq!(metrics.poll_duration_seconds.get_sample_count(), 2);
        assert_eq!(metrics.merge_duration_seconds.get_sample_count(), 1);
    }

    #[test]
    fn test_metrics_text_encoding() {
        let registry = Registry::new();
        let metrics = CompactMetrics::new(&registry);

        metrics.polls_total.inc();
        metrics.delta_version.set(42.0);

        let encoder = TextEncoder::new();
        let metric_families = registry.gather();
        let mut buffer = Vec::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();

        let output = String::from_utf8(buffer).unwrap();
        assert!(output.contains("dsrch_compact_polls_total"));
        assert!(output.contains("dsrch_compact_delta_version"));
    }
}
