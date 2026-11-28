//! Assistant profile and document-set store proofs
//! (MEDULLA-DECISION-ENGINE-R656.2): revision compare-and-set, sharing through
//! the RBAC evaluator, the non-disclosing envelope and each refusal, with a
//! cross-tenant negative. Saves go through `admit_put` and the same
//! `AddNode` upsert the gateway commits; reads go through `read_op`.

use super::assistant_profile::{admit_put, read_op, Admitted, ReadOut};
use medulla_graph_core::graph::GraphCore;
use medulla_identity_types::assistant_profile::{
    AssistantProfile, DocumentSet, DocumentSetKind, ModelHints, ProfileOp, ProfileShare,
    ASSISTANT_PROFILE_SCHEMA_VERSION,
};
use medulla_isolation::isolation::IsolationLayer;
use medulla_server_mutation::mutation::apply_coalescable_write;
use {
    medulla_identity_core_types::acl::AgentIdentity, medulla_identity_core_types::acl::AgentRole,
    medulla_identity_core_types::acl::GrantEffect, medulla_identity_core_types::acl::RbacAction,
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

/// Two writers save one profile from the same revision at once. Each admits and
/// commits inside the profile's save lane, as `handle_put` does, with the commit
/// held open across a yield: exactly one wins and the other is refused with
/// PROFILE_REVISION_CONFLICT against the state the winner wrote.
// spec: MEDULLA-DECISION-ENGINE-R656.2
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn racing_saves_of_one_revision_admit_exactly_one() {
    use super::assistant_profile::lock_save_lane;
    let core = std::sync::Arc::new(GraphCore::new());
    let iso = std::sync::Arc::new(isolation());
    save(&core, &iso, SCOPE_A, "alice", profile(0, vec![])).unwrap();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let writers = (0..2).map(|n| {
        let (core, iso, barrier) = (core.clone(), iso.clone(), barrier.clone());
        tokio::spawn(async move {
            let mut next = profile(1, vec![]);
            next.name = format!("writer {n}");
            let op = ProfileOp::PutProfile { profile: next };
            barrier.wait().await;
            let _lane = lock_save_lane("g", &op).await;
            let admitted = admit_put(&core, &iso, SCOPE_A, "alice", &op)?;
            // The commit is still in flight when the other writer is ready.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let Admitted::Profile(write, saved) = admitted else {
                panic!("a profile save admits a profile");
            };
            apply_coalescable_write(&core, &write).unwrap();
            Ok::<_, String>(saved)
        })
    });
    let mut outcomes = Vec::new();
    for writer in writers.collect::<Vec<_>>() {
        outcomes.push(writer.await.unwrap());
    }
    let won: Vec<_> = outcomes.iter().filter_map(|o| o.as_ref().ok()).collect();
    let lost: Vec<_> = outcomes.iter().filter_map(|o| o.as_ref().err()).collect();
    assert_eq!(won.len(), 1, "{outcomes:?}");
    assert_eq!(won[0].revision, 2);
    assert_eq!(lost.len(), 1, "{outcomes:?}");
    assert!(lost[0].contains("PROFILE_REVISION_CONFLICT"), "{}", lost[0]);
    let stored = get(&core, &iso, SCOPE_A, "alice", "p-1").unwrap();
    assert_eq!(stored.revision, 2);
    assert_eq!(stored.name, won[0].name);
}

/// Participant surface proofs (MEDULLA-DECISION-ENGINE-R617.8, R617.9). Each write is admitted by
/// the served handler's admission, then applied as the one write the gateway commits; a refused
/// write changes nothing.
mod participant_surface_proofs {
    use super::super::participant_surface::{admit, plan_memberships, scope_max_sessions, Out};
    use super::{SCOPE_A, SCOPE_B};
    use medulla_graph_core::graph::GraphCore;
    use medulla_protocol_types::protocol::Method;
    use medulla_server_auth::access::requires_write;
    use medulla_server_mutation::mutation::apply_coalescable_write;
    use medulla_server_mutation::mutation::plan::GATEWAY_ROUTED;
    use medulla_types::contract::{ResourceId, TenantId};
    use medulla_work_types::memory::{
        ConversationMessage, ConversationRecord, MessageRole, ThreadAnchor,
    };
    use medulla_work_types::memory_scope::{MemoryScope, ObservationConfig};
    use medulla_work_types::participant::{
        participant_iri, Participant, ParticipantId, ParticipantKind, ParticipantOp,
        PARTICIPANT_SCHEMA_VERSION,
    };

    fn participant(slug: &str, represents: &str, acts_as: &[&str], name: &str) -> Participant {
        Participant {
            participant_id: ParticipantId::new(slug).unwrap(),
            tenant: TenantId::new("caller-supplied").unwrap(),
            workspace: ResourceId::new("ws-main").unwrap(),
            kind: ParticipantKind::Person,
            display_name: name.into(),
            represents: Some(represents.into()),
            acts_as: acts_as.iter().map(|a| a.to_string()).collect(),
            schema_version: PARTICIPANT_SCHEMA_VERSION,
        }
    }

    /// Admit and apply one operation; the refusal text when it is refused, and whether it
    /// committed a write.
    fn serve(
        core: &GraphCore,
        scope: &str,
        agent: &str,
        op: ParticipantOp,
    ) -> Result<(Out, bool), String> {
        let admitted = admit(core, scope, agent, &op)?;
        let committed = admitted.write.is_some();
        if let Some(write) = &admitted.write {
            apply_coalescable_write(core, write).unwrap();
        }
        Ok((admitted.out, committed))
    }

    /// A committed transcript thread node, as ingest writes it.
    fn thread(core: &GraphCore, session: &str, connector: &str) -> String {
        let id = format!("transcript-thread:{session}");
        core.add_node(
            id.clone(),
            rmp_serde::to_vec_named(&serde_json::json!({
                "type": "Transcript",
                "connector": connector,
                "session_id": session,
            }))
            .unwrap(),
        );
        id
    }

    fn scope_of(id: &str, owner: &str, sessions: &[&str]) -> MemoryScope {
        MemoryScope {
            scope_id: id.into(),
            tenant: "caller-supplied".into(),
            name: "work".into(),
            owner_principal: owner.into(),
            session_ids: sessions.iter().map(|s| s.to_string()).collect(),
            shared_with: Default::default(),
            schema_version: 1,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R617.9
    #[test]
    fn participant_methods_are_served_and_classified() {
        let method = Method::Participant {
            op: Box::new(ParticipantOp::Put {
                participant: participant("ada-1", "urn:person:ada", &["alice"], "Ada"),
            }),
        };
        assert!(requires_write(&method));
        assert!(GATEWAY_ROUTED.contains(&"Participant"));

        let core = GraphCore::new();
        let put = |name: &str, acts: &[&str], agent: &str, scope: &str| {
            serve(
                &core,
                scope,
                agent,
                ParticipantOp::Put {
                    participant: participant("ada-1", "urn:person:ada", acts, name),
                },
            )
        };
        // Created under the verified tenant, whatever tenant the caller wrote.
        let (out, committed) = put("Ada", &["alice"], "alice", SCOPE_A).unwrap();
        let Out::Participant(created) = out else {
            panic!("a put answers a participant")
        };
        assert!(committed);
        assert_eq!(created.tenant.as_str(), SCOPE_A);

        // Replay of an equal body commits nothing.
        let (_, committed) = put("Ada", &["alice"], "alice", SCOPE_A).unwrap();
        assert!(!committed);

        // A different body updates the name and acts_as only.
        let (out, committed) = put("Ada L.", &["alice", "bob"], "alice", SCOPE_A).unwrap();
        let Out::Participant(updated) = out else {
            panic!("a put answers a participant")
        };
        assert!(committed);
        assert_eq!(updated.display_name, "Ada L.");
        assert_eq!(
            updated.acts_as,
            vec!["alice".to_string(), "bob".to_string()]
        );

        // The kind and represents are immutable.
        let mut other_kind = participant("ada-1", "urn:person:ada", &["alice"], "Ada");
        other_kind.kind = ParticipantKind::Agent;
        let refused = serve(
            &core,
            SCOPE_A,
            "alice",
            ParticipantOp::Put {
                participant: other_kind,
            },
        )
        .err()
        .unwrap();
        assert!(
            refused.starts_with("PARTICIPANT_KIND_INVALID: "),
            "{refused}"
        );

        // Cross-tenant negative: another tenant neither updates it nor overwrites its node, and
        // the refusal is the non-disclosing envelope; so is a principal that does not act as it.
        let foreign = put("Hijack", &["mallory"], "mallory", SCOPE_B).unwrap_err();
        assert!(
            foreign.starts_with("PARTICIPANT_NOT_VISIBLE: "),
            "{foreign}"
        );
        let not_acting = put("Hijack", &["mallory"], "mallory", SCOPE_A).unwrap_err();
        assert!(
            not_acting.starts_with("PARTICIPANT_NOT_VISIBLE: "),
            "{not_acting}"
        );

        // AddMember: needs a session with a thread; idempotent on the pair.
        let join = |session: &str, scope: &str, agent: &str| {
            serve(
                &core,
                scope,
                agent,
                ParticipantOp::AddMember {
                    session_id: session.into(),
                    participant_id: ParticipantId::new("ada-1").unwrap(),
                },
            )
        };
        let unknown = join("chat-9", SCOPE_A, "alice").unwrap_err();
        assert!(
            unknown.starts_with("MEMBERSHIP_SESSION_UNKNOWN: "),
            "{unknown}"
        );
        let thread_id = thread(&core, "chat-9", "ws-main");
        let iri = participant_iri(
            &ResourceId::new("ws-main").unwrap(),
            &ParticipantId::new("ada-1").unwrap(),
        );
        let (_, committed) = join("chat-9", SCOPE_A, "alice").unwrap();
        assert!(committed);
        assert!(core.has_edge(&thread_id, &iri));
        let (_, committed) = join("chat-9", SCOPE_A, "alice").unwrap();
        assert!(!committed);
        // The other tenant cannot add the participant.
        let other_tenant = join("chat-9", SCOPE_B, "alice").unwrap_err();
        assert!(
            other_tenant.starts_with("PARTICIPANT_NOT_VISIBLE: "),
            "{other_tenant}"
        );

        // PutScope: the owner saves; another principal cannot change it; the session limit comes
        // from the knob.
        let (out, _) = serve(
            &core,
            SCOPE_A,
            "alice",
            ParticipantOp::PutScope {
                scope: scope_of("sc-1", "alice", &["chat-9"]),
            },
        )
        .unwrap();
        let Out::Scope(saved) = out else {
            panic!("a scope put answers a scope")
        };
        assert_eq!(saved.tenant, SCOPE_A);
        let not_owner = serve(
            &core,
            SCOPE_A,
            "bob",
            ParticipantOp::PutScope {
                scope: scope_of("sc-1", "bob", &["chat-9"]),
            },
        )
        .err()
        .unwrap();
        assert!(not_owner.starts_with("SCOPE_NOT_VISIBLE: "), "{not_owner}");
        let too_many: Vec<String> = (0..=scope_max_sessions())
            .map(|n| format!("s{n}"))
            .collect();
        let refs: Vec<&str> = too_many.iter().map(String::as_str).collect();
        let limit = serve(
            &core,
            SCOPE_A,
            "alice",
            ParticipantOp::PutScope {
                scope: scope_of("sc-2", "alice", &refs),
            },
        )
        .err()
        .unwrap();
        assert!(limit.starts_with("SCOPE_SESSION_LIMIT: "), "{limit}");

        // SetObservation: a principal acting as the participant, or the scope owner of the
        // session, may set it; anyone else is refused.
        let config = ObservationConfig {
            session_id: "chat-9".into(),
            participant_id: ParticipantId::new("ada-1").unwrap(),
            observe_self: true,
            observe_others: true,
        };
        let set = |agent: &str| {
            serve(
                &core,
                SCOPE_A,
                agent,
                ParticipantOp::SetObservation {
                    config: config.clone(),
                },
            )
        };
        assert!(set("bob").is_ok());
        let stranger = set("carol").unwrap_err();
        assert!(
            stranger.starts_with("OBSERVER_NOT_PERMITTED: "),
            "{stranger}"
        );
        let (_, committed) = set("alice").unwrap();
        assert!(!committed);
    }

    fn two_speaker_thread(tenant: &str) -> ConversationRecord {
        let turn = |id: &str, speaker: &str, at: i64| ConversationMessage {
            message_id: ResourceId::new(id).unwrap(),
            role: MessageRole::User,
            provenance_key: ResourceId::new(format!("src:{id}")).unwrap(),
            timestamp_ms: at,
            text: format!("turn {id}"),
            speaker: Some(ParticipantId::new(speaker).unwrap()),
        };
        let mut conversation = ConversationRecord::new(
            tenant,
            "chat-9",
            vec![
                turn("m1", "ada-1", 10),
                turn("m2", "bo-2", 20),
                turn("m3", "ada-1", 30),
            ],
        )
        .unwrap();
        conversation.thread_anchor = Some(ThreadAnchor {
            thread_node_id: "transcript-thread:chat-9".into(),
            workspace: ResourceId::new("ws-main").unwrap(),
        });
        conversation
    }

    // spec: MEDULLA-DECISION-ENGINE-R617.8
    #[test]
    fn ingest_adds_speakers_as_members() {
        let core = GraphCore::new();
        let thread_id = thread(&core, "chat-9", "ws-main");
        for (slug, name) in [("ada-1", "Ada"), ("bo-2", "Bo")] {
            serve(
                &core,
                SCOPE_A,
                "alice",
                ParticipantOp::Put {
                    participant: participant(slug, &format!("urn:person:{slug}"), &["alice"], name),
                },
            )
            .unwrap();
        }
        let conversation = two_speaker_thread(SCOPE_A);
        let anchor = conversation.thread_anchor.clone().unwrap();
        // Two distinct speakers write two edges in one batch.
        let (write, _) = plan_memberships(&core, SCOPE_A, &anchor, &conversation)
            .unwrap()
            .unwrap();
        let Method::BatchUpdate { operations_msgpack } = &write else {
            panic!("memberships are one batch")
        };
        medulla_graph_algos::algorithms::batch_update(&core, operations_msgpack).unwrap();
        for slug in ["ada-1", "bo-2"] {
            let iri = participant_iri(&anchor.workspace, &ParticipantId::new(slug).unwrap());
            assert!(core.has_edge(&thread_id, &iri), "{slug}");
        }
        // A replay writes nothing: every speaker is already a member.
        assert!(plan_memberships(&core, SCOPE_A, &anchor, &conversation)
            .unwrap()
            .is_none());

        // Each record carries its speaker.
        let embeddings = vec![vec![0.0f32]; conversation.messages.len()];
        let requests =
            super::super::memory_ingest::thread_requests(&conversation, embeddings, 30 * 60_000)
                .unwrap();
        let speakers: Vec<Option<&str>> = requests
            .iter()
            .map(|request| request.record.speaker.as_ref().map(ParticipantId::as_str))
            .collect();
        assert_eq!(speakers, vec![Some("ada-1"), Some("bo-2"), Some("ada-1")]);

        // Cross-tenant negative: another tenant finds neither participant, so no edge is planned
        // and the refusal is the one an absent speaker gets.
        let other = two_speaker_thread(SCOPE_B);
        let refused = plan_memberships(&GraphCore::new(), SCOPE_B, &anchor, &other).unwrap_err();
        assert!(
            refused.starts_with("PARTICIPANT_NOT_VISIBLE: "),
            "{refused}"
        );
        let foreign = plan_memberships(&core, SCOPE_B, &anchor, &other).unwrap_err();
        assert_eq!(foreign, refused);
    }
}

/// Memory scope proofs (MEDULLA-DECISION-ENGINE-R651.9, MEDULLA-DECISION-ENGINE-R155.7): a scope
/// narrows recall to the sessions it names among the sessions the caller already sees, and the
/// tenant filter keeps applying.
mod scoped_recall_proofs {
    use super::super::memory_surface::{recall, Narrowing};
    use super::super::memory_surface_tests::{
        episode_recall, ids, recall_request, record, seal, store, NOW,
    };
    use super::super::participant_surface::{admit, resolve_scope_ref};
    use super::{SCOPE_A, SCOPE_B};
    use medulla_graph_core::graph::GraphCore;
    use medulla_server_mutation::mutation::apply_coalescable_write;
    use medulla_types::contract::ResourceId;
    use medulla_work_types::memory::{EpisodeScope, MemoryRememberRequest, MemoryTier};
    use medulla_work_types::memory_scope::{scope_node_id, MemoryScope};
    use medulla_work_types::participant::ParticipantOp;
    use std::collections::BTreeSet;

    fn scope(id: &str, owner: &str, sessions: &[&str]) -> MemoryScope {
        MemoryScope {
            scope_id: id.into(),
            tenant: "caller-supplied".into(),
            name: "work".into(),
            owner_principal: owner.into(),
            session_ids: sessions.iter().map(|s| s.to_string()).collect(),
            shared_with: BTreeSet::new(),
            schema_version: 1,
        }
    }

    fn put_scope(core: &GraphCore, tenant: &str, agent: &str, scope: MemoryScope) {
        let admitted = admit(core, tenant, agent, &ParticipantOp::PutScope { scope }).unwrap();
        apply_coalescable_write(core, admitted.write.as_ref().unwrap()).unwrap();
    }

    fn reference(id: &str) -> medulla_work_types::memory_scope::ScopeRef {
        medulla_work_types::memory_scope::ScopeRef {
            scope_id: id.into(),
        }
    }

    /// The sessions the caller's own view holds.
    fn visible(_core: &GraphCore, _tenant: &str) -> BTreeSet<String> {
        ["s1", "s2", "s3"]
            .iter()
            .map(|session| session.to_string())
            .collect()
    }

    fn in_session(tenant: &str, id: &str, session: &str) -> MemoryRememberRequest {
        let mut item = record(tenant, id, MemoryTier::Episodic, "note of the session");
        item.session_id = Some(ResourceId::new(session).unwrap());
        MemoryRememberRequest {
            record: item,
            embedding: vec![],
            episode: None,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.9
    #[test]
    fn scope_narrows_recall_and_acl_still_applies() {
        let core = GraphCore::new();
        for (id, session) in [("m1", "s1"), ("m2", "s2"), ("m3", "s3")] {
            store(&core, SCOPE_A, &in_session(SCOPE_A, id, session));
        }
        store(&core, SCOPE_B, &in_session(SCOPE_B, "foreign", "s1"));
        put_scope(
            &core,
            SCOPE_A,
            "alice",
            scope("work", "alice", &["s1", "s3", "s9"]),
        );

        let search = |tenant: &str, sessions: BTreeSet<String>| {
            let narrowing = Narrowing {
                documents: None,
                sessions: Some(sessions),
            };
            recall(
                &core,
                tenant,
                NOW,
                &recall_request(vec![], &["note"], 1_000),
                Some(&narrowing),
            )
            .unwrap()
        };
        // The scope intersects what the caller sees: s9 is named but not visible, s2 is visible
        // but not named.
        let allowed = resolve_scope_ref(
            &core,
            SCOPE_A,
            "alice",
            &reference("work"),
            &visible(&core, SCOPE_A),
        )
        .unwrap();
        assert_eq!(allowed.iter().cloned().collect::<Vec<_>>(), ["s1", "s3"]);
        let mut held = ids(&search(SCOPE_A, allowed.clone()).hits)
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        held.sort();
        assert_eq!(held, ["m1", "m3"]);
        // A scope that names nothing the caller sees returns an empty answer and never widens.
        put_scope(&core, SCOPE_A, "alice", scope("nowhere", "alice", &["s9"]));
        let nothing = resolve_scope_ref(
            &core,
            SCOPE_A,
            "alice",
            &reference("nowhere"),
            &visible(&core, SCOPE_A),
        )
        .unwrap();
        assert!(nothing.is_empty());
        assert!(search(SCOPE_A, nothing).hits.is_empty());
        // The tenant filter still applies: the other tenant's record in s1 is never returned.
        assert!(ids(&search(SCOPE_A, allowed).hits)
            .iter()
            .all(|id| *id != "foreign"));

        // The three scope refusals.
        let code = |result: Result<BTreeSet<String>, String>| result.unwrap_err();
        let visible_now = visible(&core, SCOPE_A);
        assert!(code(resolve_scope_ref(
            &core,
            SCOPE_A,
            "alice",
            &reference("absent"),
            &visible_now
        ))
        .starts_with("SCOPE_NOT_FOUND: "));
        assert!(code(resolve_scope_ref(
            &core,
            SCOPE_A,
            "bob",
            &reference("work"),
            &visible_now
        ))
        .starts_with("SCOPE_NOT_VISIBLE: "));
        // A stored scope above the knob limit refuses on read.
        let over: Vec<String> = (0..=super::super::participant_surface::scope_max_sessions())
            .map(|n| format!("x{n}"))
            .collect();
        let mut huge = scope("huge", "alice", &[]);
        huge.session_ids = over.into_iter().collect();
        huge.tenant = SCOPE_A.into();
        core.add_node(
            scope_node_id(SCOPE_A, "huge"),
            rmp_serde::to_vec_named(&serde_json::json!({
                "type": "MemoryScope",
                "tenant_scope": SCOPE_A,
                "json": serde_json::to_string(&huge).unwrap(),
            }))
            .unwrap(),
        );
        assert!(code(resolve_scope_ref(
            &core,
            SCOPE_A,
            "alice",
            &reference("huge"),
            &visible_now
        ))
        .starts_with("SCOPE_SESSION_LIMIT: "));
        // Cross-tenant negative: the same scope id under another tenant is a different node, so
        // the first tenant's scope is absent to it.
        assert!(code(resolve_scope_ref(
            &core,
            SCOPE_B,
            "alice",
            &reference("work"),
            &visible_now
        ))
        .starts_with("SCOPE_NOT_FOUND: "));
    }

    // spec: MEDULLA-DECISION-ENGINE-R155.7
    #[test]
    fn scoped_recall_returns_only_scope_episodes() {
        let core = GraphCore::new();
        seal(&core, SCOPE_A, "ep:1", "s1", &[("e1", 100), ("e2", 200)]);
        seal(&core, SCOPE_A, "ep:2", "s2", &[("f1", 300), ("f2", 400)]);
        seal(&core, SCOPE_A, "ep:3", "s3", &[("g1", 500), ("g2", 600)]);
        seal(&core, SCOPE_B, "ep:b", "s1", &[("h1", 150), ("h2", 250)]);
        put_scope(
            &core,
            SCOPE_A,
            "alice",
            scope("two", "alice", &["s1", "s3"]),
        );
        let allowed = resolve_scope_ref(
            &core,
            SCOPE_A,
            "alice",
            &reference("two"),
            &visible(&core, SCOPE_A),
        )
        .unwrap();
        let mut request = episode_recall(EpisodeScope::Episodes, 10_000);
        request.memory_scope = Some(reference("two"));
        let narrowing = Narrowing {
            documents: None,
            sessions: Some(allowed),
        };
        let result = recall(&core, SCOPE_A, 20_000, &request, Some(&narrowing)).unwrap();
        let mut episodes: Vec<&str> = result
            .episodes
            .iter()
            .map(|e| e.episode_id.as_str())
            .collect();
        episodes.sort();
        assert_eq!(episodes, ["ep:1", "ep:3"]);
        let mut held = ids(&result.hits)
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        held.sort();
        assert_eq!(held, ["e1", "e2", "g1", "g2"]);
        // The result lists the scope id and the session count after narrowing.
        assert_eq!(result.memory_scope.as_deref(), Some("two"));
        assert_eq!(result.scope_session_count, Some(2));
        // Cross-tenant negative: another tenant's episode of s1 never appears for tenant A.
        assert!(result
            .episodes
            .iter()
            .all(|e| e.episode_id.as_str() != "ep:b"));
    }
}

/// Observation flag proofs (MEDULLA-DECISION-ENGINE-R651.10): the stored settings of a session
/// decide which perspectives its turns are derived from.
mod observation_flag_proofs {
    use super::super::participant_surface::{admit, observation_of};
    use super::SCOPE_A;
    use medulla_graph_core::graph::GraphCore;
    use medulla_server_mutation::mutation::apply_coalescable_write;
    use medulla_types::contract::{ResourceId, TenantId};
    use medulla_work_types::memory_scope::ObservationConfig;
    use medulla_work_types::participant::{
        Participant, ParticipantId, ParticipantKind, ParticipantOp, PARTICIPANT_SCHEMA_VERSION,
    };
    use {
        medulla_decision_stats_records::decision::participant_derive::derivation_targets,
        medulla_decision_stats_records::decision::participant_derive::DerivationInput,
    };

    fn id(slug: &str) -> ParticipantId {
        ParticipantId::new(slug).unwrap()
    }

    fn serve(core: &GraphCore, agent: &str, op: ParticipantOp) {
        let admitted = admit(core, SCOPE_A, agent, &op).unwrap();
        if let Some(write) = &admitted.write {
            apply_coalescable_write(core, write).unwrap();
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.10
    #[test]
    fn observation_flags_decide_which_perspectives_exist() {
        let core = GraphCore::new();
        core.add_node(
            "transcript-thread:chat-9".into(),
            rmp_serde::to_vec_named(&serde_json::json!({
                "type": "Transcript", "connector": "ws-main", "session_id": "chat-9",
            }))
            .unwrap(),
        );
        let members = [id("ada-1"), id("bo-2")];
        for member in &members {
            serve(
                &core,
                "alice",
                ParticipantOp::Put {
                    participant: Participant {
                        participant_id: member.clone(),
                        tenant: TenantId::new("caller").unwrap(),
                        workspace: ResourceId::new("ws-main").unwrap(),
                        kind: ParticipantKind::Person,
                        display_name: member.as_str().to_string(),
                        represents: Some(format!("person:{}", member.as_str())),
                        acts_as: vec!["alice".into()],
                        schema_version: PARTICIPANT_SCHEMA_VERSION,
                    },
                },
            );
        }
        let count = |core: &GraphCore| {
            let configs: Vec<ObservationConfig> = members
                .iter()
                .map(|member| observation_of(core, SCOPE_A, "chat-9", member))
                .collect();
            derivation_targets(&DerivationInput {
                scope_id: "",
                session_id: "chat-9",
                members: &members,
                configs: &configs,
            })
            .len()
        };
        // Nothing stored: the system view of both speakers and each one's own view.
        assert_eq!(count(&core), 4);
        let set = |observe_self: bool, observe_others: bool| {
            serve(
                &core,
                "alice",
                ParticipantOp::SetObservation {
                    config: ObservationConfig {
                        session_id: "chat-9".into(),
                        participant_id: id("ada-1"),
                        observe_self,
                        observe_others,
                    },
                },
            );
        };
        // Flipping each flag of one member adds or removes exactly its perspectives.
        set(true, true);
        assert_eq!(count(&core), 5);
        set(false, true);
        assert_eq!(count(&core), 4);
        set(false, false);
        assert_eq!(count(&core), 3);
        set(true, false);
        assert_eq!(count(&core), 4);
        // A setting of another session never applies to this one.
        assert!(observation_of(&core, SCOPE_A, "other-session", &id("ada-1")).observe_self);
    }
}

// spec: MEDULLA-DECISION-ENGINE-R656.5
#[test]
fn profile_refusals_change_no_state() {
    // The six codes are registered once, in the participant table.
    let registered: Vec<&str> = medulla_types::participant::REFUSALS
        .iter()
        .map(|code| code.as_str())
        .collect();
    for code in [
        "PROFILE_NOT_FOUND",
        "PROFILE_NOT_VISIBLE",
        "PROFILE_REVISION_CONFLICT",
        "PROFILE_TOOL_NOT_ALLOWED",
        "DOCUMENT_SET_NOT_FOUND",
        "DOCUMENT_SET_QUERY_NOT_READ_ONLY",
    ] {
        assert_eq!(
            registered.iter().filter(|held| **held == code).count(),
            1,
            "{code}"
        );
    }

    let core = GraphCore::new();
    let iso = isolation();
    save(&core, &iso, SCOPE_A, "alice", profile(0, vec![])).unwrap();
    // Every stored byte of the graph, in node-id order.
    let snapshot = |core: &GraphCore| {
        let mut ids = core.node_ids();
        ids.sort();
        ids.into_iter()
            .map(|id| {
                let blob = core.get_node_properties(&id);
                (id, blob)
            })
            .collect::<Vec<_>>()
    };
    let before = snapshot(&core);

    // PROFILE_NOT_FOUND: a save derived from a revision of a profile that was never stored.
    let mut ghost = profile(3, vec![]);
    ghost.profile_id = "p-ghost".into();
    let not_found = save(&core, &iso, SCOPE_A, "alice", ghost).unwrap_err();
    assert!(not_found.starts_with("PROFILE_NOT_FOUND: "), "{not_found}");
    // PROFILE_REVISION_CONFLICT: a save derived from a stale revision.
    let conflict = save(&core, &iso, SCOPE_A, "alice", profile(7, vec![])).unwrap_err();
    assert!(
        conflict.starts_with("PROFILE_REVISION_CONFLICT: "),
        "{conflict}"
    );
    // PROFILE_NOT_VISIBLE: a profile nobody shared with the caller, and another tenant's.
    let hidden = get(&core, &iso, SCOPE_A, "bob", "p-1").unwrap_err();
    assert!(hidden.starts_with("PROFILE_NOT_VISIBLE: "), "{hidden}");
    let foreign = get(&core, &iso, SCOPE_B, "alice", "p-1").unwrap_err();
    assert!(foreign.starts_with("PROFILE_NOT_VISIBLE: "), "{foreign}");
    // DOCUMENT_SET_QUERY_NOT_READ_ONLY and DOCUMENT_SET_NOT_FOUND.
    let not_read_only = admit_put(
        &core,
        &iso,
        SCOPE_A,
        "alice",
        &ProfileOp::PutDocumentSet {
            set: set(DocumentSetKind::SavedQuery(
                "INSERT INTO x VALUES (1)".into(),
            )),
        },
    )
    .map(|_| ())
    .unwrap_err();
    assert!(
        not_read_only.starts_with("DOCUMENT_SET_QUERY_NOT_READ_ONLY: "),
        "{not_read_only}"
    );
    let absent_set = read_op(
        &core,
        &iso,
        SCOPE_A,
        "alice",
        &ProfileOp::GetDocumentSet {
            set_id: "set-ghost".into(),
        },
    )
    .map(|_| ())
    .unwrap_err();
    assert!(
        absent_set.starts_with("DOCUMENT_SET_NOT_FOUND: "),
        "{absent_set}"
    );

    // None of the refusals wrote anything.
    assert_eq!(snapshot(&core), before);
}

mod derive_worker_proofs {
    use super::super::memory_surface_tests::{record, store};
    use super::super::participant_derive::{
        advance_checkpoint, derive_input, payload_node_id, payload_write, read_checkpoint,
    };
    use super::{SCOPE_A, SCOPE_B};
    use medulla_decision_types::claims::ObserverRef;
    use medulla_graph_core::graph::GraphCore;
    use medulla_protocol_types::protocol::Method;
    use medulla_server_mutation::mutation::apply_coalescable_write;
    use medulla_types::contract::ResourceId;
    use medulla_work_types::memory::{MemoryRememberRequest, MemoryTier};
    use medulla_work_types::participant::ParticipantId;
    use {
        medulla_decision_stats_records::decision::participant_derive::DeriveKey,
        medulla_decision_stats_records::decision::participant_derive::DerivePayload,
    };

    fn id(slug: &str) -> ParticipantId {
        ParticipantId::new(slug).unwrap()
    }

    /// Four turns of session `s1`: ada, bo, ada, ada.
    fn seed(core: &GraphCore, tenant: &str) {
        for (n, (turn, speaker)) in [("t0", "ada"), ("t1", "bo"), ("t2", "ada"), ("t3", "ada")]
            .iter()
            .enumerate()
        {
            let mut item = record(tenant, turn, MemoryTier::Episodic, &format!("said {turn}"));
            item.session_id = Some(ResourceId::new("s1").unwrap());
            item.speaker = Some(id(speaker));
            item.timestamp_ms = 100 + n as i64;
            store(
                core,
                tenant,
                &MemoryRememberRequest {
                    record: item,
                    embedding: vec![],
                    episode: None,
                },
            );
        }
    }

    fn payload(first: &str, last: &str, turns: u32) -> (DeriveKey, DerivePayload) {
        let key = DeriveKey {
            scope_id: String::new(),
            session_id: "s1".into(),
            subject: id("ada"),
            observer: ObserverRef::System,
        };
        let payload = DerivePayload {
            key: key.clone(),
            first_turn_id: first.into(),
            last_turn_id: last.into(),
            turns,
            task_id: format!("task-{first}"),
            last_turn_ms: 103,
            due_at_ms: 103,
        };
        (key, payload)
    }

    fn put_payload(core: &GraphCore, tenant: &str, payload: &DerivePayload) -> String {
        let node_id = payload_node_id(tenant, &payload.key);
        let write = payload_write(tenant, &node_id, payload).unwrap();
        apply_coalescable_write(core, &write).unwrap();
        node_id
    }

    fn apply(core: &GraphCore, write: &Method) {
        let Method::BatchUpdate { operations_msgpack } = write else {
            panic!("the checkpoint is one batch");
        };
        medulla_graph_algos::algorithms::batch_update(core, operations_msgpack).unwrap();
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.12
    #[test]
    fn derive_job_writes_claims_with_turn_evidence() {
        let core = GraphCore::new();
        seed(&core, SCOPE_A);
        seed(&core, SCOPE_B);
        let (key, first_range) = payload("t0", "t2", 2);
        let node = put_payload(&core, SCOPE_A, &first_range);

        // The worker reads only the subject's turns of the range, in order, with their ids.
        let input = derive_input(&core, SCOPE_A, &node).unwrap();
        assert_eq!(input.subject, id("ada"));
        assert_eq!(input.observer, ObserverRef::System);
        let ids: Vec<&str> = input
            .turns
            .iter()
            .map(|turn| turn.turn_id.as_str())
            .collect();
        assert_eq!(ids, ["t0", "t2"]);
        assert!(input.max_conclusions >= 1);

        // Completing the range writes the checkpoint and one event node in one batch.
        let (write, completed) = advance_checkpoint(&core, SCOPE_A, &node, 2, 500).unwrap();
        assert_eq!(completed.claims_written, 2);
        assert_eq!(completed.last_turn_id, "t2");
        apply(&core, write.as_ref().expect("a new range writes"));
        assert_eq!(
            read_checkpoint(&core, SCOPE_A, &key).unwrap().last_turn_id,
            "t2"
        );
        let events: Vec<String> = core
            .node_ids()
            .into_iter()
            .filter(|node| node.starts_with("memory-event:"))
            .collect();
        assert_eq!(events.len(), 1);
        // Completing the same range again writes nothing.
        assert!(advance_checkpoint(&core, SCOPE_A, &node, 2, 600)
            .unwrap()
            .0
            .is_none());
        // More conclusions than the knob allows is refused and changes nothing.
        let (_, next) = payload("t3", "t3", 1);
        let next_node = put_payload(&core, SCOPE_A, &next);
        let over = advance_checkpoint(&core, SCOPE_A, &next_node, u32::MAX, 700).unwrap_err();
        assert!(over.starts_with("DERIVE_CONCLUSION_LIMIT: "), "{over}");
        assert_eq!(
            read_checkpoint(&core, SCOPE_A, &key).unwrap().last_turn_id,
            "t2"
        );

        // Another tenant reads the payload as absent, on both operations.
        let foreign = derive_input(&core, SCOPE_B, &node).unwrap_err();
        assert!(
            foreign.starts_with("DERIVE_PAYLOAD_NOT_FOUND: "),
            "{foreign}"
        );
        assert!(advance_checkpoint(&core, SCOPE_B, &node, 1, 1).is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.12
    #[test]
    fn a_range_after_a_gap_is_held_and_writes_nothing() {
        let core = GraphCore::new();
        seed(&core, SCOPE_A);
        let (key, first) = payload("t0", "t0", 1);
        let first_node = put_payload(&core, SCOPE_A, &first);
        let (write, _) = advance_checkpoint(&core, SCOPE_A, &first_node, 1, 10).unwrap();
        apply(&core, write.as_ref().unwrap());

        // ada's next turn is t2; a range that starts at t3 skips it.
        let (_, skipping) = payload("t3", "t3", 1);
        let node = put_payload(&core, SCOPE_A, &skipping);
        let before: Vec<String> = core.node_ids();
        let held = derive_input(&core, SCOPE_A, &node).unwrap_err();
        assert!(held.starts_with("DERIVE_ORDER_HELD: "), "{held}");
        let held = advance_checkpoint(&core, SCOPE_A, &node, 1, 20).unwrap_err();
        assert!(held.starts_with("DERIVE_ORDER_HELD: "), "{held}");
        assert_eq!(core.node_ids().len(), before.len());
        assert_eq!(
            read_checkpoint(&core, SCOPE_A, &key).unwrap().last_turn_id,
            "t0"
        );
    }
}
