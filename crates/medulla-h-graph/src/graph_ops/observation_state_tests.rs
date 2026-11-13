//! Observation state store proofs (MEDULLA-DECISION-ENGINE-R816). The served
//! proof is `tests/observation_state_served.rs::state_is_run_bound`; these
//! cover the admission and read rules on one graph holding two tenants.

use super::observation_state::{admit_put, read_op, Admitted};
use medulla_graph_core::graph::GraphCore;
use medulla_server_mutation::mutation::apply_coalescable_write;
use medulla_work_types::observation_state::{ObservationInput, ObservationKey};

const NOW: u64 = 1_000_000_000;

fn input(run: &str, revision: u64, expires_at_ms: u64) -> ObservationInput {
    ObservationInput {
        run_id: run.to_string(),
        source: "screen".to_string(),
        frame: 3,
        revision,
        payload_digest: "0f".repeat(32),
        expires_at_ms,
    }
}

fn store(core: &GraphCore, tenant: &str, input: &ObservationInput) {
    let Admitted::Write(write, _) = admit_put(core, tenant, NOW, input).unwrap() else {
        panic!("a first put writes");
    };
    apply_coalescable_write(core, &write).unwrap();
}

// spec: MEDULLA-DECISION-ENGINE-R816
#[test]
fn records_are_keyed_by_tenant_and_expire() {
    let core = GraphCore::new();
    let live = input("run-a", 1, NOW + 1_000);
    store(&core, "tenant-a", &live);
    store(&core, "tenant-b", &input("run-a", 1, NOW + 2_000));

    let key = live.key();
    let a = read_op(&core, "tenant-a", NOW, &key).unwrap();
    assert_eq!(a.tenant_id, "tenant-a");
    assert_eq!(a.expires_at_ms, NOW + 1_000);
    let b = read_op(&core, "tenant-b", NOW, &key).unwrap();
    assert_eq!(b.expires_at_ms, NOW + 2_000);
    // A tenant that stored nothing reads the non-disclosing envelope.
    let error = read_op(&core, "tenant-c", NOW, &key).unwrap_err();
    assert!(error.starts_with("VISUAL_STATE_CROSS_TENANT"), "{error}");
    // Past its expiry the record is stale.
    let error = read_op(&core, "tenant-a", NOW + 1_000, &key).unwrap_err();
    assert!(error.starts_with("VISUAL_STATE_STALE"), "{error}");
}

// spec: MEDULLA-DECISION-ENGINE-R816
#[test]
fn put_validates_shape_window_and_repeats() {
    let core = GraphCore::new();
    let day = 24 * 60 * 60 * 1000;
    for bad in [
        input("", 1, NOW),
        input(&"r".repeat(129), 1, NOW),
        input("run", 1, NOW + day + 1),
        ObservationInput {
            payload_digest: "ZZ".repeat(32),
            ..input("run", 1, NOW)
        },
        ObservationInput {
            source: String::new(),
            ..input("run", 1, NOW)
        },
    ] {
        let error = admit_put(&core, "tenant-a", NOW, &bad).err().unwrap();
        assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
    }
    // The edges of the window are accepted.
    assert!(admit_put(&core, "tenant-a", NOW, &input("run", 1, NOW + day)).is_ok());
    assert!(admit_put(&core, "tenant-a", NOW, &input("run", 1, NOW - day)).is_ok());

    let first = input("run", 2, NOW + 10);
    store(&core, "tenant-a", &first);
    assert!(matches!(
        admit_put(&core, "tenant-a", NOW, &first).unwrap(),
        Admitted::Same(_)
    ));
    let mut changed = first.clone();
    changed.payload_digest = "a1".repeat(32);
    let error = admit_put(&core, "tenant-a", NOW, &changed).err().unwrap();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
    // A read key with an empty run id is malformed, not absent.
    let key = ObservationKey {
        run_id: String::new(),
        source: "screen".to_string(),
        frame: 3,
        revision: 2,
    };
    assert!(read_op(&core, "tenant-a", NOW, &key)
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));
}
