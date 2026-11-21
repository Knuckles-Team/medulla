//! Agent component projection budget (MEDULLA-ORCH-VOCAB-R003.6).
//!
//! `project_component` over an A2A card that declares the maximum of 64 capabilities: the
//! work is linear in the declared capabilities. Never gates a pull request.
//!
//! Run: `cargo bench -p medulla-h-admin --features rdf --bench agent_library_projection`

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_agent_types::agent_component::{
    AgentComponentDraft, AgentComponentEntry, AgentComponentKind, DraftPublication, DraftSubject,
};
use medulla_h_admin::agent_library_projection::map::project_component;
use std::hint::black_box;

fn card() -> AgentComponentEntry {
    let digest = format!("sha256:{}", "1".repeat(64));
    let mut draft = AgentComponentDraft::bare(
        DraftSubject {
            component_id: "card-bench",
            kind: AgentComponentKind::A2aAgentCard,
            version: "1.0.0",
            content_digest: &digest,
            summary: "card",
        },
        DraftPublication {
            tenant_id: "tenant-bench",
            actor_scope: "agent-builder",
            purpose_id: "agent-construction",
            policy_digest: &digest,
            source_revision: "rev-1",
            source_revision_digest: &digest,
        },
    );
    draft.declared_capabilities = (0..64).map(|i| format!("urn:vendor:capability-{i}")).collect();
    AgentComponentEntry::publish(draft, 1, 1).expect("the card publishes")
}

fn bench_project_component(c: &mut Criterion) {
    let entry = card();
    c.bench_function("project_component_64_capabilities", |b| {
        b.iter(|| black_box(project_component(black_box(&entry))))
    });
}

criterion_group!(benches, bench_project_component);
criterion_main!(benches);
