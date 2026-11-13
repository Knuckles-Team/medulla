//! Assistant profile and document-set store proofs
//! (MEDULLA-DECISION-ENGINE-R656.2): revision compare-and-set, sharing through
//! the RBAC evaluator, the non-disclosing envelope and each refusal, with a
//! cross-tenant negative. Saves go through `admit_put` and the same
//! `AddNode` upsert the gateway commits; reads go through `read_op`.

use super::assistant_profile::{admit_put, read_op, Admitted, ReadOut};
use medulla_graph_core::graph::GraphCore;
use medulla_isolation::isolation::IsolationLayer;
use medulla_server_mutation::mutation::apply_coalescable_write;
use medulla_types::acl::{AgentIdentity, AgentRole, GrantEffect, RbacAction};
use medulla_types::assistant_profile::{
    AssistantProfile, DocumentSet, DocumentSetKind, ModelHints, ProfileOp, ProfileShare,
    ASSISTANT_PROFILE_SCHEMA_VERSION,
};

const SCOPE_A: &str = "scope-a";
const SCOPE_B: &str = "scope-b";

fn isolation() -> IsolationLayer {
    let mut isolation = IsolationLayer::new();
    for (agent_id, roles) in [
        ("alice", vec![]),
        ("bob", vec!["analyst".to_string()]),
        ("carol", vec!["editor".to_string()]),
        ("dave", vec![]),
    ] {
        isolation.register_agent(AgentIdentity {
            agent_id: agent_id.to_string(),
            role: AgentRole::Agent,
            teams: Vec::new(),
            roles,
        });
    }
    isolation
}

fn profile(revision: u64, shares: Vec<ProfileShare>) -> AssistantProfile {
    AssistantProfile {
        profile_id: "p-1".into(),
        tenant: "caller-supplied".into(),
        name: "Research helper".into(),
        instructions: "Answer from the cited documents only.".into(),
        document_set_ids: vec!["set-1".into()],
        tool_allowlist: vec!["web_search".into()],
        model_hints: ModelHints {
            preferred_model_profile_refs: vec![],
            max_context_tokens: None,
        },
        owner_principal: "alice".into(),
        shares,
        revision,
        schema_version: ASSISTANT_PROFILE_SCHEMA_VERSION,
    }
}

fn share(role: &str, action: RbacAction) -> ProfileShare {
    ProfileShare {
        role: role.into(),
        action,
        effect: GrantEffect::Allow,
    }
}

/// Admit and apply one profile save; the refusal text when it is refused.
fn save(
    core: &GraphCore,
    iso: &IsolationLayer,
    scope: &str,
    agent: &str,
    profile: AssistantProfile,
) -> Result<AssistantProfile, String> {
    let op = ProfileOp::PutProfile { profile };
    match admit_put(core, iso, scope, agent, &op)? {
        Admitted::Profile(write, saved) => {
            apply_coalescable_write(core, &write).unwrap();
            Ok(saved)
        }
        Admitted::Set(..) => panic!("a profile save admits a profile"),
    }
}

fn get(
    core: &GraphCore,
    iso: &IsolationLayer,
    scope: &str,
    agent: &str,
    id: &str,
) -> Result<AssistantProfile, String> {
    let op = ProfileOp::GetProfile {
        profile_id: id.into(),
    };
    match read_op(core, iso, scope, agent, &op)? {
        ReadOut::Profile(profile) => Ok(profile),
        other => panic!("{other:?}"),
    }
}

fn list(core: &GraphCore, iso: &IsolationLayer, scope: &str, agent: &str) -> Vec<AssistantProfile> {
    match read_op(core, iso, scope, agent, &ProfileOp::ListProfiles).unwrap() {
        ReadOut::Profiles(profiles) => profiles,
        other => panic!("{other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R656.2
#[test]
fn profile_save_share_and_conflict() {
    let core = GraphCore::new();
    let iso = isolation();

    // A first save is derived from revision 0 and stores revision 1 under the
    // verified tenant, whatever tenant the caller wrote.
    let first = save(&core, &iso, SCOPE_A, "alice", profile(0, vec![])).unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(first.tenant, SCOPE_A);
    assert_eq!(get(&core, &iso, SCOPE_A, "alice", "p-1").unwrap(), first);

    // A profile nobody shared, and a profile that does not exist, answer with
    // one non-disclosing envelope.
    let hidden = get(&core, &iso, SCOPE_A, "bob", "p-1").unwrap_err();
    let absent = get(&core, &iso, SCOPE_A, "bob", "p-missing").unwrap_err();
    assert!(hidden.starts_with("PROFILE_NOT_VISIBLE: "), "{hidden}");
    assert_eq!(
        hidden.replace("p-1", "<id>"),
        absent.replace("p-missing", "<id>"),
    );
    assert!(list(&core, &iso, SCOPE_A, "bob").is_empty());

    // The owner shares Read with analysts: the next revision, readable by bob,
    // who still may not save.
    let shared = save(
        &core,
        &iso,
        SCOPE_A,
        "alice",
        profile(1, vec![share("analyst", RbacAction::Read)]),
    )
    .unwrap();
    assert_eq!(shared.revision, 2);
    assert_eq!(get(&core, &iso, SCOPE_A, "bob", "p-1").unwrap(), shared);
    assert_eq!(list(&core, &iso, SCOPE_A, "bob"), vec![shared.clone()]);
    let denied = save(&core, &iso, SCOPE_A, "bob", profile(2, vec![])).unwrap_err();
    assert!(denied.starts_with("ACCESS_DENIED: "), "{denied}");
    // dave holds no share at all and learns nothing.
    assert!(get(&core, &iso, SCOPE_A, "dave", "p-1")
        .unwrap_err()
        .starts_with("PROFILE_NOT_VISIBLE: "));
    assert!(save(&core, &iso, SCOPE_A, "dave", profile(2, vec![]))
        .unwrap_err()
        .starts_with("PROFILE_NOT_VISIBLE: "));

    // A save derived from a stale revision conflicts and writes nothing.
    let conflict = save(&core, &iso, SCOPE_A, "alice", profile(1, vec![])).unwrap_err();
    assert!(
        conflict.starts_with("PROFILE_REVISION_CONFLICT: "),
        "{conflict}"
    );
    assert_eq!(
        get(&core, &iso, SCOPE_A, "alice", "p-1").unwrap().revision,
        2
    );
    // A save of a revision above zero against nothing is not found.
    let mut other = profile(3, vec![]);
    other.profile_id = "p-2".into();
    assert!(save(&core, &iso, SCOPE_A, "alice", other)
        .unwrap_err()
        .starts_with("PROFILE_NOT_FOUND: "));

    // An Admin grant may save, but the owner never changes.
    let mut with_admin = profile(
        2,
        vec![
            share("analyst", RbacAction::Read),
            share("editor", RbacAction::Admin),
        ],
    );
    with_admin.name = "Renamed".into();
    let admin_saved = save(&core, &iso, SCOPE_A, "alice", with_admin).unwrap();
    assert_eq!(admin_saved.revision, 3);
    let mut by_editor = profile(3, admin_saved.shares.clone());
    by_editor.name = "Edited by an admin".into();
    let edited = save(&core, &iso, SCOPE_A, "carol", by_editor.clone()).unwrap();
    assert_eq!(
        (edited.revision, edited.owner_principal.as_str()),
        (4, "alice")
    );
    by_editor.revision = 4;
    by_editor.owner_principal = "carol".into();
    assert!(save(&core, &iso, SCOPE_A, "carol", by_editor)
        .unwrap_err()
        .starts_with("ACCESS_DENIED: "));

    // Cross-tenant negative: another tenant's callers read and list nothing,
    // and cannot save over the stored node.
    assert!(get(&core, &iso, SCOPE_B, "alice", "p-1")
        .unwrap_err()
        .starts_with("PROFILE_NOT_VISIBLE: "));
    assert!(list(&core, &iso, SCOPE_B, "alice").is_empty());
    assert!(save(&core, &iso, SCOPE_B, "alice", profile(0, vec![]))
        .unwrap_err()
        .starts_with("PROFILE_NOT_VISIBLE: "));
    assert_eq!(get(&core, &iso, SCOPE_A, "alice", "p-1").unwrap(), edited);

    // The instruction limit is the registry knob.
    let mut long = profile(0, vec![]);
    long.profile_id = "p-long".into();
    long.instructions = "x".repeat(8001);
    assert!(save(&core, &iso, SCOPE_A, "alice", long)
        .unwrap_err()
        .starts_with("INVALID_ARGUMENT: "));
}

fn set(kind: DocumentSetKind) -> DocumentSet {
    DocumentSet {
        set_id: "set-1".into(),
        tenant: "caller-supplied".into(),
        name: "Policies".into(),
        kind,
        owner_principal: "alice".into(),
        shares: vec![share("analyst", RbacAction::Read)],
        schema_version: ASSISTANT_PROFILE_SCHEMA_VERSION,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R656.2
#[test]
fn document_set_save_share_and_read_only_query() {
    let core = GraphCore::new();
    let iso = isolation();
    let put = |scope: &str, agent: &str, set: DocumentSet| match admit_put(
        &core,
        &iso,
        scope,
        agent,
        &ProfileOp::PutDocumentSet { set },
    )? {
        Admitted::Set(write, saved) => {
            apply_coalescable_write(&core, &write).unwrap();
            Ok::<_, String>(saved)
        }
        Admitted::Profile(..) => panic!("a set save admits a set"),
    };
    // A saved query that is not a read-only UQL pipeline is refused.
    let refused = put(
        SCOPE_A,
        "alice",
        set(DocumentSetKind::SavedQuery(
            "INSERT INTO x VALUES (1)".into(),
        )),
    )
    .unwrap_err();
    assert!(
        refused.starts_with("DOCUMENT_SET_QUERY_NOT_READ_ONLY: "),
        "{refused}"
    );
    let saved = put(
        SCOPE_A,
        "alice",
        set(DocumentSetKind::Members(vec!["urn:doc:1".into()])),
    )
    .unwrap();
    assert_eq!(saved.tenant, SCOPE_A);
    let read = |scope: &str, agent: &str| {
        read_op(
            &core,
            &iso,
            scope,
            agent,
            &ProfileOp::GetDocumentSet {
                set_id: "set-1".into(),
            },
        )
    };
    assert_eq!(read(SCOPE_A, "bob").unwrap(), ReadOut::Set(saved));
    assert!(read(SCOPE_A, "dave").is_err());
    assert!(read(SCOPE_B, "alice").is_err());
    assert!(put(SCOPE_A, "bob", set(DocumentSetKind::Members(vec![])))
        .unwrap_err()
        .starts_with("ACCESS_DENIED: "));
}
