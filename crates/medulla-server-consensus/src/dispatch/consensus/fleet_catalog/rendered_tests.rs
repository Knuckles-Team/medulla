use super::*;
use std::cell::RefCell;

const DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn skill_entry() -> AgentComponentEntry {
    serde_json::from_value(serde_json::json!({
        "schema_version": 3,
        "component_id": "mcp:skills/skill/triage",
        "kind": "skill",
        "version": "1",
        "content_digest": DIGEST,
        "facts": {"facts": "opaque"},
        "provenance": {"origin": "native"},
        "summary": "triage",
        "attributes": {},
        "tenant_id": "tenant-a",
        "actor_scope": "principal:sha256:importer",
        "purpose_id": "purpose",
        "policy_digest": DIGEST,
        "source_revision": "connector-pack",
        "source_revision_digest": DIGEST,
        "entry_revision": 2,
        "lifecycle": "published",
        "definition_digest": DIGEST,
        "created_at_ms": 1,
        "updated_at_ms": 2,
    }))
    .expect("fixture is a valid component entry")
}

struct Graph {
    revision: u64,
    body: RefCell<Option<Vec<u8>>>,
    terms: RefCell<Vec<OntologyTermRef>>,
    delta: RefCell<Vec<CodeDeltaRecord>>,
}

impl Graph {
    fn new() -> Self {
        Graph {
            revision: 7,
            body: RefCell::new(Some(b"# triage\nstep one".to_vec())),
            terms: RefCell::new(vec![
                OntologyTermRef {
                    iri: "https://knuckles-team.github.io/ontology/core#Task".into(),
                    defined: true,
                },
                OntologyTermRef {
                    iri: "https://knuckles-team.github.io/ontology/core#Tool".into(),
                    defined: true,
                },
            ]),
            delta: RefCell::new(vec![CodeDeltaRecord {
                path: "src/triage.rs".into(),
                change_digest: "aa".into(),
            }]),
        }
    }
}

impl SkillGraphContent for Graph {
    fn revision(&self) -> u64 {
        self.revision
    }
    fn skill_body(&self, _: &str) -> Option<Vec<u8>> {
        self.body.borrow().clone()
    }
    fn referenced_terms(&self, _: &str) -> Vec<OntologyTermRef> {
        self.terms.borrow().clone()
    }
    fn code_delta(&self, _: &str) -> Vec<CodeDeltaRecord> {
        self.delta.borrow().clone()
    }
}

// spec: MEDULLA-TYPED-PACKS-R100.2
#[test]
fn render_reflects_latest_ingest() {
    let entry = skill_entry();
    let mut graph = Graph::new();
    let before = render_skill_package(&entry, "skill://triage", &graph).unwrap();
    assert_eq!(before.graph_revision, 7);
    assert_eq!(before.digest.len(), 64);

    // A re-ingest changes the referenced code: new digest, new revision.
    graph.delta.borrow_mut().push(CodeDeltaRecord {
        path: "src/triage.rs".into(),
        change_digest: "bb".into(),
    });
    graph.revision = 8;
    let after = render_skill_package(&entry, "skill://triage", &graph).unwrap();
    assert_ne!(after.digest, before.digest);
    assert_eq!(after.graph_revision, 8);

    // A new body changes the digest as well.
    let body_changed = Graph::new();
    *body_changed.body.borrow_mut() = Some(b"# triage\nstep two".to_vec());
    assert_ne!(
        render_skill_package(&entry, "skill://triage", &body_changed)
            .unwrap()
            .digest,
        before.digest
    );

    // A missing body and an undefined term refuse.
    let empty = Graph::new();
    *empty.body.borrow_mut() = None;
    assert!(matches!(
        render_skill_package(&entry, "skill://triage", &empty),
        Err(RenderRefusal::BodyMissing { .. })
    ));
    let dangling = Graph::new();
    dangling.terms.borrow_mut()[0].defined = false;
    assert!(matches!(
        render_skill_package(&entry, "skill://triage", &dangling),
        Err(RenderRefusal::DanglingOntologyRef { .. })
    ));
}

// spec: MEDULLA-TYPED-PACKS-R100.2
#[test]
fn same_revision_same_digest() {
    let entry = skill_entry();
    let graph = Graph::new();
    let first = render_skill_package(&entry, "skill://triage", &graph).unwrap();
    // Reordering terms and delta never changes the digest.
    graph.terms.borrow_mut().reverse();
    graph.delta.borrow_mut().reverse();
    let second = render_skill_package(&entry, "skill://triage", &graph).unwrap();
    assert_eq!(first, second);
    assert!(first
        .digest
        .chars()
        .all(|c| matches!(c, '0'..='9' | 'a'..='f')));
}
