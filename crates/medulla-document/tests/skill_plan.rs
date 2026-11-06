//! Skill-document compilation (MEDULLA-REPO-INGEST-R002.1.2, R002.1.4 compiler side).

use medulla_document::skill_plan::{
    compile_skill, CapabilityLookup, SkillDocument, SkillPlanRefusal,
};
use medulla_types::contract::{Digest256, ServerErrorCode};

struct Registry;

impl CapabilityLookup for Registry {
    fn knows(&self, capability_id: &str) -> bool {
        capability_id.starts_with("cargo.")
    }
    fn capability_digest(&self) -> Digest256 {
        Digest256::sha256(b"capabilities")
    }
    fn type_digest(&self) -> Digest256 {
        Digest256::sha256(b"types")
    }
}

fn compile(text: &str) -> Result<Vec<String>, SkillPlanRefusal> {
    let doc = SkillDocument {
        directory_ref: "skills/deploy",
        text,
    };
    compile_skill(&doc, &Registry).map(|(proposal, _)| {
        proposal
            .tasks
            .iter()
            .map(|task| task.task_id.clone())
            .collect()
    })
}

const DOCUMENT: &str = "# Deploy\n\nProse: not a step.\n\n## Steps\n\
1. fmt: format the code\n   capability: cargo.fmt\n\
2. lint: lint the code\n\
3. ship: publish both\n   after: fmt, lint\n\n## Notes\nnote: ignored\n";

// spec: MEDULLA-REPO-INGEST-R002.1.2
#[test]
fn steps_compile_in_source_order_with_dependencies() {
    let doc = SkillDocument {
        directory_ref: "skills/deploy",
        text: DOCUMENT,
    };
    let (proposal, compilation) = compile_skill(&doc, &Registry).expect("compiles");
    let ids: Vec<_> = proposal.tasks.iter().map(|t| t.task_id.as_str()).collect();
    assert_eq!(ids, ["fmt", "lint", "ship"]);
    assert_eq!(
        proposal.tasks.as_slice()[2].depends_on.as_slice(),
        ["fmt", "lint"]
    );
    assert_eq!(proposal.output_task_id, "ship");
    assert_eq!(compilation.grammar_revision, "skill-steps-1");
    assert_eq!(compilation.steps.len(), 3);
    let first = &compilation.steps[0];
    assert_eq!(first.capability_id.as_deref(), Some("cargo.fmt"));
    let span = &DOCUMENT[first.span_start as usize..first.span_end as usize];
    assert!(span.starts_with("1. fmt:") && span.ends_with("cargo.fmt"));
    assert_eq!(compile(DOCUMENT).expect("again"), ids);
}

#[test]
fn each_refusal_maps_to_its_registered_code() {
    let cases = [
        ("## Steps\n- : no id\n", ServerErrorCode::SkillStepIdMissing),
        (
            "## Steps\n- a: x\n- a: y\n",
            ServerErrorCode::SkillStepIdDuplicate,
        ),
        (
            "## Steps\n- a: x\n  after: ghost\n",
            ServerErrorCode::SkillDependencyUnresolved,
        ),
        (
            "## Steps\n- a: x\n  after: b\n- b: y\n  after: a\n",
            ServerErrorCode::SkillCycle,
        ),
        (
            "## Steps\n- a: x\n  capability: nope.run\n",
            ServerErrorCode::SkillCapabilityUnresolved,
        ),
        (
            "## Steps\n- a: x\n  retries: 3\n",
            ServerErrorCode::SkillTypeUnsupported,
        ),
        ("# Only prose\n", ServerErrorCode::SkillSpanMissing),
    ];
    for (text, expected) in cases {
        assert_eq!(compile(text).expect_err(text).code(), expected, "{text}");
    }
}
