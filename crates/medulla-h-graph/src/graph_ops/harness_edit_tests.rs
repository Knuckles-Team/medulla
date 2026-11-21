//! Harness edit lifecycle proofs (MEDULLA-DECISION-ENGINE-R173.7): the admission and read
//! rules on one graph holding the edits of two tenants.

use super::harness_edit::{admit_transition, read_op};
use medulla_graph_core::graph::GraphCore;
use medulla_server_mutation::mutation::apply_coalescable_write;
use medulla_types::edit_machine::{EditEvent, EditStatus};

fn put_edit(core: &GraphCore, id: &str, status: &str, tenant: Option<&str>) {
    let mut properties = serde_json::json!({
        "_harness_edit": true,
        "editStatus": status,
        "editRound": 3,
    });
    if let Some(tenant) = tenant {
        properties["tenant_id"] = serde_json::json!(tenant);
    }
    core.add_node(id.to_string(), rmp_serde::to_vec_named(&properties).unwrap());
}

fn apply(core: &GraphCore, tenant: &str, id: &str, event: EditEvent) -> Result<EditStatus, String> {
    let admitted = admit_transition(core, tenant, id, event)?;
    apply_coalescable_write(core, &admitted.write).unwrap();
    Ok(admitted.record.status)
}

// spec: MEDULLA-DECISION-ENGINE-R173.7
#[test]
fn transitions_move_the_status_and_refuse_illegal_pairs() {
    let core = GraphCore::new();
    put_edit(&core, "edit:a", "proposed", Some("tenant-a"));

    assert_eq!(
        apply(&core, "tenant-a", "edit:a", EditEvent::Ship),
        Ok(EditStatus::Shipped)
    );
    assert_eq!(
        read_op(&core, "tenant-a", "edit:a").unwrap().status,
        EditStatus::Shipped
    );
    // The other properties of the node are kept.
    let blob = core.get_node_properties("edit:a").unwrap();
    let properties = medulla_types::msgpack::decode_property_object(&blob).unwrap();
    assert_eq!(properties["editRound"], 3);
    assert_eq!(properties["tenant_id"], "tenant-a");

    // Shipping twice is an illegal pair: refused with the typed record, nothing written.
    let error = apply(&core, "tenant-a", "edit:a", EditEvent::Ship).unwrap_err();
    assert!(error.starts_with("TRANSITION_REFUSED"), "{error}");
    assert!(error.contains("EditMachine"), "{error}");
    assert_eq!(
        read_op(&core, "tenant-a", "edit:a").unwrap().status,
        EditStatus::Shipped
    );

    assert_eq!(
        apply(&core, "tenant-a", "edit:a", EditEvent::Revert),
        Ok(EditStatus::Reverted)
    );
    // Reverted is terminal.
    for event in [EditEvent::Ship, EditEvent::Revert] {
        let error = apply(&core, "tenant-a", "edit:a", event).unwrap_err();
        assert!(error.starts_with("TRANSITION_REFUSED"), "{error}");
        assert!(error.contains("terminal_state"), "{error}");
    }

    // A stored word outside the machine is never repaired.
    put_edit(&core, "edit:legacy", "rejected", None);
    let error = read_op(&core, "tenant-a", "edit:legacy").unwrap_err();
    assert!(error.starts_with("HARNESS_EDIT_STATUS_INVALID"), "{error}");
    let error = apply(&core, "tenant-a", "edit:legacy", EditEvent::Revert).unwrap_err();
    assert!(error.starts_with("HARNESS_EDIT_STATUS_INVALID"), "{error}");
}

// spec: MEDULLA-DECISION-ENGINE-R173.7
#[test]
fn foreign_tenant_edit_reads_as_unknown() {
    let core = GraphCore::new();
    put_edit(&core, "edit:b", "proposed", Some("tenant-b"));
    // A node that carries no tenant belongs to the graph.
    put_edit(&core, "edit:graph", "proposed", None);
    core.add_node(
        "plain:node".to_string(),
        rmp_serde::to_vec_named(&serde_json::json!({"editStatus": "proposed"})).unwrap(),
    );

    for id in ["edit:b", "edit:absent", "plain:node"] {
        let error = read_op(&core, "tenant-a", id).unwrap_err();
        assert!(error.starts_with("HARNESS_EDIT_UNKNOWN"), "{id}: {error}");
        let error = apply(&core, "tenant-a", id, EditEvent::Ship).unwrap_err();
        assert!(error.starts_with("HARNESS_EDIT_UNKNOWN"), "{id}: {error}");
    }
    // The foreign edit is unchanged, and its own tenant moves it.
    assert_eq!(
        read_op(&core, "tenant-b", "edit:b").unwrap().status,
        EditStatus::Proposed
    );
    assert_eq!(
        apply(&core, "tenant-b", "edit:b", EditEvent::Ship),
        Ok(EditStatus::Shipped)
    );
    assert_eq!(
        read_op(&core, "tenant-a", "edit:graph").unwrap().status,
        EditStatus::Proposed
    );
}
