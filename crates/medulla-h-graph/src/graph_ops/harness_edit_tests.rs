//! Harness edit lifecycle proofs (MEDULLA-DECISION-ENGINE-R173.7): the admission and read
//! rules on one graph holding the edits of two tenants.

use super::harness_edit::{admit_transition, create_batch, read_op};
use medulla_graph_core::graph::GraphCore;
use medulla_mutation_types::mutation_scope::MutationSurface;
use medulla_protocol_types::protocol::Method;
use medulla_server_api::batch_compile::{compile_methods, CompileBatch};
use medulla_server_mutation::mutation::apply_coalescable_write;
use medulla_types::edit_machine::{
    EditEvent, EditStatus, StatusWriter, EDIT_STATUS_ENGINE_OWNED_MESSAGE,
};

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

fn compile_as(writer: StatusWriter, methods: Vec<Method>) -> Result<(), String> {
    compile_methods(
        CompileBatch {
            batch_id: "edit-batch",
            request_id: 1,
            attempt_nonce: None,
            principal: Some("agent:a"),
            tenant: "tenant-a",
            graph: "graph-a",
            placement_epoch: 0,
            idempotency_key: "idem-edit",
            expected_graph_version: Some(0),
            fencing_token: None,
            created_at_ms: 1,
            default_surface: MutationSurface::Graph,
            authoritative_state: None,
            status_writer: writer,
        },
        methods,
    )
    .map(|_| ())
}

// spec: MEDULLA-DECISION-ENGINE-R173.8
#[test]
fn only_harness_edit_sets_edit_status() {
    let node = |status: &str| Method::AddNode {
        node_id: "edit:new".to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
            "_harness_edit": true,
            "editStatus": status,
        }))
        .unwrap(),
    };
    // A caller's write of the status is refused, whatever the value, and the same write
    // compiles under the edit machine and for a replica.
    for status in ["proposed", "shipped"] {
        assert_eq!(
            compile_as(StatusWriter::Caller, vec![node(status)]).unwrap_err(),
            EDIT_STATUS_ENGINE_OWNED_MESSAGE
        );
        assert!(compile_as(StatusWriter::EditMachine, vec![node(status)]).is_ok());
        assert!(compile_as(StatusWriter::Replicated, vec![node(status)]).is_ok());
    }

    // `Create` writes the edit as `proposed` with its `targetsDimension` edge, in one batch.
    let core = GraphCore::new();
    core.add_node("dim:tokens".to_string(), rmp_serde::to_vec_named(&serde_json::json!({})).unwrap());
    let (write, operations) = create_batch("tenant-a", "edit:new", "dim:tokens", 4, Some(2)).unwrap();
    assert!(compile_as(StatusWriter::EditMachine, vec![write.clone()]).is_ok());
    assert_eq!(
        compile_as(StatusWriter::Caller, vec![write]).unwrap_err(),
        EDIT_STATUS_ENGINE_OWNED_MESSAGE,
        "a caller's batch naming the status is refused too"
    );
    medulla_graph_algos::algorithms::batch_update(&core, &operations).unwrap();
    let record = read_op(&core, "tenant-a", "edit:new").unwrap();
    assert_eq!(record.status, EditStatus::Proposed);
    let properties = medulla_types::msgpack::decode_property_object(
        &core.get_node_properties("edit:new").unwrap(),
    )
    .unwrap();
    assert_eq!(properties["editRound"], 4);
    assert_eq!(properties["policyRevision"], 2);
    assert_eq!(properties["tenant_id"], "tenant-a");
    assert!(core
        .analysis_snapshot()
        .edge_properties
        .contains_key(&("edit:new".to_string(), "dim:tokens".to_string())));

    // The status then moves only by the machine: ship answers `shipped`.
    assert_eq!(
        apply(&core, "tenant-a", "edit:new", EditEvent::Ship),
        Ok(EditStatus::Shipped)
    );
}
