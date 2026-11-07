//! Per-stage spans of the served Decide path (MEDULLA-DECISION-ENGINE-R072.100).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use super::*;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

type Fields = BTreeMap<String, String>;

#[derive(Default)]
struct Collect(Fields);

impl Visit for Collect {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(
            field.name().to_string(),
            format!("{value:?}").replace('"', ""),
        );
    }
}

/// A subscriber that keeps every span's name and final field values.
#[derive(Default)]
struct Capture {
    next: AtomicU64,
    spans: Mutex<BTreeMap<u64, (String, Fields)>>,
}

struct Sink(std::sync::Arc<Capture>);

impl Subscriber for Sink {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, attrs: &Attributes<'_>) -> Id {
        let id = self.0.next.fetch_add(1, Ordering::SeqCst) + 1;
        let mut fields = Collect::default();
        attrs.record(&mut fields);
        self.0
            .spans
            .lock()
            .unwrap()
            .insert(id, (attrs.metadata().name().to_string(), fields.0));
        Id::from_u64(id)
    }
    fn record(&self, id: &Id, values: &Record<'_>) {
        let mut fields = Collect::default();
        values.record(&mut fields);
        if let Some((_, existing)) = self.0.spans.lock().unwrap().get_mut(&id.into_u64()) {
            existing.extend(fields.0);
        }
    }
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, _: &Event<'_>) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

// spec: MEDULLA-DECISION-ENGINE-R072.100
#[tokio::test]
async fn served_decide_emits_one_span_per_stage() {
    // tracing-core treats a process with at most one live registered
    // dispatcher as "just one" and then computes a new callsite's interest
    // from the REGISTERING thread's default only. With this test's scoped
    // subscriber the only live one, a parallel test thread that first hits
    // `medulla.decide.method` caches it as never-enabled, and the span is
    // never created here. A second dispatcher kept alive for the whole test
    // binary keeps every registration on the path that asks all live
    // dispatchers, this one included.
    static ANCHOR: std::sync::LazyLock<tracing::Dispatch> = std::sync::LazyLock::new(|| {
        tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default())
    });
    std::sync::LazyLock::force(&ANCHOR);
    let capture = std::sync::Arc::new(Capture::default());
    let dispatch = tracing::Dispatch::new(Sink(capture.clone()));
    let _guard = tracing::dispatcher::set_default(&dispatch);
    // Callsites registered before this subscriber existed: recompute their
    // interest with it in place.
    tracing::callsite::rebuild_interest_cache();

    let h = Harness::new().await;
    let (schema_pin, _) = publish_route_schema(&h);
    decide_and_assert_abstains(&h, &schema_pin, None).await;

    let spans = capture.spans.lock().unwrap();
    // The stages the Decide path runs; commit, learn and replay belong to the
    // commit and job calls.
    for stage in [
        "admit",
        "visibility",
        "candidates",
        "constraints",
        "entailment",
        "solve",
        "rank",
        "conclude",
        "record",
    ] {
        let name = format!("medulla.decide.{stage}");
        let found: Vec<&Fields> = spans
            .values()
            .filter(|(span, _)| *span == name)
            .map(|(_, fields)| fields)
            .collect();
        assert_eq!(found.len(), 1, "exactly one {name} span");
        let fields = found[0];
        assert_eq!(fields.get("family").map(String::as_str), Some("route"));
        assert_eq!(fields.get("tenant").map(String::as_str), Some(TENANT));
        assert_eq!(fields.get("outcome").map(String::as_str), Some("ok"));
        assert!(
            fields.contains_key("duration_ms"),
            "{name} records its duration"
        );
    }
    assert_eq!(
        spans
            .values()
            .filter(|(name, _)| name == "medulla.decide.method")
            .count(),
        1,
        "the method span stays the parent"
    );
}
