//! medulla-server-served-tests: End-to-end served tests: the full dispatch chain, handler and wire protocol round trips through the assembled server.
//!
//! Layer F11a (MEDULLA-CONTRACT-R056.11). Every module is a
//! test module: the crate is empty outside the test build.

#![recursion_limit = "256"]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg(test)]

#[cfg(test)]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod tests;

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod split_tests;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;

/// The refusal tap of the served-test harness (MEDULLA-OBSERVE-R001.3).
///
/// A handler that returns an error text without a declared code collapses to `INTERNAL` on the
/// wire; the engine counts it and keeps its cause reference. A test that sends requests through
/// a tap fails when one of its own requests was answered by such a collapse, unless the test
/// declared the text with [`RefusalTap::expect_unclassified`]. The check runs when the tap
/// drops, which is the end of the test, and only for the request ids the tap issued, so tests
/// that run in parallel never fail each other.
pub mod refusal_tap {
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    use medulla_protocol_types::protocol::{unclassified_refusals, CauseRef, UnclassifiedRefusal};

    /// The tap of one test.
    #[derive(Default)]
    pub struct RefusalTap {
        issued: Mutex<BTreeSet<u64>>,
        declared: Mutex<Vec<CauseRef>>,
    }

    impl RefusalTap {
        /// A tap with no issued request.
        pub fn new() -> Self {
            Self::default()
        }

        /// Record a request id this test sent, so a collapse answering it is the test's.
        pub fn issue(&self, request_id: u64) {
            self.issued.lock().expect("tap lock").insert(request_id);
        }

        /// Declare that the test expects the uncoded refusal `text`: its cause reference is no
        /// longer a collapse.
        pub fn expect_unclassified(&self, text: &str) {
            self.declared
                .lock()
                .expect("tap lock")
                .push(CauseRef::of(text));
        }

        /// The uncoded refusals that answered this test's requests and that it did not declare.
        pub fn collapsed(&self) -> Vec<UnclassifiedRefusal> {
            let issued = self.issued.lock().expect("tap lock").clone();
            let declared = self.declared.lock().expect("tap lock").clone();
            unclassified_refusals(|id| issued.contains(&id))
                .into_iter()
                .filter(|refusal| !declared.contains(&refusal.cause))
                .collect()
        }

        /// Fail the test when a refusal collapsed.
        pub fn assert_no_collapse(&self) {
            let collapsed = self.collapsed();
            assert!(
                collapsed.is_empty(),
                "a refusal collapsed to INTERNAL: type it, or declare it with expect_unclassified: {collapsed:#?}"
            );
        }
    }

    impl Drop for RefusalTap {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                self.assert_no_collapse();
            }
        }
    }
}

/// A log capture for the served tests (MEDULLA-OBSERVE-R002.3 and R002.4).
///
/// [`log_capture::install`] sets one process-wide subscriber that keeps every event with the
/// spans it ran inside and the fields of those spans, so a test can ask which request and trace
/// ids a log line carried. It is the only subscriber of the test process: a second install
/// returns the first.
pub mod log_capture {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    /// One span an event ran inside.
    #[derive(Clone, Debug)]
    pub struct CapturedSpan {
        /// The span's name.
        pub name: String,
        /// The span's fields as text.
        pub fields: Vec<(String, String)>,
    }

    impl CapturedSpan {
        /// The text of the field `name`.
        pub fn field(&self, name: &str) -> Option<&str> {
            self.fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        }
    }

    /// One logged event.
    #[derive(Clone, Debug)]
    pub struct CapturedEvent {
        /// The event's target, such as `medulla::transition`.
        pub target: String,
        /// The event's message.
        pub message: String,
        /// The event's own fields as text.
        pub fields: Vec<(String, String)>,
        /// The spans the event ran inside, outermost first.
        pub spans: Vec<CapturedSpan>,
    }

    impl CapturedEvent {
        /// The text of the event field `name`.
        pub fn field(&self, name: &str) -> Option<&str> {
            self.fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        }

        /// The first enclosing span that has the field `name` equal to `value`.
        pub fn span_with(&self, name: &str, value: &str) -> Option<&CapturedSpan> {
            self.spans
                .iter()
                .find(|span| span.field(name) == Some(value))
        }
    }

    #[derive(Default)]
    struct Fields(Vec<(String, String)>, String);

    impl Visit for Fields {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.1 = format!("{value:?}");
            } else {
                self.0
                    .push((field.name().to_string(), format!("{value:?}")));
            }
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "message" {
                self.1 = value.to_string();
            } else {
                self.0.push((field.name().to_string(), value.to_string()));
            }
        }
    }

    struct SpanRecord {
        span: CapturedSpan,
    }

    #[derive(Default)]
    struct Collector {
        next_id: AtomicU64,
        spans: Mutex<HashMap<u64, SpanRecord>>,
        events: Mutex<Vec<CapturedEvent>>,
    }

    thread_local! {
        static ENTERED: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    }

    struct Handle(Arc<Collector>);

    impl Subscriber for Handle {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, attributes: &Attributes<'_>) -> Id {
            let id = self.0.next_id.fetch_add(1, Ordering::Relaxed) + 1;
            let mut fields = Fields::default();
            attributes.record(&mut fields);
            self.0.spans.lock().expect("span table").insert(
                id,
                SpanRecord {
                    span: CapturedSpan {
                        name: attributes.metadata().name().to_string(),
                        fields: fields.0,
                    },
                },
            );
            Id::from_u64(id)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut fields = Fields::default();
            event.record(&mut fields);
            let spans = self.0.spans.lock().expect("span table");
            let chain = ENTERED.with(|stack| {
                stack
                    .borrow()
                    .iter()
                    .filter_map(|id| spans.get(id).map(|record| record.span.clone()))
                    .collect()
            });
            drop(spans);
            self.0
                .events
                .lock()
                .expect("event list")
                .push(CapturedEvent {
                    target: event.metadata().target().to_string(),
                    message: fields.1,
                    fields: fields.0,
                    spans: chain,
                });
        }

        fn enter(&self, span: &Id) {
            ENTERED.with(|stack| stack.borrow_mut().push(span.into_u64()));
        }

        fn exit(&self, span: &Id) {
            ENTERED.with(|stack| {
                let mut stack = stack.borrow_mut();
                if let Some(at) = stack.iter().rposition(|id| *id == span.into_u64()) {
                    stack.remove(at);
                }
            });
        }
    }

    /// The captured log of the process.
    #[derive(Clone)]
    pub struct Capture(Arc<Collector>);

    impl Capture {
        /// Every event captured so far, oldest first.
        pub fn events(&self) -> Vec<CapturedEvent> {
            self.0.events.lock().expect("event list").clone()
        }
    }

    /// Install the capture as the process-wide subscriber, once, and return it.
    pub fn install() -> Capture {
        static INSTALLED: OnceLock<Capture> = OnceLock::new();
        INSTALLED
            .get_or_init(|| {
                let collector = Arc::new(Collector::default());
                tracing::subscriber::set_global_default(Handle(collector.clone()))
                    .expect("no other subscriber is installed in the served-test process");
                Capture(collector)
            })
            .clone()
    }
}
