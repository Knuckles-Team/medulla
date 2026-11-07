//! The commuting diagram of the process sources (MEDULLA-CQ-ORCH-R701.12).
//!
//! Three nodes, BPMN, EPC and canonical, and four arrows: BPMN to canonical
//! (`parse_bpmn`, then the stages), EPC to canonical (the adapter's recorded
//! canonical form, then admission), canonical to BPMN (`workflow_to_bpmn`)
//! and canonical to EPC (the export with its losses).

use std::path::{Path, PathBuf};

use medulla_core::process_exchange::bpmn_lower::{bpmn_source_graph, BPMN_KIND_ANNOTATIONS};
use medulla_core::process_exchange::bpmn_read::{parse_bpmn, BpmnLimits};
use medulla_core::process_exchange::workflow_bpmn::workflow_to_bpmn;
use medulla_core::process_import::{
    admit_canonical, compile_process, export_canonical, losses_for,
};
use medulla_types::process_import::{CanonicalWorkflow, ProcessSourceSystem};

const FIXTURES: &[&str] = &[
    "invoice_approval",
    "or_split_join",
    "event_based_wait",
    "boundary_event",
    "default_flow",
    "called_subprocess",
    "decision_step",
];

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process_equivalence")
}

fn read_json(name: &str, suffix: &str) -> CanonicalWorkflow {
    let path = dir().join(format!("{name}{suffix}"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// BPMN to canonical: Parse, then Lower, Allocate and Emit.
fn from_bpmn_bytes(bytes: &[u8]) -> Result<CanonicalWorkflow, String> {
    let doc = parse_bpmn(bytes, BpmnLimits::default()).map_err(|e| e.to_string())?;
    let graph = bpmn_source_graph(&doc).map_err(|e| e.to_string())?;
    compile_process(&graph, BPMN_KIND_ANNOTATIONS).map_err(|e| e.to_string())
}

/// Canonical to BPMN and back to canonical.
fn via_bpmn(workflow: &CanonicalWorkflow) -> Result<CanonicalWorkflow, String> {
    let doc = workflow_to_bpmn(workflow).map_err(|e| e.to_string())?;
    let graph = bpmn_source_graph(&doc).map_err(|e| e.to_string())?;
    compile_process(&graph, BPMN_KIND_ANNOTATIONS).map_err(|e| e.to_string())
}

/// Canonical to EPC and back to canonical.
fn via_epc(workflow: &CanonicalWorkflow) -> Result<CanonicalWorkflow, String> {
    admit_canonical(&export_canonical(workflow, ProcessSourceSystem::ArisEpc))
        .map_err(|e| e.to_string())
}

/// One equality of the diagram; a failure names the fixture, the equality
/// and the two digests.
fn square(fixture: &str, equality: &str, left: &str, right: &str) -> Result<(), String> {
    if left == right {
        Ok(())
    } else {
        Err(format!(
            "fixture {fixture}: equality {equality} does not commute: {left} != {right}"
        ))
    }
}

/// The four equalities over one fixture's BPMN and EPC forms.
fn commutes(
    fixture: &str,
    bpmn: &CanonicalWorkflow,
    epc: &CanonicalWorkflow,
) -> Result<(), String> {
    square(fixture, "(1) BPMN = EPC", &bpmn.digest, &epc.digest)?;
    square(
        fixture,
        "(2) BPMN>C>BPMN>C",
        &bpmn.digest,
        &via_bpmn(bpmn)?.digest,
    )?;
    square(
        fixture,
        "(3) EPC>C>EPC>C",
        &epc.digest,
        &via_epc(epc)?.digest,
    )?;
    square(
        fixture,
        "(4) EPC>C>BPMN>C",
        &epc.digest,
        &via_bpmn(epc)?.digest,
    )?;
    square(
        fixture,
        "(4) BPMN>C>EPC>C",
        &bpmn.digest,
        &via_epc(bpmn)?.digest,
    )
}

// spec: MEDULLA-CQ-ORCH-R701.12
#[test]
fn every_fixture_square_commutes() {
    for &name in FIXTURES {
        let bytes = std::fs::read(dir().join(format!("{name}.bpmn"))).unwrap();
        let bpmn = from_bpmn_bytes(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let golden = read_json(name, ".canonical.json");
        assert_eq!(
            bpmn.steps, golden.steps,
            "{name}: steps equal the golden form"
        );
        assert_eq!(
            bpmn.flows, golden.flows,
            "{name}: flows equal the golden form"
        );
        assert_eq!(
            bpmn.digest, golden.digest,
            "{name}: digest equals the golden form"
        );
        let epc = admit_canonical(&read_json(name, ".epc.canonical.json"))
            .unwrap_or_else(|e| panic!("{name}: EPC form not admitted: {e}"));
        assert_eq!(epc.source.system, ProcessSourceSystem::ArisEpc);
        assert_eq!(
            epc.losses,
            losses_for(&epc, ProcessSourceSystem::ArisEpc),
            "{name}: what EPC cannot express is listed in losses"
        );
        if let Err(defect) = commutes(name, &bpmn, &epc) {
            panic!("{defect}");
        }
    }
}

// spec: MEDULLA-CQ-ORCH-R701.12
#[test]
fn a_broken_square_names_fixture_equality_and_digests() {
    let name = "invoice_approval";
    let bytes = std::fs::read(dir().join(format!("{name}.bpmn"))).unwrap();
    let bpmn = from_bpmn_bytes(&bytes).unwrap();
    let text = String::from_utf8(bytes)
        .unwrap()
        .replace("Approve invoice", "Reject invoice");
    let broken = from_bpmn_bytes(text.as_bytes()).unwrap();
    let defect = commutes(name, &broken, &bpmn).unwrap_err();
    assert!(defect.contains("fixture invoice_approval"), "{defect}");
    assert!(defect.contains("equality (1) BPMN = EPC"), "{defect}");
    assert!(
        defect.contains(&broken.digest) && defect.contains(&bpmn.digest),
        "{defect}"
    );
}
