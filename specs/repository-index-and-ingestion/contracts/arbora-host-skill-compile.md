# Seam: the host admits and runs a compiled skill plan and grants indexing memory

Provider: medulla MEDULLA-REPO-INGEST-R002.1.3
Consumer: arbora ARBORA-HOST-R001.2.1.2

Medulla compiles a pinned skill document into a typed task plan and stores the receipt. The host admits and runs that plan. The host also grants the working memory that an incremental index job reserves.

## Wire shape

`Method::SourceIngest` with record kind `skill_document` returns a receipt that names the `SkillCompilation` digests. A plan read returns the `TaskPlan` of `crates/medulla-decision-types/src/decision/task_plan.rs`, or a refusal when the binding is stale. An index job carries `max_working_bytes`, which the host grant (`ARBORA-CAPACITY-R753`) bounds.

## Refusal codes

`compile_skill` refuses with `SKILL_STEP_ID_MISSING`, `SKILL_STEP_ID_DUPLICATE`, `SKILL_DEPENDENCY_UNRESOLVED`, `SKILL_CYCLE`, `SKILL_CAPABILITY_UNRESOLVED`, `SKILL_TYPE_UNSUPPORTED` and `SKILL_SPAN_MISSING`. A stale binding refuses a plan read. `SEMANTIC_MODE_UNSUPPORTED` refuses an unsupported typing mode.

## Owner rows

- medulla: MEDULLA-REPO-INGEST-R002.1.3 (skill plan wiring) and MEDULLA-REPO-INGEST-R006.4 (bounded incremental indexing).
- arbora: ARBORA-HOST-R001.2.1.2 (admit and run the plan) and ARBORA-CAPACITY-R753 (working-memory grant).
