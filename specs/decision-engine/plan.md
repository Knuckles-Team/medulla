# MEDULLA-DECISION-ENGINE — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

The decision engine answers typed questions through one pipeline. Every served operation enters at the dispatch boundary of `crates/medulla-server-router`. The handler crate `crates/medulla-h-decide` runs the stages. `crates/medulla-compute` and `crates/medulla-decision-policy` hold the exact solver. `crates/medulla-decision-stats` holds the fitted heads. The sealed record lives in `crates/medulla-decision-types`. Each requirement plugs into one stage, one question family or one cross-cutting concern. A requirement that plugs into none is not deliverable.

```text
arbora (admission, auth, principal) --> medulla dispatch
  Decide / AgentAssemble / DecideText / DecisionCommit / DecisionFit / DecisionEval / DecisionLog
    0 Admit > 1 Visibility > 2 Candidates > 3 Policy + hard constraints
      > 4 Entailment > 5 Exact solve > 6 Statistical rank > 7 Conclude or abstain
      > 8 Sealed record == DecisionCommit ==> 9 Commit > 10 Learn > 11 Replay
```

| Stage | Owning code | Contract |
|---|---|---|
| 0 Admit | `crates/medulla-h-decide/src/decide/served_adapter.rs` | Typed request parse. Caps: 64 KiB, 64 candidates, 32 capabilities. Principal and tenant come from the verified context. |
| 1 Visibility | `crates/medulla-h-decide/src/decide/candidates.rs` | Tenant and row visibility apply before any feature or count. |
| 2 Candidates | `crates/medulla-h-decide/src/decide/candidates.rs` | Only published, tenant-visible component revisions enter. Pack facts enter as claims. |
| 3 Policy and constraints | `crates/medulla-h-decide/src/decide/guardrail.rs`, `crates/medulla-decision-outcome-types/src/decision/limit_guard.rs` | Pins, denials, guardrails and limits remove options. Each removal is an eliminated-constraint premise. |
| 4 Entailment | `crates/medulla-decision-types/src/decision/record/` | Derivation records back entailed answers. |
| 5 Exact solve | `crates/medulla-decision-policy/src/solve`, `crates/medulla-compute/src/assemble` | Bounded 0-1 model, certificate and node budget. |
| 6 Statistical rank | `crates/medulla-decision-stats/src/decision`, `crates/medulla-h-decide/src/decide/stat_decide.rs` | Fitted heads rank legal options or abstain. They never widen the legal set. |
| 7 Conclude | `crates/medulla-compute/src/assemble/conclude.rs`, `crates/medulla-h-decide/src/decide/stat_decide.rs` | One `ResolutionKind`, one `EvidenceClass`, one typed abstention. |
| 8 Record | `crates/medulla-decision-types/src/decision/record.rs`, `crates/medulla-h-decide/src/decide/stat_log.rs` | The sealed record pins inputs, premises, digests and result. |
| 9 Commit | `crates/medulla-h-decide/src/decide/commit.rs` | Re-derive, verify, catalog compare-and-set, idempotent retry. |
| 10 Learn | `crates/medulla-h-decide/src/decide/stat_learning.rs`, `crates/medulla-h-decide/src/decide/stat_jobs.rs` | Outcomes, off-policy estimates, fit, evaluate, guarded publish. |
| 11 Replay | `crates/medulla-h-decide/src/decide/stat_replay.rs`, `crates/medulla-h-decide/src/decide/stat_walk_forward.rs` | Byte-identical replay and walk-forward evaluation. |

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain of `rust-toolchain.toml`; Python 3.14 (floor 3.12) for served tests.
- **Primary Dependencies**: The workspace crates named in the Project Structure section and the standard library. No generative model and no model server.
- **Storage**: The Agent Library component store holds `DecisionRecord` components. Graph-sourced summaries use RLS-routed graph writes. Decision-owned tables are in the durable kernel.
- **Testing**: Rust unit tests per crate, the served `Harness` under `crates/medulla-h-decide/src/decide/stat_tests/`, and served Python tests under `tests/decide/`.
- **Target Platform**: Linux x86_64 and aarch64 hosts without AVX2 assumptions; fixed-point arithmetic keeps replay bit-identical.
- **Project Type**: Library crates and a server handler crate.
- **Performance Goals**: `Decide` p50 20 ms and p99 150 ms at 64 candidates and 32 capabilities. `AgentAssemble` finishes within its declared node budget (`MEDULLA-DECISION-ENGINE-R178`).
- **Constraints**: Evaluate-only operations write nothing. Visibility filtering precedes features. Each refusal has one stable code. No arbitrary result cap. No version suffix in a name.
- **Scale/Scope**: The leaf rows of `requirements.md`, spread over 9 user stories and 16 `FR-` labels.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. Every row lands in a Rust crate and the engine store.
- **R-II. Every wait has a deadline**: pass. The solver has a node budget and each job has a deadline.
- **R-III. Every row sets budgets and names a benchmark**: pass. `MEDULLA-DECISION-ENGINE-R178` holds the budgets and the nightly suite.
- **R-IV. Crates are small, layered and cut over hard**: pass. Rows name the post-split crates, and no handler crate calls a sibling handler crate.
- **Organization standards**: pass. Rows follow the spec standard, ecosystem boundaries and the wording standard.

## Project Structure

```text
crates/medulla-decision-types/src/decision/        existing: record, request, policy, replay, task_plan types
crates/medulla-decision-outcome-types/src/decision/ existing: outcome and limit_guard types
crates/medulla-decision-policy/src/solve/          existing: 0-1 model, search, certificate, verifier
crates/medulla-decision-stats/src/decision/        existing: fitted heads, scorer, calibration
crates/medulla-compute/src/assemble/               existing: AgentAssemble model, conclude, seal
crates/medulla-h-decide/src/decide/                existing: Decide, commit, jobs, log handlers
crates/medulla-h-decide/src/decide/stat_tests/     existing: served Rust harness tests
crates/medulla-h-admin/src/admin/connector_pack/   existing: pack projection (project_head)
crates/medulla-server-router/src/dispatch/router/decision_plane.rs  existing: dispatch arms
tests/decide/                                      existing: served Python tests
```

New files that rows add are marked `new` in their own `Shape:` sentence.

## Phase 0: Research

No unknowns; no research.md.

## Phase 1: Design

The nine rules below bind every row.

1. **Question-family registry (`MEDULLA-DECISION-ENGINE-R001.101`).** One `DecisionFamily` trait holds the per-stage hooks. An exhaustive `match` maps each `QuestionKind` to its family, so the compiler rejects a kind without a family.
2. **One refusal taxonomy (`MEDULLA-DECISION-ENGINE-R010.100`).** `AbstainReason` covers outcomes and `DecisionError` covers request refusals. Each variant has one stable code, and a test proves every code is unique.
3. **Names and schema evolution.** No component name carries a version suffix. A record carries a `schema_version`. A decoder accepts every published version and refuses an unknown one.
4. **`Decide` is evaluate-only.** `Decide`, `AgentAssemble` and `DecideText` write nothing. Durable writes go through `DecisionCommit`, `DecisionLog` or a decision job, each the single write authority for its state kind (`MEDULLA-DECISION-ENGINE-R017`).
5. **Integration-first proof.** A behaviour row is VERIFIED only by a test that enters through the production dispatch with a tenant and principal context.
6. **Observability (`MEDULLA-DECISION-ENGINE-R072.100`).** Each stage emits one span `medulla.decide.<stage>` with family, tenant, outcome and duration. The sealed record is the audit artifact.
7. **Fitted-head governance.** A head moves through draft, evaluated, published, champion and retired. A challenger runs in shadow before promotion. Rollback is one compare-and-set on the champion pointer (`MEDULLA-DECISION-ENGINE-R805`).
8. **Evolution-tuned knobs (`MEDULLA-DECISION-ENGINE-R179`).** Each tunable value is a named knob with a default, bounds and a unit. Knobs of security, policy, write-back and irreversible questions are `Fixed`.
9. **Ownership.** A medulla row keeps the fact, decision and record side. Served, contract and connector parts live in the owner's spec and link by ID.

Data entities are in [data-model.md](data-model.md). The shortest served proof is in [quickstart.md](quickstart.md). Each seam to another repository has a document in `contracts/`.

### Process control steps and decisions (R134.111.14 to R134.111.18, R180)

A task plan gains four control step kinds beside prompt and tool steps: branch, join, decision and approval wait. `plan_explicit_tasks` stays the one canonicalizer. It checks arm targets, defaults, joins and approval timeouts. The plan digest covers every control field. Branch choice at run time is a `Decide` question, not harness logic. `BranchSelect` evaluates the arm conditions over bound step outputs. `DecisionTableEval` evaluates a stored DMN table (`MEDULLA-CQ-ORCH-R702.5`) under its hit policy. Expressions use the simple unary test subset; anything else refuses `DMN_EXPRESSION_UNSUPPORTED`. The meristem run loop applies the verdict; the consumer rows are in `../orchestration-competency/contracts/meristem-process-control-steps.md`. Call path: `Method::Decide` → `handle_decide` → `crates/medulla-h-decide/src/decide/family/process_decision.rs` → `evaluate_table` and `select_branch` in `medulla-decision-stats`.

### Decomposition and skill planning (R134.3, R134.4)

Two proposers feed `plan_explicit_tasks` and add no second canonicalizer or cycle check. `decompose_task` (decision-types) routes harness subtasks to specialists by score. `plan_over_skills` (decision-stats, beside the candidate-local BM25) chooses, merges and splits over the tenant's published skills. Both are reached through the typed `decomposition` and `skill_query` parameters of a `task_plan` question, and read their thresholds from decision-policy knobs (`knob_families/task_plan.rs`). Design calls are in research.md.

## Phase 2: Task approach

`tasks.md` holds one task per leaf row in table order, producer first. A model row precedes the rows that wire it. A row lands in one slice with its caller, so no type lands without a production caller. Tasks of different question families run in parallel. Tasks that edit the shared `decision_plane.rs` dispatch file run in series.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- Every row names a budget and a benchmark where it sits on the hot path, per article R-III.
- A public item in a decision crate has a non-test caller under a handler or server crate. `MEDULLA-CONTRACT-R055` gates this.
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.
- Dispatch stays in `decision_plane.rs`; a handler crate never calls a sibling handler crate.

## Production caller chain

- `Method::Decide` arm (`crates/medulla-server-router/src/dispatch/router/decision_plane.rs:112`) to `handle_decide` (`crates/medulla-h-decide/src/decide/statistical.rs:11`) to `stat_decide::handle_decide` (`crates/medulla-h-decide/src/decide/stat_decide.rs:60`) to the family hooks, the executor and the sealed record.
- `Method::AgentAssemble` arm (`decision_plane.rs:64`) to `handle_agent_assemble` (`crates/medulla-h-decide/src/decide/assemble.rs:25`) to `assemble_for` (`assemble.rs:46`) to `crates/medulla-compute/src/assemble/` and `crates/medulla-decision-policy/src/solve`.
- `Method::DecisionCommit` arm (`decision_plane.rs:100`) to `handle_decision_commit` (`crates/medulla-h-decide/src/decide/commit.rs:22`) to the Agent Library transaction.
- `Method::DecisionFit`, `Method::DecisionEval` and `Method::DecisionLog` arms (`decision_plane.rs:119`, `:126`, `:133`) to `crates/medulla-h-decide/src/decide/jobs.rs` and `log.rs`.
- `Method::ConnectorPack` arm (`decision_plane.rs:161`) to `project_head` (`crates/medulla-h-admin/src/admin/connector_pack/reproject.rs:74`), which leaves an outbox row for the capability feed of `MEDULLA-DECISION-ENGINE-R172.7.2`.
- Obsolete paths to delete: the per-kind `if` chains in `stat_decide.rs` and `candidates.rs` once the family registry lands.

### Capability ranking catalog (R829.1 to R829.3)

Pre-untangled design, one line per answer.

- Owner. Medulla `crates/medulla-h-decide`. Consumers send candidates and a fallback; they hold no scorer.
- Cohesion. The feature computation sits beside the rank family; the five question ids share it.
- Dependency direction. The family reads the catalog table and the feature module; the SDK and arbora call the served method only.
- Interface depth. One served method, `Decide`, with five new question ids.
- Special cases. None. A tool, a skill and an operation differ by the question id and nothing else.
- State. None.
- Environment. The two weights are knobs of the feature.

Order: R829.1, R829.2, R829.3.

## Cross-repo seams

- Served web page selection: `MEDULLA-DECISION-ENGINE-R802.3` provides, `ARBORA-OPS-R067` consumes. Contract: [arbora-served-operations.md](contracts/arbora-served-operations.md).
- A2A routing and fleet calls: `MEDULLA-DECISION-ENGINE-R044.2` provides, `ARBORA-A2A-R023` consumes. Contract: [arbora-a2a-routing.md](contracts/arbora-a2a-routing.md).
- Harness consumption of decisions: `MEDULLA-DECISION-ENGINE-R134.111` provides, `MERISTEM-HARNESS-CYCLE-R001` consumes. Contract: [harness-decision-consumption.md](contracts/harness-decision-consumption.md).
- Connector path outcomes: `MEDULLA-DECISION-ENGINE-R159.3` provides, `SCION-CONNECTOR-CONTROL-R904` consumes. Contract: [connector-control-feedback.md](contracts/connector-control-feedback.md).
- Governed write-back proposals: `MEDULLA-DECISION-ENGINE-R615` provides, `SCION-GOVERNED-WRITEBACK-R006` consumes. Contract: [governed-writeback-proposals.md](contracts/governed-writeback-proposals.md).

## Loops and enrichment behaviour (R653.2-R653.8, R614.2)

- Loop kernels are pure functions in `crates/medulla-statechart/src/loops.rs`; the handler
  `crates/medulla-h-decide/src/decide/loops.rs` is the single writer and holds each loop as a loop-chart
  instance plus a `loop_record` instance. Decisions: `research.md`, "Improvement loops".
- Enrichment behaviours are pure functions in `crates/medulla-enrich/src/semantic.rs`.

## Topology admission (R046.750, R046.751)

`assemble::topology::admit::admit_topology(plan, depth, caps)` runs in `conclude::solved` on the sealed plan, adds
`topology_cap:<name>=<value>` and `plan_digest:<digest>` premises, and abstains with the refusal code on a breach.
Caps = topology policy caps, tightened by the pinned cap knobs, tightened by the request. Bench group
`decide/topology_admit` with a `group_budgets` entry (p50 0.2 ms, p99 0.8 ms).

## Run credit (R100.102)

`stat_learning::dispatch` routes `LearningWrite::RunOutcome` to `stat_run_credit::record_run`, which reads the
referenced decisions' stored retrieval outcomes, runs the pure kernel `medulla_decision_stats::decision::run_credit::credit_steps`
and commits the receipt (`run_credit/run/<run_id>`) with every new baseline revision
(`run_credit/baseline/<sha256 of capability>/<inverted revision>`) in one batch. Knobs `run_credit.max_steps` and
`run_credit.baseline_weight` come from the default decision policy.
- R036.106: `ProfileGrantMachine` and the `rbac_profile_use_grant` owner table; the predecessor layout is upgraded only through the offline upgrade registry; the commit admission lists the caller's active grants as `profile_grant:<id>` premises (research.md).
- R036.105.1 and R036.105.2: the coordinator-decision certificate lives in its own `xshard_decision_certificate` owner table with a graph-shard lineage predecessor and an offline-upgrade registry row, never under a reserved key of `XSHARD_PREPARE` or `XSHARD_DECISION`; `DecisionCommit` writes the pending marker and the terminal decision with the sealed certificate through `commit_cross_shard_recoverable`; the deadline is the knob `xshard.decision_deadline_ms` (research.md).
- R036.105.2 handler order: `Coordinator::begin` (pending marker plus certificate request) runs before the owners reserve; the stale-catalog vote precedes the decision; the record is written only after a durable COMMIT; both owners resolve from `Coordinator::recover`. No backend refuses `DECISION_COORDINATOR_UNAVAILABLE` (research.md).
- R113.2.5 and R113.3.1.1: the shared-owner commit path (record and evidence in one Agent Library transaction, lease as the graph participant, decided by the R036.105.2 coordinator decision) and the durable `decision_retry_allowance` table are the producers of R113.2.2 to R113.2.4 and R113.3.2.

## Thought search (R750.750 to R750.754)

`Decide` with `QuestionKind::ThoughtSearch` runs `family/thought_search.rs`, which calls `stat_thought_search::conclude`: parse the typed `thought_search` and `task_class` params, read the strategy from `DecisionPolicy.reasoning`, validate the frontier, read the knobs through `ThoughtSearchKnobs::from_policy`, apply request lowerings, then run `choose_expansion`. The answer is sealed as `FamilyAnswer::ThoughtSearch` with `thought:*` and `knob:*` premises. Nothing is written.

- R140.100 to R140.103: the four pure trajectory decisions (`trajectory_budget`, `trajectory_projection`, `branch_breadth`, `pairwise`) and their `trajectory.*`, `breadth.*` and `pairwise.*` knob table sit in `medulla-decision-types`; their refusal tables join `codes::FAMILY_TABLES` under `TaskPlan`. The projection shuffle is ChaCha8 (`rand_chacha`). The served wiring and the `trajectory_decisions_journey` root slice test land with R140.104 and R140.105.

### R614.3 SemanticEnrich route

`Method::SemanticEnrich` (method_11) is served by `graph_pipeline/semantic_enrichment.rs`: schedule gate, then the pure behaviours of `medulla-enrich`, then the digest-keyed write-set of `medulla-h-ingest::semantic_enrichment` committed through the ChangeEnvelope authority. Refusals map the typed `EnrichError` codes (registered in `ServerErrorCode` and the method's error routing) and write nothing.

Scoped adapter wiring (R174.7.3, R174.7.4): `stat_adapter.rs` fits graph-scope retrieval-outcome adapters on the original split and every other scope through `fit_scoped`; `ensure_room` enforces `adapter.max_per_tenant` before an activation; `monitor` is the only automatic rollback path.

Index-side adapters (R174.7.3.2): `FitAdapter` accepts `side = index`; `qualified_index` checks the receipt for a cycle's `index_adapter`, `stat_learning::prepare` decodes the kernel and gives the cycle a `NoEndpoint` embedder and a CPU lease, and `stat_generation_cycle::materialize_batch` applies the kernel to stored vectors batch by batch under the same checkpoint and state machine as a re-embed.
Usage guidance ordering (R177.2): `order_steps` is a Kahn walk over the `:precedes` edges; each round it takes the best of the ready steps, scanned in tool-id order so the result never depends on input order. A rate decides only between two ready steps whose tools both have `usage.min_trials` outcomes; the cycle error names the steps on the cycle so R177.4 can refuse with them.

Research frontier (R705.2): `expand_frontier` in `medulla-decision-stats` is a binary-heap walk over caller-supplied source nodes (Q32 expected value, id tie-break); the family `research_voi` (R705.3) supplies the nodes from the graph and reads `research.*` knobs through the registry.

Feature matching (R707.2): `crates/medulla-decision-stats/src/decision/feature_match.rs` holds `canonical_id`, `entropy_mbit`, `score_pair`, `dedup` and `advance_ledger`, all pure; the family `feature_match` (R707.3) supplies embedding cosines and the `match.*` knobs, and the ledger handler calls `advance_ledger` with the revision the caller read.
## Capability designation index (R172.2 to R172.6)

The resident index is `crates/medulla-decision-memory/src/memory/capability_index/`: `postings.rs` (sorted posting lists), `arena.rs` (SQ8 codes, f32 vectors, `HnswIndex` for large pools), `shard.rs` (snapshots, upsert, tombstone, compaction, embedding pin, re-index swap) and `capability_index.rs` (tenant shards plus the commons shard, `read`). The blend is `crates/medulla-decision-rank/src/rank/capability_rank.rs`. Eligibility, reward update and the `designation.*` knobs are in `crates/medulla-decision-types/src/decision/capability_designation.rs`. The `Decide` family wiring, reward persistence and the decision-log op build on these.
