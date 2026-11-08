//! Tests of the engine metrics registry (moved out of `metrics.rs` to keep
//! the module under the per-file line cap).
use super::*;

#[test]
fn test_render_contains_recorded_series() {
    let m = KernelMetrics::new();
    m.record_request("Ping", 0.0007);
    m.graph_op("agent:metrics-test");
    m.set_graph_size("agent:metrics-test", 3, 2);
    m.auth_failure();
    m.access_denied();
    m.busy_rejected();
    m.checkpoint_completed(0.02);
    // CONCEPT:MEDULLA-OS.observability.write-lock-gap-histogram — write-lock-gap histograms register + render.
    m.observe_write_lock_wait("agent:metrics-test", 0.003);
    m.observe_write_lock_hold("agent:metrics-test", 0.0005);
    m.set_resource_stats(2, 1024, 2048, 3, 256, 10);

    let out = m.render();
    // Each test owns its value, so no other test shares this registry.
    for needle in [
        "medulla_requests_total{op=\"Ping\"}",
        "medulla_request_duration_seconds_bucket{op=\"Ping\"",
        "medulla_graph_ops_total",
        "medulla_graph_nodes",
        "medulla_graph_edges",
        "medulla_auth_failures_total",
        "medulla_access_denied_total",
        "medulla_busy_rejections_total",
        "medulla_checkpoint_duration_seconds_count",
        "medulla_checkpoint_last_success_timestamp_seconds",
        "medulla_write_lock_wait_seconds_bucket",
        "medulla_write_lock_hold_seconds_bucket",
        "medulla_effective_cpu_cores",
        "medulla_effective_memory_limit_bytes",
        "medulla_process_rss_bytes",
        "medulla_write_coalescer_queue_depth",
        "medulla_write_coalescer_queue_bytes",
        "medulla_write_coalescer_operations_total",
    ] {
        assert!(out.contains(needle), "missing {needle} in:\n{out}");
    }
}

#[test]
fn test_graph_label_cardinality_is_bounded() {
    let m = KernelMetrics::new();
    // Saturate the label space, then confirm new names aggregate.
    for i in 0..200 {
        m.graph_op(&format!("agent:cardinality-{i}"));
    }
    let out = m.render();
    assert!(
        out.contains("medulla_graph_ops_total{graph=\"__overflow__\"}"),
        "expected overflow aggregation past the label cap"
    );
    // Deleting frees the slot and removes the series.
    m.drop_graph("agent:cardinality-0");
    assert!(!m.render().contains("graph=\"agent:cardinality-0\""));

    // Deleting any graph clears the shared overflow series. Recreating a
    // name after all tracked slots are freed must acquire a fresh bounded
    // raw slot rather than inheriting stale `__overflow__` state.
    for i in 1..200 {
        m.drop_graph(&format!("agent:cardinality-{i}"));
    }
    assert!(
        !m.render().contains("graph=\"__overflow__\""),
        "last deleted overflow owner must clear every overflow series"
    );
    m.graph_op("agent:cardinality-recreated");
    let recreated = m.render();
    assert!(
        recreated.contains("graph=\"agent:cardinality-recreated\""),
        "a recreated graph should receive a fresh bounded label slot"
    );
}

#[cfg(feature = "server")]
#[tokio::test]
async fn test_http_exposition_endpoint() {
    let m: &'static KernelMetrics = Box::leak(Box::new(KernelMetrics::new()));
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    m.record_request("Health", 0.001);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(m.serve(listener));

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut out = Vec::new();
    stream.read_to_end(&mut out).await.unwrap();
    let text = String::from_utf8_lossy(&out);
    assert!(text.starts_with("HTTP/1.1 200 OK"), "got: {text}");
    assert!(text.contains("medulla_requests_total"));
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R005.4
/// Verifies MEDULLA-INCREMENTAL-VIEWS-R005.4: Prometheus view metrics
#[test]
fn view_metrics_render_with_bounded_labels() {
    let m = KernelMetrics::new();
    m.view_changes_applied("plan", 3);
    m.view_demoted("plan", "MATVIEW_NOT_INCREMENTAL");
    m.view_shadow_checked("sql", "match");
    m.set_view_state_bytes("plan", 128);
    let out = m.render();
    for needle in [
        "medulla_view_changes_applied_total{family=\"plan\"}",
        "medulla_view_demotions_total{family=\"plan\",reason_code=\"MATVIEW_NOT_INCREMENTAL\"}",
        "medulla_view_shadow_checks_total{family=\"sql\",outcome=\"match\"}",
        "medulla_view_state_bytes{family=\"plan\"}",
        "medulla_views_resident_state_bytes",
    ] {
        assert!(out.contains(needle), "missing {needle} in:\n{out}");
    }
    // Only the family, reason code and outcome label names exist: no view or graph label.
    for line in out.lines().filter(|l| l.starts_with("medulla_view")) {
        assert!(
            !line.contains("view=") && !line.contains("graph="),
            "{line}"
        );
    }
}

// spec: MEDULLA-SDLC-R023
/// Verifies MEDULLA-SDLC-R023: the kernel series are one owned value; two
/// values in one process share no registry, no series and no graph-label slots.
#[test]
fn two_kernel_metrics_values_share_no_series() {
    let first = KernelMetrics::new();
    let second = KernelMetrics::new();
    first.record_request("Ping", 0.001);
    first.access_denied();
    for i in 0..200 {
        first.graph_op(&format!("agent:first-{i}"));
    }
    second.graph_op("agent:second");

    let one = first.render();
    let two = second.render();
    assert!(
        one.contains("medulla_requests_total{op=\"Ping\"} 1"),
        "{one}"
    );
    assert!(one.contains("medulla_access_denied_total 1"), "{one}");
    assert!(!two.contains("medulla_requests_total{op="), "{two}");
    assert!(two.contains("medulla_access_denied_total 0"), "{two}");
    // The first value saturated its own label cap; the second still has slots.
    assert!(one.contains("graph=\"__overflow__\""), "{one}");
    assert!(
        two.contains("medulla_graph_ops_total{graph=\"agent:second\"} 1"),
        "{two}"
    );
    assert!(!two.contains("__overflow__"), "{two}");
}
