//! A process-wide `tracing` layer that records every closed span's name and fields
//! and every event's fields, so served tests can assert the evidence a handler emits.
// Included by `#[path]` into several targets; each uses a subset (the
// `tests/common/test_support.rs` convention).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// One captured span or event: its name and its recorded fields as strings.
#[derive(Debug, Clone, Default)]
pub struct Captured {
    pub name: String,
    pub fields: BTreeMap<String, String>,
    /// For an event: the name of the innermost enclosing span.
    pub parent: Option<String>,
}

#[derive(Default)]
struct Fields(BTreeMap<String, String>);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_owned(), value.to_string());
    }
}

static SPANS: Mutex<Vec<Captured>> = Mutex::new(Vec::new());
static EVENTS: Mutex<Vec<Captured>> = Mutex::new(Vec::new());

struct CaptureLayer;

impl<S> Layer<S> for CaptureLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(fields);
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            if let Some(fields) = span.extensions_mut().get_mut::<Fields>() {
                values.record(fields);
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let parent = ctx.event_span(event).map(|s| s.name().to_owned());
        EVENTS.lock().unwrap().push(Captured {
            name: event.metadata().name().to_owned(),
            fields: fields.0,
            parent,
        });
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id) {
            let fields = span
                .extensions()
                .get::<Fields>()
                .map(|f| f.0.clone())
                .unwrap_or_default();
            SPANS.lock().unwrap().push(Captured {
                name: span.name().to_owned(),
                fields,
                parent: None,
            });
        }
    }
}

/// Install the capture layer as the global default (once per test binary).
pub fn install() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let subscriber = tracing_subscriber::registry().with(CaptureLayer);
        tracing::subscriber::set_global_default(subscriber).expect("span capture installs");
    });
}

/// Closed spans with this name whose `inputs_digest` equals `digest`.
pub fn spans(name: &str, digest: &str) -> Vec<Captured> {
    SPANS
        .lock()
        .unwrap()
        .iter()
        .filter(|s| {
            s.name == name && s.fields.get("inputs_digest").map(String::as_str) == Some(digest)
        })
        .cloned()
        .collect()
}

/// Events emitted inside a span with this name.
pub fn events_in(span_name: &str) -> Vec<Captured> {
    EVENTS
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e.parent.as_deref() == Some(span_name))
        .cloned()
        .collect()
}

/// The digest the served handler records: SHA-256 of the named MessagePack request.
pub fn digest_of<T: serde::Serialize>(method: &T) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(
        rmp_serde::to_vec_named(method).expect("encode"),
    ))
}
