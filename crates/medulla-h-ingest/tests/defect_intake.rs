//! MEDULLA-SDLC-R003.3: one filing seats the stored defects of its repository in the defect
//! book and answers the touched defect.

use medulla_h_ingest::defect_intake::file_defect;
use medulla_source_types::lifecycle::{
    DefectFileOutcome, DefectIntake, DefectStatus, FileDefectRequest,
};

fn filing(text: &str, at: u64) -> FileDefectRequest {
    FileDefectRequest {
        repo: "demo".into(),
        intake: DefectIntake::GateFailure,
        title: "gate test failed".into(),
        severity: None,
        source_record_id: format!("gate_verdict:c{at}:test"),
        observed_at: at,
        text: text.into(),
    }
}

// spec: MEDULLA-SDLC-R003.3
#[test]
fn a_filing_opens_then_absorbs_over_the_stored_defects() {
    let first = file_defect(Vec::new(), filing("panic at src/a.rs:42 addr 0x1f", 1));
    assert_eq!(first.outcome, DefectFileOutcome::Created);
    assert_eq!(first.node_id(), "defect:demo/def-0001");
    assert_eq!(first.defect.status, DefectStatus::New);

    // The same signature (line numbers and addresses normalize away) lands on the stored defect.
    let second = file_defect(
        vec![first.defect],
        filing("panic at src/a.rs:99 addr 0xbeef", 2),
    );
    assert_eq!(second.outcome, DefectFileOutcome::Absorbed);
    assert_eq!(second.defect.id, "def-0001");
    assert_eq!(second.receipt().symptoms, 2);

    // A different signature opens the next defect of the repository.
    let third = file_defect(vec![second.defect], filing("a different failure", 3));
    assert_eq!(third.outcome, DefectFileOutcome::Created);
    assert_eq!(third.defect.id, "def-0002");
}
