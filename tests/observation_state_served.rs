//! MEDULLA-DECISION-ENGINE-R816: served run-bound observation state. A put
//! stores one record keyed by the verified tenant; a get answers it while it is
//! live; a stale or another tenant's read is refused. Two tenants share one
//! store.
#![cfg(all(feature = "redb", feature = "security"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_kernel::kernel::dispatch_support::authoritative_now_ms;
use medulla_protocol_types::protocol::{Method, Request, Response, ResultPayload};
use medulla_work_types::observation_state::{
    ObservationInput, ObservationKey, ObservationRecord, ObservationStateOp,
};

const SECRET: &str = "observation-state-served-secret";
const OTHER_TENANT: &str = "tenant-b";

fn input(run: &str, revision: u64, expires_at_ms: u64) -> ObservationInput {
    ObservationInput {
        run_id: run.to_string(),
        source: "screen".to_string(),
        frame: 7,
        revision,
        payload_digest: "ab".repeat(32),
        expires_at_ms,
    }
}

fn key(run: &str, revision: u64) -> ObservationKey {
    ObservationKey {
        run_id: run.to_string(),
        source: "screen".to_string(),
        frame: 7,
        revision,
    }
}

fn request(id: u64, tenant: Option<&str>, op: ObservationStateOp) -> Request {
    let method = Method::ObservationState { op: Box::new(op) };
    match tenant {
        None => test_support::commons_request(SECRET, id, method),
        Some(tenant) => {
            let mut context = common::context_for(common::TEST_AGENT);
            context.tenant = tenant.to_string();
            common::signed_request_with_context(SECRET, id, "__commons__", context, method)
        }
    }
}

async fn send(state: &test_support::SharedState, request: Request) -> Response {
    Box::pin(test_support::dispatch(state, request)).await
}

fn record(response: &Response) -> ObservationRecord {
    assert!(
        response.error.is_none(),
        "unexpected refusal: {:?}",
        response.refusal_text()
    );
    match &response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("record"),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn refusal(response: &Response) -> String {
    assert!(response.error.is_some(), "expected a refusal");
    response.refusal_text().unwrap_or_default()
}

// spec: MEDULLA-DECISION-ENGINE-R816
#[tokio::test(flavor = "multi_thread")]
async fn state_is_run_bound() {
    let (dir, persistence) = common::tempdir_persistence();
    let state = test_support::state_with(SECRET, common::current_isolation(), dir, persistence);
    let now = authoritative_now_ms();
    let live = now + 60_000;

    // Put stores the record under the verified tenant; the request names none.
    let stored = record(
        &send(
            &state,
            request(
                1,
                None,
                ObservationStateOp::Put {
                    record: input("run-1", 1, live),
                },
            ),
        )
        .await,
    );
    assert_eq!(stored.tenant_id, common::TEST_TENANT);
    assert_eq!(stored.run_id, "run-1");
    assert_eq!(stored.expires_at_ms, live);

    // A repeat of the same record is accepted unchanged; a different record at
    // the same key is refused.
    let again = send(
        &state,
        request(
            2,
            None,
            ObservationStateOp::Put {
                record: input("run-1", 1, live),
            },
        ),
    )
    .await;
    assert_eq!(record(&again), stored);
    let mut changed = input("run-1", 1, live);
    changed.payload_digest = "cd".repeat(32);
    let conflict = send(
        &state,
        request(3, None, ObservationStateOp::Put { record: changed }),
    )
    .await;
    assert!(refusal(&conflict).contains("PARAMETER_INVALID"));

    // Get returns the live record.
    let got = record(
        &send(
            &state,
            request(
                4,
                None,
                ObservationStateOp::Get {
                    key: key("run-1", 1),
                },
            ),
        )
        .await,
    );
    assert_eq!(got, stored);

    // The same key in another tenant of the same store is a different record:
    // the first tenant's record is refused to it, and its own put does not
    // disturb the first.
    let foreign = send(
        &state,
        request(
            5,
            Some(OTHER_TENANT),
            ObservationStateOp::Get {
                key: key("run-1", 1),
            },
        ),
    )
    .await;
    assert!(refusal(&foreign).contains("VISUAL_STATE_CROSS_TENANT"));
    let other = record(
        &send(
            &state,
            request(
                6,
                Some(OTHER_TENANT),
                ObservationStateOp::Put {
                    record: input("run-1", 1, live + 1_000),
                },
            ),
        )
        .await,
    );
    assert_eq!(other.tenant_id, OTHER_TENANT);
    let first_again = record(
        &send(
            &state,
            request(
                7,
                None,
                ObservationStateOp::Get {
                    key: key("run-1", 1),
                },
            ),
        )
        .await,
    );
    assert_eq!(first_again, stored);

    // An expired record is stale on read. The expiry window is 24 hours either
    // side of the authoritative clock, so a past expiry within it is storable.
    let past = now.saturating_sub(5_000);
    record(
        &send(
            &state,
            request(
                8,
                None,
                ObservationStateOp::Put {
                    record: input("run-1", 2, past),
                },
            ),
        )
        .await,
    );
    let stale = send(
        &state,
        request(
            9,
            None,
            ObservationStateOp::Get {
                key: key("run-1", 2),
            },
        ),
    )
    .await;
    assert!(refusal(&stale).contains("VISUAL_STATE_STALE"));

    // Outside the window, or a malformed key or digest, is PARAMETER_INVALID.
    let far = now + 25 * 60 * 60 * 1000;
    let too_far = send(
        &state,
        request(
            10,
            None,
            ObservationStateOp::Put {
                record: input("run-1", 3, far),
            },
        ),
    )
    .await;
    assert!(refusal(&too_far).contains("PARAMETER_INVALID"));
    let mut bad_digest = input("run-1", 3, live);
    bad_digest.payload_digest = "AB".repeat(32);
    let bad = send(
        &state,
        request(11, None, ObservationStateOp::Put { record: bad_digest }),
    )
    .await;
    assert!(refusal(&bad).contains("PARAMETER_INVALID"));
    let empty_run = send(
        &state,
        request(
            12,
            None,
            ObservationStateOp::Put {
                record: input("", 3, live),
            },
        ),
    )
    .await;
    assert!(refusal(&empty_run).contains("PARAMETER_INVALID"));
    let long_run = "r".repeat(129);
    let too_long = send(
        &state,
        request(
            13,
            None,
            ObservationStateOp::Put {
                record: input(&long_run, 3, live),
            },
        ),
    )
    .await;
    assert!(refusal(&too_long).contains("PARAMETER_INVALID"));

    // An unknown key is the same non-disclosing envelope as a foreign one.
    let unknown = send(
        &state,
        request(
            14,
            None,
            ObservationStateOp::Get {
                key: key("run-none", 1),
            },
        ),
    )
    .await;
    assert!(refusal(&unknown).contains("VISUAL_STATE_CROSS_TENANT"));
}
