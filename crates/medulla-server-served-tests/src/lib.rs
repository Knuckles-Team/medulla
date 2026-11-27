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
