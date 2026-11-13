# Test specification

Tests are keyed to the published commit and record command, result, fixture version and workflow URL in an acceptance record. The baseline suite starts a disposable local Medulla instance and creates its own tenant, pack and graph fixtures. External services are optional deployment qualification, not a PR prerequisite.

## Integration proof standard

This standard applies to every row bound after 2026-10-30. It implements AD-5 in `plan.md`. A test that exercises a type or function in isolation proves the algorithm, not the row.

### Proof levels

| Level | Harness | Verifies a row alone |
|---|---|---|
| L1 Unit | `#[cfg(test)]` module or crate test | No |
| L2 Property and golden vector | `proptest`, committed byte vectors, exhaustive small models | No |
| L3 Served (Rust) | `Harness` in `crates/medulla-h-decide/src/decide/stat_tests`, which runs the real handler and dispatch | Yes |
| L4 Served (client) | `tests/decide/*_served.py` over a loopback engine with authentication | Yes |
| L5 Cross-repository consumer | The owner repository's test against medulla's published types | For the consumer row only |
| L6 Nightly qualification | Benchmarks, fuzzing, cross-target replay | For AD-9 and R007 rows only |

### Required assertions for an L3 or L4 proof

1. The request enters through the production dispatch with a tenant and principal context.
2. The test asserts the outcome, the `ResolutionKind` and the `EvidenceClass`.
3. The test asserts the premises that the row adds to the sealed record.
4. Each refusal case asserts its stable code from the refusal taxonomy (AD-2).
5. A row that adds a stage behaviour includes one negative case from another tenant (R013).
6. A row that adds a durable write proves the write goes through its single write authority (R017), and that `Decide` wrote nothing.

### Per-root slice

A root's slice PR binds every child row. The `.1` and `.2` rows bind to their L1 or L2 tests and to the root's L3 test. The `.3`, `.4` and `.5` rows bind to L3 tests. The `.6` row binds to an L3 test, plus an L4 test when the root adds a client method.

### Reachability

The reachability gate (AD-6) fails when a public decision item has no non-test caller under `src/`. A row whose code fails the gate is not VERIFIED, whatever tests it binds.

## A. Contract and deterministic records

| Case | Setup and assertion |
|---|---|
| A1 method/profile parity | Generate Rust/Python/contract descriptors. `AgentAssemble`, `DecisionCommit`, `Decide`, `DecisionFit`, `DecisionEval`, pack import and content reads have exactly one reachable handler, stated scope, stability and feature profile. Generated files are fresh. |
| A2 canonical replay | Commit v1, v2 and v3 records with golden byte vectors on all supported release targets. Decode, re-encode, verify and compare digest/answer bytes. Unknown future version fails closed. Replaying stored inputs is stable even when live catalog changes. |
| A3 forged record | Mutate a candidate fact, premise class, solver bound, objective, policy digest or outcome. Retain the old digest. `DecisionCommit` refuses; no record/component/outbox write occurs. |
| A4 snapshot race | Publish or withdraw a candidate after evaluate and before commit. Expected catalog digest mismatch returns stale-catalog; unrelated publish outside the candidate universe does not. Retry same committed record is idempotent. |
| A5 limits | 64 KiB/64 candidate/32 capability limits accept boundary values and return typed `RecordTooLarge` or bounded refusal above them. Check no partial write. |

## B. Solver, entailment and explanation

Generate small 0–1 models with covering, implication, exactly-one, knapsack, compatibility and cardinality constraints; compare optimum and infeasibility to exhaustive enumeration. Repeat with shuffled input order and assert byte-identical answer/certificate. Corrupt each certificate field and confirm the independent verifier rejects it. Exhaust work budget with and without a policy-approved gap; confirm incumbent/gap reporting or typed abstention. Test no artificial result-count cutoff. Plant unknown cost and missing capability facts; neither may be treated as zero or proven true. Construct `is_a` paths whose weakest premise is claim or observation; derivation class remains proof while conclusion class weakens. A why-not re-solve must name a visible violation or worse objective and must not leak invisible alternatives. Fail a post-solve template validator and verify no-good-cut re-solve or budget abstention.

## C. Visibility, policy and statistics

Use two principals in one tenant plus a separate tenant. Seed an invisible candidate that would dominate a visible one. Verify it changes neither shortlist, candidate-local BM25/vector score, count, digest, explanation, aggregate nor abstention reason. Test policy tightening accepted and each relaxation field rejected. A fitted head may reorder only legal candidates and may abstain. It cannot restore a denied candidate or authorize an irreversible action. Use synthetic full-label and bandit datasets. Assert exact propensities, zero-support refusal, clipped/SWITCH-IPS/doubly-robust calculations, ESS threshold, minimum support, censoring and drift behavior. A self-reported outcome, low-fidelity trace or unapproved commit principal cannot enter training. `DecisionFit` yields a draft; promotion without a matching `DecisionEval` receipt, or with changed head/policy/schema/dataset, is refused. Check HMAC exploration unpredictability to callers, exact logged propensity, budget enforcement and forbidden question classes. Walk-forward folds must purge and embargo as configured; a shared cap may not be exceeded across concurrent options.

## D. Served decision journey

Against a locally started server, publish a typed component with claim-class capabilities. Call Python `AgentAssemble`, inspect record/certificate/why-not, then call `DecisionCommit`. Read it back and pin it as graph synthesis evidence. Repeat through the generated Python method wrapper and a wire client. Call `DecideText` with quoted candidate blocks, typed parameters and malformed spans. Run a fit/evaluate/publish/decide cycle with an independently evaluated outcome; assert receipt and record lineage. A second tenant sees no record. For topology, verify capacity-premise inputs, bounded shape variables, fail-closed template shape, proposed graph and no capacity lease before separate acquisition. For entity resolution and finance, verify proposals/analysis only and no write or trade side effect.

## D2. Task-plan facts

Publish two templates and one A2A agent card with overlapping capabilities. Make one template invisible to the caller. Call the coverage query for two task IRIs. Expect the closure, both visible covering components and claim class on the agent card. Expect no trace of the invisible template in rows, counts or errors. Call the guardrail query for a subclassed task and an unrelated task. Expect the entailed rules with paths for the first task and none for the second. Restart the server and repeat both calls. Expect byte-identical premises. (MEDULLA-DECISION-ENGINE-R126, MEDULLA-DECISION-ENGINE-R127)

Drive `GuardrailEntailment` through the dispatcher with a published parent-class rule and exact-class rule. Verify the complete subclass path, source generation and ontology digest, and byte-identical response premises after reopening the owner store. An unrelated task returns no rules. A foreign-tenant request refuses before the read; foreign rules, non-rule components, retired heads and withdrawn heads never enter the result. Empty task input refuses rather than answering an empty rule set. The method remains a read-only snapshot under `agent:assemble-read`. (MEDULLA-DECISION-ENGINE-R127.2.2)

## E. Pack facts at the decision boundary

Publish one pack-provided component with native and foreign capability annotations, cost, latency, modality, schema digest, model facts and `contract_pin`. Assemble from that component and assert every such premise remains a publisher claim in the record and explanation. The native capability may participate in an `is_a` derivation whose conclusion class is still claim. The foreign IRI remains an exact unentailed claim. Omit the member in a later valid pack and assert `Withdrawn` makes it ineligible. Re-import byte-identical entry content and assert its candidate digest and decision result do not churn. A stale projection marker must prevent a projected ontology or shape fact from entering a decision. Plant `DecisionRecord`, `DecisionHead` and `DecisionPolicy` pack entries and assert refusal before any decision-owned state changes. These probes use the pack API as a fixture. Pack parser, body and crash recovery have their own acceptance suite.

Check metric labels omit raw facts and errors omit invisible component identifiers. Every negative probe asserts no durable state change. A served test uses disposable credentials and a local policy fixture.

## H. Quality and acceptance matrix

### MEDULLA-DECISION-ENGINE-R116.2: generated-client runtime admission

Use the published Python contract against a disposable served fixture. Exercise an engine read and a decision call without importing `medulla.engine` or constructing a bespoke facade. Prove that the existing Rust `admit_call_path` runs before dispatch. Check the full generated method inventory, operation-specific adapters and `send_by_id` so an extra sender cannot bypass that boundary.

Pass an empty contract version and a bespoke call path through the real runtime boundary. Both must fail before transport dispatch or any durable side effect. Attempt an arbitrary `_send` adapter and a caller-asserted `Contract` label; neither may substitute for the owned admission-to-dispatch binding. Include a mutation fixture that removes admission from one generated sender and proves the coverage check fails. Re-run generated artifact freshness after changing the generator. Direct gate unit tests, source-text checks and mocked successful dispatch do not replace the served acceptance fixture.

### MEDULLA-DECISION-ENGINE-R118 and MEDULLA-DECISION-ENGINE-R119 focused acceptance

| ID | Positive fixture | Refusal and boundary fixture |
|---|---|---|
| MEDULLA-DECISION-ENGINE-R118 | Two source pools with independently labeled successes/failures, one sparse pool and two tenant scopes yield reproducible posterior, Brier/log/decomposition/calibration scores; the governed value agrees in decision policy, retrieval rank and UQL. | Self-reported labels, invisible outcomes and missing pool history cannot improve another tenant's score; a caller-supplied weight cannot override the materialization; sparse history abstains. |
| MEDULLA-DECISION-ENGINE-R119 | A fixed bitemporal series produces exact purged/embargoed walk-forward folds, proportional shared-cap allocations, real trial-count DSR, split PBO, incumbent DM, option contributions and byte-identical sealed replay after checkpoint resume. | Future-known rows, overlap, changed source/head/policy digest, over-cap allocation, policy-dependent environment, bad supersedes digest and malformed `REPLAY WALK FORWARD` refuse without a sealed run or head promotion. |

For each changed code path run focused Rust/Python/client tests, `cargo fmt --check`, and workspace Clippy with applicable features. Check generated contract freshness, CCCC, KISS, Dupehound and jscpd differential with zero new pairs. Hosted CI runs build/package tests for affected features and the locally provisioned served journey. Optional cloud/cluster/identity-provider tests run in a separate deployment qualification. A missing live environment cannot block normal external PRs. A complete acceptance record ties every relevant A–E case and gate result to the same published commit. `SOURCE_LANDED` stays distinct from `ACCEPTED`.

### MEDULLA-DECISION-ENGINE-R113.1: capacity commit type

On 2026-10-11, all six tests in `capacity_commit.rs` passed with Rust 1.96.0.
The test harness compiled the production module directly.
It linked the real cached `medulla-types` library for `CapacityDecision`.
No dependency stub or replacement constructor was used.

The tests cover all 13 capacity decisions and both commit parts.
Accepted and replayed decisions must retain the full lease list and publish evidence.
Each other decision must retain its reason and grant exactly one retry.
A denied decision with either commit part must fail.
An accepted or replayed decision without either commit part must fail.

To repeat this focused check, set `DEPS` to the cached dependency directory.
Set `MEDULLA_TYPES` to its compiled `libeg_types-*.rlib` file.
Use the Rust version that built that library:

```bash
root="$PWD"
work="$(mktemp -d)"
printf 'pub use medulla_types::native_control;
#[path = "%s/crates/medulla-agent-types/src/agent_topology/capacity_commit.rs"]
mod capacity_commit;
' "$root" > "$work/harness.rs"
rustc --edition=2021 --test "$work/harness.rs" \
  --extern "medulla_types=$MEDULLA_TYPES" -L "dependency=$DEPS" -o "$work/tests"
"$work/tests" --nocapture
```

The normal workspace command is `cargo test -p medulla-types capacity_commit`.
The direct-module run verifies this leaf's constructor contract only.
It does not verify a durable transaction or enforce retry use at runtime.
MEDULLA-DECISION-ENGINE-R113.2 tracks the atomic transaction.
MEDULLA-DECISION-ENGINE-R113.3 tracks retry enforcement.

### Planned runtime checks for MEDULLA-DECISION-ENGINE-R113

The refusal handler fixture exists; native execution remains pending. Other transaction fixtures remain planned and unexecuted.
Planned test names do not identify existing tests.
The transaction and durable runtime rows remain SPECIFIED. The local retry model has direct native proof.
Each fixture is a bounded acceptance slice. It does not include the missing transaction implementation.

On 2026-10-11, source inspection found separate write owners:

- `crates/medulla-persistence-records/src/agent_library.rs` opens `agent_library.redb` with `AgentLibraryOwner`.
- `crates/medulla-persistence-records/src/decision_record.rs::commit_decision_record` writes through that owner.
- `crates/medulla-persistence-records/src/agent_graph/write.rs` publishes the graph and its synthesis evidence separately.
- `crates/medulla-resource-store/src/redb_store/capacity_lease.rs::commit` uses a graph-scoped `ShardWrite` and a separate commit.
- `crates/medulla-h-decide/src/decide/commit.rs::commit` calls only the record commit path.

No current call joins these owners into the requested transaction.
A shared transaction and recovery design is a prerequisite for `.2.2` to `.2.4`.
That design must preserve the existing owner boundaries and avoid a second capacity authority.
The implementation exceeds the five-minute fixture slices. It needs further decomposition after that design decision.
The refusal guard in `.2.1` must remain until the transaction path exists.
The durable denial record and consumption operation for `.3.2` are also missing.
The request fixture in `.3.3` depends on those operations.

| Requirement | Intended source or test file | Planned proving test | State |
|---|---|---|---|
| MEDULLA-DECISION-ENGINE-R113.2.1 | `crates/medulla-h-decide/src/decide/commit.rs` | `tests::topology_commit_refuses_uncoordinated_owners` | Handler fixture added; native execution pending |
| MEDULLA-DECISION-ENGINE-R113.2.5 | `crates/medulla-persistence-records/src/decision_record/shared_commit.rs` (new) | `tests::shared_commit_writes_record_and_evidence_in_one_transaction`, `tests::shared_commit_is_blocked_by_an_admission_fence`; L3 `commit_admission_tests.rs::topology_shared_commit_journey` | SPECIFIED; not run |
| MEDULLA-DECISION-ENGINE-R113.2.2 | `crates/medulla-h-decide/src/decide/stat_tests/commit_admission_tests.rs` | `topology_commit_publishes_capacity_and_evidence` | SPECIFIED; not run |
| MEDULLA-DECISION-ENGINE-R113.2.3 | `crates/medulla-h-decide/src/decide/stat_tests/commit_admission_tests.rs` | `topology_capacity_denial_leaves_no_commit_parts` | SPECIFIED; not run |
| MEDULLA-DECISION-ENGINE-R113.2.4 | `crates/medulla-h-decide/src/decide/stat_tests/commit_admission_tests.rs` | `topology_commit_recovery_preserves_atomic_outcome` | SPECIFIED; not run |
| MEDULLA-DECISION-ENGINE-R113.3.1 | `crates/medulla-agent-types/src/agent_topology/retry_allowance.rs` | `tests::retry_allowance_consumes_once`, `tests::invalid_allowances_fail_without_mutation` | Direct native module tests pass; workspace integration not run |
| MEDULLA-DECISION-ENGINE-R113.3.1.1 | `crates/medulla-persistence-records/src/decision_record/retry_allowance.rs` (new) | `decision_record/tests.rs::retry_allowance_table_issues_once_and_consumes_once` | SPECIFIED; not run |
| MEDULLA-DECISION-ENGINE-R113.3.2 | `crates/medulla-persistence-records/src/decision_record/tests.rs` | `topology_denial_allowance_consumption_survives_restart` | SPECIFIED; not run |
| MEDULLA-DECISION-ENGINE-R113.3.3 | `crates/medulla-h-decide/src/decide/commit.rs` | `tests::topology_denial_accepts_one_followup_request` | SPECIFIED; not run |

R113.2 requires all five transaction tests (the shared path `.5` and the three fixtures, plus the refusal guard `.1`). R113.3 requires all four retry tests (the table `.1.1`, the model `.1`, durable consumption `.2` and the handler `.3`).
Neither rollup is runtime acceptance evidence.

### R090.1 native acceptance receipt

`crates/medulla-decision-types/src/decision/statistical/scorer.rs::budget_tests` passed all six native tests with Rust 1.96.0.
`a_budget_at_the_ceiling_is_declared` proves ceiling acceptance.
The parameter, byte, and latency refusal tests prove independent limit enforcement.
`each_budget_axis_accepts_its_boundary_and_refuses_overflow` covers zero, exact independent ceilings, and `u32::MAX` refusals.
The harness imports the unchanged production module with real cached `medulla-types` and `serde` dependencies; no substitutes were used.
The reproducible Cargo command is `cargo test -p medulla-types decision::statistical::scorer::budget_tests`.
The equivalent direct native harness ran; the full Cargo crate build and hosted CI were not run locally.
This receipt covers typed declarations only, not measured latency or the parent R090 benchmark obligation.

The latency boundary accepts 9,999 microseconds and refuses 10,000 microseconds.

### Remaining R090 acceptance

MEDULLA-DECISION-ENGINE-R090.2 requires `tests/test_decision_resident_scorer.py::test_compiled_engine_scores_without_external_runtime`.
MEDULLA-DECISION-ENGINE-R090.3 requires `crates/medulla-numeric/benches/decision_resident_scorer.rs::resident_scorer_meets_declared_budget`.
R090.2 passed in the [Python suite job](https://github.com/Knuckles-Team/medulla/actions/runs/38148086774/job/114499779006)
of hosted Release run 38148086774 at `e5190222b38d628b7d0f268ce4c46b184a170ee1`.
That job passed all 3,702 tests, including the resident runtime tracing case.
The Python job succeeded; the parent Release run failed overall. This is the
scoped compiled-runtime acceptance, not a claim that the parent workflow or all
served decision paths passed. R090.3 has paired native measurements below:
the Cargo launch is inconclusive; the standalone run meets the measured limits.
Budget declarations alone do not establish either result.

### R090.3 native release measurement

The [structured receipt](../../crates/medulla-numeric/benches/decision_resident_scorer.receipt.json)
binds measured source `41cce5dd08d7f27aeacf23b6f01d177dfcbf7c59` to the executable
SHA-256, file length, compiler identity, release flags and native benchmark JSON.
The run used Rust 1.96.0 on `x86_64-unknown-linux-gnu`, optimization 3, thin LTO,
one codegen unit, no debug assertions, and at most three build jobs. The initial
build took 503 seconds within its 900-second bound; total command time was
504.766 seconds. There was no package-profile override or facade build.

| Case | Parameters | p99 thread CPU (microseconds) |
| --- | ---: | ---: |
| Fitted promotion recipe | 234 | 11.183 |
| Maximum legal head dimensions, 256-row fixture | 1360 | 1412.071 |

Both cases meet the 700,000-parameter and 9,999-microsecond ceilings over 1,000
calls after 100 warmups. The 637,136-byte runner produced a whole-runner peak RSS
of 124,788,736 bytes, above the 8,388,608-byte ceiling. Its scorer-specific resident
memory remains unknown. The native result is **inconclusive**, with exit code 2;
Neither serialized model size nor runner file length resolves this above-ceiling
RSS result. It remains preserved in the first receipt.

### R090.3 standalone corroboration

The [standalone receipt](../../crates/medulla-numeric/benches/decision_resident_scorer.standalone.receipt.json)
records the exact same executable, with no rebuild, fixture changes, reduced
samples or baseline subtraction. A separate shell ran it under `/usr/bin/time -v`.
Its SHA-256 remains `b648cd00dc5dae3d5648b269f5245db0175c3c5ae3d9f51611e4dea501c260a5`.

The native peak RSS was 2,850,816 bytes; external time reported exactly 2,784 KiB.
Both are below 8 MiB. The fitted and 256-row fixture p99 CPU measurements were
7.713 and 1205.546 microseconds; parameters remained 234 and 1360. The run exited
0 in 0.99 seconds after the same 100 warmups and 1,000 samples per case.
The standalone native fixture therefore meets all measured resource limits with
verified release provenance. This does not establish served parent acceptance.
The difference between launch methods is observed; startup or inherited
high-water effects remain hypotheses, not an established causal attribution.

### R093.2 planned served acceptance

These tests are planned and have not run.

| Row | Proving test | Required observation |
|---|---|---|
| MEDULLA-DECISION-ENGINE-R093.2.1 | `tests/synthetic/test_synthetic_decision_export.py::test_corpus_collects_seeded_served_results` | Exported candidates match recorded served-client results. |
| MEDULLA-DECISION-ENGINE-R093.2.2 | `tests/synthetic/test_synthetic_decision_export.py::test_served_labels_require_collected_results` | Invalid provenance and labels fail closed. |
| MEDULLA-DECISION-ENGINE-R093.2.3 | `tests/synthetic/test_served_corpus_training.py::test_training_consumes_served_ground_truth` | Training consumes episodes collected from a real server. |

### Planned proofs for MEDULLA-DECISION-ENGINE-R130.4 to R137.100

Each entry below is planned. The test file or function does not exist in the tree yet; the row that binds it adds it at the stated post-split path.

- `MEDULLA-DECISION-ENGINE-R130.4`: `crates/medulla-h-decide/src/decide/stat_tests/rank_tests.rs::uncalibrated_option_abstains_with_its_code` (planned, L3): proves uncalibrated option abstains. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R130.5`: `crates/medulla-h-decide/src/decide/stat_tests/rank_tests.rs::tied_pair_records_tie_break_premises` (planned, L3): proves ranking premises in the sealed record. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R130.6`: `crates/medulla-h-decide/src/decide/stat_tests/rank_tests.rs::tied_codes_resolve_by_item_score_end_to_end` (planned, L3): proves hot-path proof for calibrated ranking. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R131`: `crates/medulla-h-decide/src/decide/stat_tests/limit_guard_tests.rs::breaching_action_abstains_and_commit_records_the_breach` (planned, L3): proves limit guard refuses a breaching action with a typed reason (rollup). The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R131.3`: `crates/medulla-h-decide/src/decide/stat_tests/limit_guard_tests.rs::breaching_action_abstains_and_commit_records_the_breach` (planned, L3): proves limit stage wired into Decide. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R131.4`: `crates/medulla-h-decide/src/decide/stat_tests/limit_guard_tests.rs::breach_abstains_naming_the_first_rule` (planned, L3): proves limit breach abstention. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R131.5`: `crates/medulla-h-decide/src/decide/stat_tests/limit_guard_tests.rs::committed_breach_writes_one_observation_linked_to_the_signal` (planned, L3): proves committed breach becomes an observation linked to the signal. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R131.6`: `crates/medulla-h-decide/src/decide/stat_tests/limit_guard_tests.rs::breaching_action_abstains_and_commit_records_the_breach` (planned, L3): proves hot-path proof for the limit guard. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R132`: `crates/medulla-h-decide/src/decide/stat_tests/entry_trust_tests.rs::unverified_entry_cannot_override_a_verified_fact` (planned, L3): proves retrieved entries carry a trust tier and a poison check gates premises (rollup). The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R132.3`: `crates/medulla-h-decide/src/decide/stat_tests/entry_trust_tests.rs::unverified_entry_cannot_override_a_verified_fact` (planned, L3): proves entry admission wired into Decide. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R132.4`: `crates/medulla-h-decide/src/decide/stat_tests/entry_trust_tests.rs::poisoned_only_required_fact_abstains` (planned, L3): proves poisoned-entry abstention. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R132.5`: `crates/medulla-h-decide/src/decide/stat_tests/entry_trust_tests.rs::refused_entries_are_recorded_and_replay` (planned, L3): proves refused entries and tiers in the sealed record. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R132.6`: `crates/medulla-h-decide/src/decide/stat_tests/entry_trust_tests.rs::unverified_entry_cannot_override_a_verified_fact` (planned, L3): proves hot-path proof for entry trust. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133`: `crates/medulla-h-decide/src/decide/stat_tests/runtime_quota_tests.rs::runtime_quota_journey` (planned, L3): proves run traces as typed facts and runtime quotas (rollup). The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.1`: `crates/medulla-h-decide/src/decide/stat_tests/runtime_quota_tests.rs::runtime_quota_journey` (planned, L3): proves quota and run-trace records. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.2`: `crates/medulla-h-decide/src/decide/stat_tests/runtime_quota_tests.rs::runtime_quota_journey` (planned, L3): proves quota resolution only tightens. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.3`: `crates/medulla-h-decide/src/decide/stat_tests/runtime_quota_tests.rs::cross_tenant_task_class_is_not_read` (planned, L3): proves runtime quota family wiring. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.4`: `crates/medulla-h-decide/src/decide/stat_tests/runtime_quota_tests.rs::above_ceiling_refuses_before_task_lookup` (planned, L3): proves quota refusals and the missing-task-class answer. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.4`: `crates/medulla-h-decide/src/decide/stat_tests/runtime_quota_tests.rs::missing_task_class_answers_defaults` (planned, L3): proves quota refusals and the missing-task-class answer. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.5`: `crates/medulla-h-decide/src/decide/stat_tests/runtime_quota_tests.rs::premises_name_each_value_and_default_source` (planned, L3): proves quota premises. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134`: `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::decide_routes_automatic_task_plan` (planned, L3): proves tool selection, decomposition and planning (rollup). The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.1`: `crates/medulla-decision-types/src/decision/task_plan/tests.rs::aggregation_round_trips` (planned, L3): proves model. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.1`: `crates/medulla-decision-types/src/decision/task_plan/tests.rs::aggregation_off_the_output_task_refuses` (planned, L3): proves model. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.1`: `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::decide_routes_automatic_task_plan` (planned, L3): proves model. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.2`: `crates/medulla-decision-stats/src/decision/tool_relevance.rs::tests::relevant_tools_fit_budget` (planned, L1): proves behavior: tool relevance ranking and covering subset. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.2`: `crates/medulla-h-decide/src/decide/stat_tests/tool_relevance_tests.rs::served_ranking_selects_covering_subset` (planned, L3): proves behavior: tool relevance ranking and covering subset. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.3`: `crates/medulla-decision-types/src/decision/task_plan/tests.rs::independent_subtasks_yield_no_edges_and_operator` (planned, L3): proves behavior: task decomposition. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.3`: `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::decide_routes_automatic_task_plan` (planned, L3): proves behavior: task decomposition. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.4`: `crates/medulla-decision-stats/src/decision/skill_plan.rs::tests::skill_plan_respects_dependencies` (planned, L3): proves behavior: planning over the skill library. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.4`: `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::decide_routes_automatic_task_plan` (planned, L3): proves behavior: planning over the skill library. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.5`: `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::decide_routes_automatic_task_plan` (planned, L3): proves wiring: automatic task plans through `Decide`. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.8`: `tests/decide/test_task_plan_served.py::test_automatic_plan_end_to_end` (planned, L4): proves hot-path proof: automatic plan served end to end. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.8`: `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::decide_routes_automatic_task_plan` (planned, L3): proves hot-path proof: automatic plan served end to end. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R136.7`: `crates/medulla-h-decide/src/decide/stat_tests/evolution_round_tests.rs::served_verdict_and_allocation_journey` (planned, L3): proves hot-path proof: served round journey. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137`: `crates/medulla-h-decide/src/decide/stat_tests/context_budget_tests.rs::over_window_journey` (planned, L3): proves context budget, recursive limits and certified package selection (rollup). The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137`: `crates/medulla-h-decide/src/decide/stat_tests/context_package_tests.rs::package_journey` (planned, L3): proves context budget, recursive limits and certified package selection (rollup). The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.3`: `crates/medulla-h-decide/src/decide/stat_tests/over_window_plan_tests.rs::window_source_order` (planned, L3): proves over-window plan family wiring. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.3`: `crates/medulla-h-decide/src/decide/stat_tests/over_window_plan_tests.rs::cross_tenant_profile_is_not_read` (planned, L3): proves over-window plan family wiring. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.4`: `crates/medulla-h-decide/src/decide/stat_tests/over_window_plan_tests.rs::budget_exhausted_names_the_limit` (planned, L3): proves over-window refusals. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.7`: `crates/medulla-decision-types/src/decision/context_package.rs::tests::package_records_round_trip` (planned, L1): proves context package records. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.7`: `crates/medulla-h-decide/src/decide/stat_tests/context_package_tests.rs::package_journey` (planned, L3): proves context package records. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.8`: `crates/medulla-decision-policy/src/solve/context_package/tests.rs::matches_tiny_multiresolution_oracle` (planned, L3): proves package lowering and exact selection kernel. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.8`: `crates/medulla-decision-policy/src/solve/context_package/tests.rs::each_ceiling_can_be_the_tightest` (planned, L3): proves package lowering and exact selection kernel. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.8`: `crates/medulla-decision-policy/src/solve/context_package/tests.rs::order_does_not_change_the_optimum` (planned, L3): proves package lowering and exact selection kernel. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.8`: `crates/medulla-h-decide/src/decide/stat_tests/context_package_tests.rs::package_journey` (planned, L3): proves package lowering and exact selection kernel. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.9`: `crates/medulla-h-decide/src/decide/stat_tests/context_package_tests.rs::decide_routes_package_selection` (planned, L3): proves package family wiring with independent certificate check. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.9`: `crates/medulla-h-decide/src/decide/stat_tests/context_package_tests.rs::corrupted_certificate_is_refused` (planned, L3): proves package family wiring with independent certificate check. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.9`: `crates/medulla-h-decide/src/decide/stat_tests/context_package_tests.rs::cross_tenant_candidate_is_not_visible` (planned, L3): proves package family wiring with independent certificate check. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.10`: `crates/medulla-h-decide/src/decide/stat_tests/context_package_tests.rs::package_refusals_are_typed` (planned, L3): proves package refusals and abstentions. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.10`: `crates/medulla-h-decide/src/decide/stat_tests/context_package_tests.rs::stale_source_is_refused` (planned, L3): proves package refusals and abstentions. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.10`: `crates/medulla-h-decide/src/decide/stat_tests/context_package_tests.rs::exhausted_budget_abstains_with_incumbent` (planned, L3): proves package refusals and abstentions. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.11`: `crates/medulla-h-decide/src/decide/stat_tests/context_package_tests.rs::package_evidence_binds_inputs` (planned, L3): proves package evidence. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.11`: `crates/medulla-h-decide/src/decide/stat_tests/context_package_tests.rs::totals_are_recomputed_not_trusted` (planned, L3): proves package evidence. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R138`: `crates/medulla-h-decide/src/decide/stat_tests/probe_verdict_tests.rs::probe_verdict_journey` (planned, L3): proves the engine holds red-team records and returns a verdict for each probe (rollup). The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R138.3`: `crates/medulla-h-decide/src/decide/stat_tests/probe_verdict_tests.rs::decide_routes_probe_verdict` (planned, L3): proves wiring. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R138.4`: `crates/medulla-h-decide/src/decide/stat_tests/probe_verdict_tests.rs::probe_verdict_refusals` (planned, L3): proves refusal. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R138.5`: `crates/medulla-h-decide/src/decide/stat_tests/probe_verdict_tests.rs::premises_name_scorers_and_scores` (planned, L3): proves evidence. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R138.6`: `crates/medulla-h-decide/src/decide/stat_tests/probe_verdict_tests.rs::probe_verdict_journey` (planned, L3): proves hot-path proof. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R138.7`: `crates/medulla-agent-types/src/agent_component/validation.rs::tests::red_team_bodies_validate_and_refuse_unknown_fields` (planned, L1): proves store red-team records as library components. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R138.7`: `crates/medulla-h-decide/src/decide/stat_tests/probe_verdict_tests.rs::probe_verdict_journey` (planned, L3): proves store red-team records as library components. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R139`: `crates/medulla-h-decide/src/decide/stat_tests/comparative_analysis_tests.rs::served_decide_returns_budgeted_plan` (planned, L3): proves comparative-analysis plan producer (rollup). The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R139.1`: `crates/medulla-decision-types/src/decision/comparative_analysis.rs::tests::comparative_types_round_trip` (planned, L1): proves model. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R139.1`: `crates/medulla-h-decide/src/decide/stat_tests/comparative_analysis_tests.rs::served_decide_returns_budgeted_plan` (planned, L3): proves model. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R139.2`: `crates/medulla-decision-stats/src/decision/comparative_analysis.rs::tests::thresholds_inclusive_and_budgets` (planned, L1): proves behaviour. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R139.2`: `crates/medulla-h-decide/src/decide/stat_tests/comparative_analysis_tests.rs::served_decide_returns_budgeted_plan` (planned, L3): proves behaviour. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R139.3`: `crates/medulla-h-decide/src/decide/stat_tests/comparative_analysis_tests.rs::served_decide_returns_budgeted_plan` (planned, L3): proves wiring and authority. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R139.3`: `crates/medulla-h-decide/src/decide/stat_tests/comparative_analysis_tests.rs::other_tenant_candidates_never_enter_the_plan` (planned, L3): proves wiring and authority. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R139.4`: `crates/medulla-h-decide/src/decide/stat_tests/comparative_analysis_tests.rs::comparative_refusals` (planned, L3): proves typed refusal. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R139.5`: `crates/medulla-h-decide/src/decide/stat_tests/comparative_analysis_tests.rs::mutating_a_bound_input_changes_the_plan_digest` (planned, L3): proves evidence. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R100.101`: `crates/medulla-h-decide/src/decide/stat_tests/grounded_eval_tests.rs::grounded_evaluation_is_sealed_with_scorer_isolation` (planned, L3): proves grounded topic coverage and probability-scored unsupported claims (rollup). The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R100.101.1`: `crates/medulla-decision-stats/src/decision/grounded_eval.rs::tests::coverage_sums_numerators_before_dividing` (planned, L1): proves grounded topic coverage. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R100.101.1`: `crates/medulla-h-decide/src/decide/stat_tests/grounded_eval_tests.rs::grounded_evaluation_is_sealed_with_scorer_isolation` (planned, L3): proves grounded topic coverage. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R100.101.2`: `crates/medulla-decision-stats/src/decision/grounded_eval.rs::tests::brier_changes_by_the_formula_when_only_p_varies` (planned, L1): proves soft-label unsupported-claim Brier score. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R100.101.2`: `crates/medulla-h-decide/src/decide/stat_tests/grounded_eval_tests.rs::grounded_evaluation_is_sealed_with_scorer_isolation` (planned, L3): proves soft-label unsupported-claim Brier score. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R100.101.3`: `crates/medulla-h-decide/src/decide/stat_tests/grounded_eval_tests.rs::grounded_evaluation_is_sealed_with_scorer_isolation` (planned, L3): proves grounded evaluation wiring, scorer isolation and evidence. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R001.100`: `crates/medulla-h-decide/src/decide/stat_tests/safety_bound_tests.rs::in_bound_passes_and_out_of_bound_withholds` (planned, L3): proves quantitative safety bounds before a recommendation surfaces. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R001.100.1`: `crates/medulla-decision-stats/src/decision/safety.rs::tests::first_violated_bound_withholds` (planned, L1): proves safety bound types and evaluator. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R001.100.1`: `crates/medulla-h-decide/src/decide/stat_tests/safety_bound_tests.rs::in_bound_passes_and_out_of_bound_withholds` (planned, L3): proves safety bound types and evaluator. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R001.100.2`: `crates/medulla-h-decide/src/decide/stat_tests/safety_bound_tests.rs::missing_stale_and_mismatched_measurements_refuse` (planned, L3): proves served safety bound check and refusals. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R001.101.1`: `crates/medulla-h-decide/src/decide/family/mod.rs::tests::every_question_kind_has_a_family` (L1): proves `DecisionFamily` trait and registry.
- `MEDULLA-DECISION-ENGINE-R001.101.2`: `crates/medulla-h-decide/src/decide/stat_tests/registry_tests.rs::decide_dispatches_every_kind_through_its_family` (L3): proves route `decide_blocking` through the registry.
- `MEDULLA-DECISION-ENGINE-R001.101.3`: `crates/medulla-h-decide/src/decide/stat_tests/registry_tests.rs::assemble_and_decide_text_use_the_registry` (planned, L3): proves route `AgentAssemble` and `DecideText` through the registry. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R001.101.5`: `crates/medulla-decision-types/src/decision/statistical.rs::tests::family_body_and_answer_round_trip` (planned, L1): proves typed family body and answer carriers. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R001.101.5`: `crates/medulla-h-decide/src/decide/stat_tests/registry_tests.rs::decide_dispatches_every_kind_through_its_family` (planned, L3): proves typed family body and answer carriers. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R135.7`: `crates/medulla-h-decide/src/decide/stat_tests/program_store_tests.rs::reusable_pattern_requires_a_verified_preplan_receipt` (planned, L3): proves pre-plan test receipt gates pattern promotion. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R106.1`: `crates/medulla-h-decide/src/decide/stat_tests/council_tests.rs::council_quorum_conflict_and_refusals` (planned, L3): proves council positions coordinate into one evidence-bound decision. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R106.1`: `crates/medulla-decision-types/src/decision/council.rs::tests` (planned, L3): proves council positions coordinate into one evidence-bound decision. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.13`: `crates/medulla-h-decide/src/decide/stat_tests/recursion_mode_tests.rs::thresholds_select_mode` (planned, L3): proves measured signals select direct or recursive mode. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.13`: `crates/medulla-h-decide/src/decide/stat_tests/recursion_mode_tests.rs::bad_signals_refuse` (planned, L3): proves measured signals select direct or recursive mode. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.9`: `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::served_decide_returns_ranked_skill_tree` (planned, L3): proves behavior: ranked skill tree per task. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.14`: `crates/medulla-h-decide/src/decide/stat_tests/context_replacement_tests.rs::replacement_follows_tolerance` (planned, L3): proves held-out comparison gates context replacement. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.14`: `crates/medulla-h-decide/src/decide/stat_tests/context_replacement_tests.rs::incomplete_verdicts_retain_raw` (planned, L3): proves held-out comparison gates context replacement. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.10`: `crates/medulla-h-decide/src/decide/stat_tests/fanout_tests.rs::summary_only_children_fit_the_parent_budget` (planned, L3): proves summary-only child returns and parent budget. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.15`: `crates/medulla-h-decide/src/decide/stat_tests/sub_call_admission_tests.rs::admits_a_matching_call` (planned, L3): proves sub-call signatures are checked before dispatch. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.15`: `crates/medulla-h-decide/src/decide/stat_tests/sub_call_admission_tests.rs::each_mismatch_refuses_with_expected_and_actual` (planned, L3): proves sub-call signatures are checked before dispatch. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.11`: `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::long_plan_splits_and_checkpoints_reconcile` (planned, L3): proves horizon partition and credit checkpoints. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R059.1`: `crates/medulla-h-decide/src/decide/stat_tests/transfer_tests.rs::aggregate_reports_robust_transfer_across_backbones` (planned, L3): proves robust transfer estimate across model backbones (rollup). The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R059.1.1`: `crates/medulla-decision-types/src/decision/transfer.rs::tests::binding_and_estimate_round_trip` (planned, L1): proves model: backbone binding and transfer estimate. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R059.1.1`: `crates/medulla-h-decide/src/decide/stat_tests/transfer_tests.rs::aggregate_reports_robust_transfer_across_backbones` (planned, L3): proves model: backbone binding and transfer estimate. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R059.1.2`: `crates/medulla-decision-stats/src/decision/transfer.rs::tests::robust_rate_is_the_backbone_minimum_and_missing_backbones_refuse` (planned, L1): proves behaviour: minimum over backbones with complete independent rows. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R059.1.2`: `crates/medulla-h-decide/src/decide/stat_tests/transfer_tests.rs::aggregate_reports_robust_transfer_across_backbones` (planned, L3): proves behaviour: minimum over backbones with complete independent rows. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R059.1.3`: `crates/medulla-h-decide/src/decide/stat_tests/transfer_tests.rs::aggregate_reports_robust_transfer_across_backbones` (planned, L3): proves wiring, refusals and evidence of the served transfer read. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R059.2`: `crates/medulla-h-decide/src/decide/stat_tests/transfer_tests.rs::three_independent_successes_make_a_skill_candidate` (L3): proves skill candidates from repeated independent successes; the L1 kernel test `crates/medulla-decision-stats/src/decision/transfer.rs::tests::skill_needs_successes_from_enough_distinct_records` covers the distinct-record floor.
- `MEDULLA-DECISION-ENGINE-R134.12`: `crates/medulla-h-decide/src/decide/stat_tests/trajectory_ladder_tests.rs::ladder_picks_smallest_rung_without_head_and_head_rung_with_one` (planned, L3): proves behavior: width and depth rung choice. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.12.1`: `crates/medulla-h-decide/src/decide/stat_tests/trajectory_ladder_tests.rs::paired_verdict_requires_complete_arms` (planned, L3): proves paired-comparison verdict record. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.13`: `crates/medulla-h-decide/src/decide/stat_tests/synthesis_tests.rs::cited_compatible_findings_account_for_every_analyst` (planned, L3): proves cited analyst synthesis with required abstention. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R029.1`: `crates/medulla-h-decide/src/decide/stat_tests/retrieval_tests.rs::provider_binding_is_independent_and_never_substituted` (planned, L3): proves committed retrieval provider binding with no substitution. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.14`: `crates/medulla-h-decide/src/decide/stat_tests/branch_outcomes_tests.rs::complete_branch_set_yields_ordered_fusion_receipt` (planned, L3): proves branch outcome records and fusion receipt. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R020.1`: `crates/medulla-h-decide/src/decide/stat_tests/exploration_pool_tests.rs::matched_context_exploits_and_unmatched_context_may_explore` (planned, L3): proves context-matched exploitation pool before exploration (rollup). The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R020.1.1`: `crates/medulla-h-decide/src/decide/stat_tests/exploration_pool_tests.rs::only_same_principal_history_matches` (planned, L3): proves context digest sealed and matched against prior records. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R020.1.2`: `crates/medulla-h-decide/src/decide/stat_tests/exploration_pool_tests.rs::matched_context_exploits_and_unmatched_context_may_explore` (planned, L3): proves exploitation pool gates exploration. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.7`: `crates/medulla-h-jobs/src/work_item_tests.rs::nested_trace_ingests_and_reads_back` (planned, L3): proves recursive trace hierarchy and critique read (rollup). The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.7.1`: `crates/medulla-decision-types/src/decision/recursive_trace.rs::tests::snapshot_checks` (planned, L1): proves recursive trace node and snapshot checks. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.7.1`: `crates/medulla-h-jobs/src/work_item_tests.rs::nested_trace_ingests_and_reads_back` (planned, L3): proves recursive trace node and snapshot checks. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.7.2`: `crates/medulla-work-store/src/redb_store/work_item/commit.rs::tests::recursive_trace_nodes_link_to_parents` (planned, L1): proves terminal commit stores recursive trace nodes. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.7.2`: `crates/medulla-h-jobs/src/work_item_tests.rs::cyclic_trace_refuses_without_state_change` (planned, L3): proves terminal commit stores recursive trace nodes. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.7.3`: `crates/medulla-h-jobs/src/work_item_tests.rs::trace_critique_reads_complete_ancestry` (planned, L3): proves authorized trace critique read. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.7.3`: `crates/medulla-h-jobs/src/work_item_tests.rs::trace_critique_refusals_leak_nothing` (planned, L3): proves authorized trace critique read. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.8`: `crates/medulla-work-store/src/redb_store/work_item/commit.rs::tests::fenced_or_retry_commit_writes_no_trace` (planned, L1): proves terminal commit writes the run trace and its produced link. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R133.8`: `crates/medulla-h-jobs/src/work_item_tests.rs::terminal_commit_with_run_trace_receipt` (planned, L3): proves terminal commit writes the run trace and its produced link. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R100.1`: `crates/medulla-h-decide/src/decide/stat_tests/feedback_tests.rs::feedback_labels_reach_the_adapter_fit` (planned, L3): proves feedback examples join decisions with outcomes (rollup). The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R100.1.1`: `crates/medulla-h-decide/src/decide/stat_tests/feedback_tests.rs::feedback_event_joins_once_and_refuses_incomplete_joins` (planned, L3): proves feedback event write. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R100.1.1`: `crates/medulla-decision-types/src/decision/statistical/feedback.rs::tests::feedback_event_round_trips` (planned, L1): proves feedback event write. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R100.1.2`: `crates/medulla-h-decide/src/decide/stat_tests/feedback_tests.rs::feedback_labels_reach_the_adapter_fit` (planned, L3): proves feedback labels reach the adapter fit. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R100.1.2`: `crates/medulla-decision-stats/src/decision/adapter.rs::tests::latest_feedback_label_wins_and_ties_go_negative` (planned, L1): proves feedback labels reach the adapter fit. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R088.2`: `crates/medulla-h-decide/src/decide/stat_tests/replay_tests.rs::event_replay_resumes_from_its_cursor_with_the_same_digest` (planned, L3): proves ordered event replay with a sealed receipt (rollup). The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R088.2.1`: `crates/medulla-decision-stats/src/decision/replay/tests.rs::event_order_refusals` (planned, L3): proves replay event and receipt types. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R088.2.1`: `crates/medulla-h-decide/src/decide/stat_tests/replay_tests.rs::event_replay_resumes_from_its_cursor_with_the_same_digest` (planned, L3): proves replay event and receipt types. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R088.2.2`: `crates/medulla-h-decide/src/decide/stat_tests/replay_tests.rs::event_replay_resumes_from_its_cursor_with_the_same_digest` (planned, L3): proves served event replay. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.16`: `crates/medulla-h-decide/src/decide/stat_tests/context_package_tests.rs::complexity_changes_the_package_at_fixed_remaining` (planned, L3): proves task complexity and remaining tokens bound the package. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.16`: `crates/medulla-h-decide/src/decide/stat_tests/context_package_tests.rs::remaining_changes_the_package_at_fixed_complexity` (planned, L3): proves task complexity and remaining tokens bound the package. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.16`: `crates/medulla-h-decide/src/decide/stat_tests/context_package_tests.rs::negative_remainder_and_tokenizer_mismatch_refuse` (planned, L3): proves task complexity and remaining tokens bound the package. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R029.2`: `crates/medulla-h-decide/src/decide/stat_tests/context_budget_tests.rs::every_source_is_selected_or_omitted_once` (L3): proves complete selected and omitted context-source partition (rollup).
- `MEDULLA-DECISION-ENGINE-R029.2.1`: `crates/medulla-decision-stats/src/decision/context_partition.rs::tests::partition_is_exhaustive_and_disjoint` (planned, L1): proves context-source partition types and rule. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R029.2.1`: `crates/medulla-h-decide/src/decide/stat_tests/context_budget_tests.rs::every_source_is_selected_or_omitted_once` (L3): proves context-source partition types and rule.
- `MEDULLA-DECISION-ENGINE-R029.2.2`: `crates/medulla-h-decide/src/decide/stat_tests/context_budget_tests.rs::refusals_and_unauthorized_sources_leak_nothing` (L3): proves served context-budget family and refusals.
- `MEDULLA-DECISION-ENGINE-R134.15`: `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::critical_chain_picks_longest_weighted_path` (planned, L3): proves weighted critical chain over a task plan. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R135.8`: `crates/medulla-h-decide/src/decide/stat_tests/program_store_tests.rs::equal_keys_reuse_one_program_and_missing_step_refuses` (planned, L3): proves compile typed graph workflows into programs. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R135.8.1`: `crates/medulla-h-decide/src/decide/stat_tests/program_store_tests.rs::new_source_revision_reuses_program_with_new_receipt` (planned, L3): proves compile receipts keep source provenance. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R135.10`: `crates/medulla-decision-types/src/decision/program_store/insight.rs::tests::insight_without_source_rejected` (planned, L1): proves insight model. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R135.10`: `crates/medulla-h-decide/src/decide/stat_tests/insight_tests.rs::served_selection_returns_insights_with_source_runs` (planned, L3): proves insight model. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R135.11`: `crates/medulla-decision-stats/src/decision/insight.rs::tests::consolidation_and_selection_order` (planned, L1): proves behavior: consolidate and select insights. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R135.11`: `crates/medulla-h-decide/src/decide/stat_tests/insight_tests.rs::served_selection_returns_insights_with_source_runs` (planned, L3): proves behavior: consolidate and select insights. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R135.12`: `crates/medulla-h-decide/src/decide/stat_tests/insight_tests.rs::served_selection_returns_insights_with_source_runs` (planned, L3): proves wiring: insight submission and selection. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R135.13`: `crates/medulla-h-decide/src/decide/stat_tests/insight_tests.rs::insight_refusals_store_nothing` (planned, L3): proves refusal: insights. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R135.14`: `crates/medulla-h-decide/src/decide/stat_tests/insight_tests.rs::selection_premises_link_source_runs` (planned, L3): proves evidence: insight selection premises. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R135.15`: `tests/decide/test_insight_selection_served.py::test_post_run_write_then_context_injection` (planned, L4): proves hot-path proof: insight write then selection served. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R135.15`: `crates/medulla-h-decide/src/decide/stat_tests/insight_tests.rs::served_selection_returns_insights_with_source_runs` (planned, L3): proves hot-path proof: insight write then selection served. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R129.100`: `crates/medulla-h-decide/src/decide/stat_tests/rule_promotion_tests.rs::rule_versions_move_through_approval_supersession_and_revocation` (planned, L3): proves versioned rule lifecycle from correction to revocation. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.100`: `crates/medulla-h-decide/src/decide/stat_tests/fanout_tests.rs::fanout_aggregate_applies_the_plan_operator_and_names_gaps` (planned, L3): proves fan-out aggregation decision. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R134.101`: `crates/medulla-h-decide/src/decide/stat_tests/fanout_tests.rs::coordinator_delivers_one_complete_artifact` (planned, L3): proves designated coordinator and one final artifact. The test does not exist yet; the owning change adds it at this exact path.
- `MEDULLA-DECISION-ENGINE-R137.100`: `crates/medulla-h-decide/src/decide/stat_tests/instruction_disclosure_tests.rs::initial_then_runtime_disclosure` (L3): proves relevance-triggered instruction disclosure.
- `MEDULLA-DECISION-ENGINE-R137.100`: `crates/medulla-h-decide/src/decide/stat_tests/instruction_disclosure_tests.rs::stale_or_foreign_version_refuses` (L3): proves relevance-triggered instruction disclosure.
- `MEDULLA-DECISION-ENGINE-R001.101.4`: `crates/medulla-h-decide/src/decide/family/mod.rs::tests::template_compiles` (planned, L1): proves family template for new roots. The test does not exist yet; the owning change adds it at this exact path.

## Static contract checks

Run `python -m pytest -q tests/test_decision_static_contracts.py` without a native engine build.
R074 checks the full component enum and agent-slot references against the closure documentation.
R123 scans Rust sources under `src` and `crates` for duplicate solve DTO declarations.
The scan permits explicitly listed, unrelated domain homonyms.
Negative fixtures add duplicate declarations, remove documented kinds, and introduce an undocumented slot reference.
These checks prove source and documentation consistency only. They do not execute solver behavior.

## Local retry model verification

Run `rustc --edition=2021 --test crates/medulla-agent-types/src/agent_topology/retry_allowance.rs -o /path/to/retry-tests`, then run that binary. This compiles the exact production module without a substitute dependency. The tests cover the 1-to-0 transition, repeated refusal, zero, and all 254 malformed counts. The module remains re-exported through `capacity_commit` for existing callers. This proves only MEDULLA-DECISION-ENGINE-R113.3.1; copies do not share state. Durable and handler enforcement remain required by R113.3.2 and R113.3.3.

## Ingestion and schema integration acceptance

The following fixtures are planned and unexecuted. Their producer-only tests remain valid for the narrower producer children. They do not prove these integration requirements.

| Requirement | Planned fixture | Acceptance state |
|---|---|---|
| MEDULLA-DECISION-ENGINE-R079.2 | `tests/test_decision_ingestion_acceptance.py::test_ingestion_embedding_admission_and_cheaper_index_skip` | SPECIFIED; no executed integration proof |
| MEDULLA-DECISION-ENGINE-R079.3 | `tests/test_decision_ingestion_acceptance.py::test_duplicate_and_oversized_units_do_not_embed` | SPECIFIED; no executed integration proof |
| MEDULLA-DECISION-ENGINE-R079.4 | `crates/medulla-h-admin/src/admin/connector_pack/tests/sql_columns.rs::sql_column_kinds_and_cardinality_control_embedding` | L3 served `SourceIngest` |
| MEDULLA-DECISION-ENGINE-R079.5 | `tests/test_decision_ingestion_acceptance.py::test_retrieval_feedback_demotes_unused_content` | SPECIFIED; no executed integration proof |
| MEDULLA-DECISION-ENGINE-R080.2 | `tests/test_decision_ingestion_acceptance.py::test_real_extractors_follow_the_lowest_sufficient_rung` | SPECIFIED; no executed integration proof |
| MEDULLA-DECISION-ENGINE-R102.2 | `crates/medulla-h-admin/src/graph_schema/approved/tests.rs::schema_repair_requires_real_shadow_ingest_before_activation` | SPECIFIED; no executed integration proof |

## Retention acceptance

The following tests use the served decision path and persistent stores.
Their bindings establish coverage. Native execution remains unrun in this delivery slice.

| Requirement | Proving test | Scope |
|---|---|---|
| MEDULLA-DECISION-ENGINE-R066.1 | `crates/medulla-h-decide/src/decide/stat_tests/retention_tests.rs::library_records_survive_every_retention_window` | Zero windows and maximum time preserve complete library record bytes. |
| MEDULLA-DECISION-ENGINE-R066.2 | `crates/medulla-h-decide/src/decide/stat_tests/retention_tests.rs::graph_inputs_expire_at_the_configured_boundaries` | Graph inputs compact at 100 milliseconds and retire 200 milliseconds later. Summaries retain their digests. |
| MEDULLA-DECISION-ENGINE-R066.3 | `crates/medulla-h-decide/src/decide/stat_tests/retention_tests.rs::evaluate_only_leaves_no_committed_records` | Sixteen evaluations leave statistical and library record stores empty. |
| MEDULLA-DECISION-ENGINE-R066.4 | Planned: `graph_summaries_expire_on_policy_window` | No implementation or test exists. Graph summaries currently remain indefinitely. |

Run `cargo test --lib --features decide,query,blob retention_tests` for the new native fixtures.
The existing graph-log fixture also checks CAS restoration, forged blobs, and retirement.
Run `cargo test --lib --features decide,query,blob graph_sourced_records_are_principal_scoped_not_tenant_scoped_on_log_read` for that fixture.
Local Rust formatting and 12 Python decision-contract checks passed. Those checks do not execute retention behavior.

## Bounded topology oracle

| Requirement | Proving test |
|---|---|
| MEDULLA-DECISION-ENGINE-R110.1 | `crates/medulla-compute/src/assemble/tests/topology_oracle.rs::property_oracle_agrees_with_the_solver_for_instances_up_to_twenty_variables` checks 66 fixtures and twenty actual variables. |

Native execution remains pending after dependency compilation exceeded local resources.
The isolated fixture arithmetic check does not execute the assembly or solver.

### R113.2.1 refusal boundary

The handler rejects solved topology plans with positive per-cell lease demand.
The handler preserves tenant, size, replay, and shape checks before this refusal.
The guard runs before Agent Library initialization or mutation.
The fixture compares both durable owner files and capacity status after refusal.
It also checks foreign-tenant refusal and successful ordinary record publication.
Advisory assembly remains unchanged. The shared transaction and recovery paths remain absent.
Native command: `cargo test --lib --features decide topology_commit_refuses_uncoordinated_owners`.
This command has not run for this change.

## Synthetic aggregate separation

| Requirement | Proving test |
|---|---|
| MEDULLA-DECISION-ENGINE-R122 | `crates/medulla-h-decide/src/decide/stat_tests.rs::fit_evaluate_publish_and_decide_end_to_end` exercises both origins through served handlers. |

The fixture excludes synthetic labels above minimum support and preserves real counts in mixed-origin aggregates.
It also checks synthetic-only logged fitting and the inline provenance guard.
Native execution remains pending.

## Typed provenance acceptance

- R121.1: `crates/medulla-compute/src/assemble/tests/evidence.rs::assembly_evidence_uses_typed_premises_not_labels` exercises actual assembly and replay.
- R121.2: planned `crates/medulla-h-decide/src/decide/stat_tests/classes_tests.rs::graph_claim_and_evidence_control_answer_class` covers graph-source provenance.

With `query`, the same fixture creates enough synthetic forecasts to reach the score support floor.
It checks absent synthetic Brier, log-loss, and ECE rows, plus exact real reputation and learned-reliability counts.

## R118.1.2 cross-surface acceptance prerequisites

`crates/medulla-h-decide/src/decide/stat_tests/reputation_tests.rs` already contains the served decision-log fixture.
`below_the_support_floor_a_reputation_has_no_numbers` covers sparse SQL output.
`source_reliability_is_learned_from_the_visible_log` covers dense UQL learning and invisible-history fallback.
Neither test proves agreement across authority policy, retrieval ranking, and UQL.

Extend this fixture through the actual governed consumers after the plan's source-identity adapter and result contract exist.
A visible sparse subject and a dense peer must share one pool and one materialization.
All three consumers must preserve the sparse subject's typed abstention without producing a numeric reliability.
All three consumers must use the dense subject's same materialized estimate, allowing only documented wire precision differences.
Vary the graph prior and caller-supplied retrieval weight; neither may replace the governed estimate or suppress abstention.
An absent subject must remain distinguishable from a visible subject below the support floor.
Invisible outcomes and another tenant's dense history must not change either result or disclose hidden counts.
The fixture must exercise real served adapters rather than manually assign equal scalar values.

These acceptance tests require the consumer prerequisites described in the plan.

## Harness configuration evidence

- `MEDULLA-DECISION-ENGINE-R128`: A served test submits one `harness_config` question and asserts the committed record names all three parts with premises.
- `MEDULLA-DECISION-ENGINE-R128.1`: `crates/medulla-decision-types/src/decision/statistical.rs::question_kind_declared_tests::harness_config_kind_serialises_snake_case`.
- `MEDULLA-DECISION-ENGINE-R128.2`: `crates/medulla-decision-types/src/decision/statistical/harness_config.rs::tests::body_requires_every_input`.
- `MEDULLA-DECISION-ENGINE-R128.3`: `crates/medulla-decision-types/src/decision/statistical/harness_config.rs::tests::answer_round_trips_through_serde`.
- `MEDULLA-DECISION-ENGINE-R128.4`: `crates/medulla-decision-types/src/decision/statistical/harness_config.rs::tests::policy_override_replaces_default`.
- `MEDULLA-DECISION-ENGINE-R128.5`: `crates/medulla-decision-types/src/decision/statistical/harness_config/context_policy/tests.rs::context_policy_none_only_when_overflow_impossible`.
- `MEDULLA-DECISION-ENGINE-R128.6`: `crates/medulla-decision-types/src/decision/statistical/harness_config.rs::tests::plan_scaffold_rules`.
- `MEDULLA-DECISION-ENGINE-R128.7`: `crates/medulla-decision-types/src/decision/statistical/harness_config.rs::tests::shell_only_needs_both_facts`.
- `MEDULLA-DECISION-ENGINE-R128.8`: `crates/medulla-server-served-tests/src/split_tests/medulla_h_decide_harness_config_tests.rs::unknown_profile_is_refused`.
- `MEDULLA-DECISION-ENGINE-R128.9`: `crates/medulla-server-served-tests/src/split_tests/medulla_h_decide_harness_config_tests.rs::tool_set_reuses_covering_subset`.
- `MEDULLA-DECISION-ENGINE-R128.10`: `crates/medulla-server-served-tests/src/split_tests/medulla_h_decide_harness_config_tests.rs::decide_routes_harness_config`.
- `MEDULLA-DECISION-ENGINE-R128.11`: `crates/medulla-server-served-tests/src/split_tests/medulla_h_decide_harness_config_tests.rs::premises_and_why_not_cover_each_part`.
- `MEDULLA-DECISION-ENGINE-R128.12`: `crates/medulla-server-served-tests/src/split_tests/medulla_h_decide_harness_config_tests.rs::all_parts_abstained_seals_abstention`.
- `MEDULLA-DECISION-ENGINE-R128.13`: `crates/medulla-server-served-tests/src/split_tests/medulla_h_decide_harness_config_tests.rs::harness_config_record_commits`.
- `MEDULLA-DECISION-ENGINE-R128.13.1`: run `absent_harness_evidence_preserves_legacy_bytes_and_digest` and `harness_evidence_roundtrips_and_changes_input_digest` in the producer module. Run `crates/medulla-server-served-tests/src/split_tests/medulla_h_decide_harness_config_tests.rs::harness_config_replay_refuses_a_relabelled_generic_record` through native dispatch. These tests do not establish successful HarnessConfig replay.
- `MEDULLA-DECISION-ENGINE-R128.13.1.1`: run `absent_harness_evidence_preserves_legacy_bytes_and_digest` and `harness_evidence_roundtrips_and_changes_input_digest` in the producer module. Run `crates/medulla-server-served-tests/src/split_tests/medulla_h_decide_harness_config_tests.rs::harness_config_replay_refuses_a_relabelled_generic_record` through native dispatch. Require a valid route control commit, rehashed injected-evidence refusal and absence of the mutated record. Both groups must execute on the exact source; the native dispatch result remains pending.
- `MEDULLA-DECISION-ENGINE-R128.13.1.2`: planned benchmarks are `crates/medulla-types/benches/harness_evaluation_evidence.rs::evidence_codec_and_digest_budget` and `benches/harness_config_replay.rs::refuses_attached_evidence_budget`. These benchmarks are unimplemented. Measure against the `HarnessEvidenceLimits` defaults in requirements and retain exact-source optimized-build measurements. Existing token and vector bounds do not prove these limits.
- `MEDULLA-DECISION-ENGINE-R128.13.2`: planned `crates/medulla-server-served-tests/src/split_tests/medulla_h_decide_harness_config_tests.rs::harness_config_record_commits` must prove full answer re-derivation and independent stored-record readback. Include changed-premise and forged-answer refusal cases.
- `MEDULLA-DECISION-ENGINE-R128.13.2.1`: planned `crates/medulla-server-served-tests/src/split_tests/medulla_h_decide_harness_config_tests.rs::harness_config_record_commits` must prove full answer re-derivation and independent stored-record readback. Include changed-premise and forged-answer refusal cases. All remain pending.
- `MEDULLA-DECISION-ENGINE-R128.13.2.2`: planned, unimplemented `benches/harness_config_replay.rs::commits_rederived_harness_config_budget` measures complete replay, persistence and readback. Measure against the `HarnessReplayLimits` defaults in requirements; retain exact-source optimized-build measurements.
- `MEDULLA-DECISION-ENGINE-R128.14`: `tests/test_generated_models.py::test_harness_config_kind_is_a_contract_question_kind`.
- `MEDULLA-DECISION-ENGINE-R128.15`: `tests/decide/test_harness_config_served.py::test_harness_config_end_to_end`.

## Committed pack receipt binding

`storage_acceptance::receipt::committed_multi_intent_receipt_binds_all_effects_and_survives_replay` covers MEDULLA-DECISION-ENGINE-R073.4.
The fixture signs served imports and reads the typed receipt, batch and physical outbox from the owner.
It checks three component intents and one pack intent against an independent digest oracle.
Each omission, adjacent swap, topic change, key change and payload change must alter the digest.
Each header name, value, removal and addition must alter the digest.
A fresh nonce replays through `commit_pack_write`; any new owner application panics.
The receipt and batch remain equal, and nonce reuse fails.
The fixture separately checks the served catalog shortcut and unchanged owner rows.

Run `cargo test --lib --features blob committed_multi_intent_receipt_binds_all_effects_and_survives_replay`.
Then run `cargo test --lib --features blob storage_acceptance` for the adjacent served storage fixtures.

## R073.3 and R073.5 production acceptance

| Requirement | Proving tests | Expected evidence |
|---|---|---|
| `MEDULLA-DECISION-ENGINE-R073.3` | `crates/medulla-persistence-records/src/agent_library/open_tests.rs::fresh_open_initializes_all_pack_tables_and_reopens_empty` | Production open exposes five empty pack tables and an empty catalog before and after reopen. The registry contains 21 owner tables. |
| `MEDULLA-DECISION-ENGINE-R073.3` | `crates/medulla-persistence-records/src/agent_library/open_tests.rs::production_open_refuses_each_genuine_predecessor_without_writing` | Every declared Agent Library predecessor fails twice by name. Both failures preserve the exact file bytes. |
| `MEDULLA-DECISION-ENGINE-R073.5` | `crates/medulla-h-admin/src/admin/connector_pack/tests/content_model.rs::a_changed_leaf_revises_transitive_reference_pins_once` and `a_shared_descendant_diamond_revises_each_component_once` | Transitive and diamond references pin the committed revisions once. Repeated imports preserve revisions. |
| `MEDULLA-DECISION-ENGINE-R073.5` | `crates/medulla-h-admin/src/admin/connector_pack/tests/withdrawal.rs::withdrawn_tool_leaves_search_but_keeps_historical_content` | Signed Search initially returns both tools. Withdrawal removes one; the published control remains. Signed historical Content returns exact bytes. A new pin refuses with `COMPONENT_WITHDRAWN` and creates no component. |

The existing lineage tests retain their independent file-digest and manifest-order assertions.
The existing admin holder test retains its historical Content assertions.
- `MEDULLA-DECISION-ENGINE-R129`: `crates/medulla-agent-types/src/agent_guardrail.rs::tests::promotion_obeys_invariants`.
- `MEDULLA-DECISION-ENGINE-R129.1`: `crates/medulla-h-decide/src/decide/stat_tests/rule_promotion_tests.rs::attach_point_is_reached`.
- `MEDULLA-DECISION-ENGINE-R129.2`: `crates/medulla-agent-types/src/agent_guardrail.rs::tests::verdict_serialises_snake_case` roundtrips both exact snake_case wire variants and preserves the invariant IRI; `verdict_rejects_unknown_variants_and_malformed_payloads` refuses wrong-case/unknown variants, missing or non-string IRIs, and unknown payload fields. These type tests do not prove the invariant gate.
- `MEDULLA-DECISION-ENGINE-R129.3`: `crates/medulla-agent-types/src/agent_guardrail.rs::tests::first_contradicted_invariant_is_named`.
- `MEDULLA-DECISION-ENGINE-R129.4`: `crates/medulla-agent-types/src/agent_guardrail.rs::tests::full_consensus_does_not_override`.
- `MEDULLA-DECISION-ENGINE-R129.5`: `crates/medulla-h-decide/src/decide/stat_tests/rule_promotion_tests.rs::rejected_rule_is_audited_not_active`.
- `MEDULLA-DECISION-ENGINE-R129.6`: `tests/decide/test_rule_promotion_served.py::test_invariant_blocks_promotion`.

### MEDULLA-DECISION-ENGINE-R130 tests

- `MEDULLA-DECISION-ENGINE-R130.1`: `crates/medulla-epistemic/src/ranking.rs::tests::ranked_candidate_round_trips`.
- `MEDULLA-DECISION-ENGINE-R130.2`: `crates/medulla-epistemic/src/ranking.rs::tests::equal_codes_order_by_item_score`.
- `MEDULLA-DECISION-ENGINE-R130.3`: `tests/decide/test_ranked_decide_served.py::test_decide_orders_by_calibrated_score`.
- `MEDULLA-DECISION-ENGINE-R130.4`: `crates/medulla-epistemic/src/ranking.rs::tests::uncalibrated_candidate_abstains`.
- `MEDULLA-DECISION-ENGINE-R130.5`: `crates/medulla-compute/src/assemble/tests/ranking_premises.rs::premises_name_tie_break`.
- `MEDULLA-DECISION-ENGINE-R130.6`: `tests/decide/test_ranked_decide_served.py::test_tied_codes_resolve_by_item_score`.

### MEDULLA-DECISION-ENGINE-R131 tests

- `MEDULLA-DECISION-ENGINE-R131.1`: `crates/medulla-decision-outcome-types/src/decision/limit_guard.rs::tests::limit_types_round_trip`.
- `MEDULLA-DECISION-ENGINE-R131.2`: `crates/medulla-decision-outcome-types/src/decision/limit_guard.rs::tests::check_returns_all_breaches`.
- `MEDULLA-DECISION-ENGINE-R131.3`: `tests/decide/test_limit_guard_served.py::test_breach_refuses`.
- `MEDULLA-DECISION-ENGINE-R131.4`: `crates/medulla-compute/src/assemble/tests/limit_refusal.rs::breach_abstains_with_rule_id`.
- `MEDULLA-DECISION-ENGINE-R131.5`: `crates/medulla-compute/src/assemble/tests/limit_refusal.rs::breach_observation_links_signal`.
- `MEDULLA-DECISION-ENGINE-R131.6`: `tests/decide/test_limit_guard_served.py::test_oversize_order_refused_and_recorded`.

### R090.3 reproducible build-then-standalone protocol

Build in the preserved native workspace, then measure the exact emitted
executable in a separate shell. Do not rebuild between the provenance capture
and standalone run:

```bash
CARGO_BUILD_JOBS=3 cargo bench --locked -p medulla-numeric --features decision \
  --bench decision_resident_scorer --profile release --no-run
```

Use the executable path emitted by Cargo; do not guess a filename or select a
stale binary by glob. Record its SHA-256 and file length, the source commit,
Rust version, target, host CPU/OS, command and effective profile. In a separate
shell, run `/usr/bin/time -v <exact-emitted-executable> --bench`, retain the native
JSON and structured external RSS/exit/timing values, and confirm the hash again.
All measured scorer code must use optimization 3. No opt0 substitute, fixture
reduction, baseline subtraction or rebuild is permitted for this comparison.
Debug assertions are refused; the external provenance receipt also verifies
optimization. Both historical launch receipts above remain preserved.

The harness emits schema-version 1 JSON (schema: `crates/medulla-numeric/benches/decision_resident_scorer.schema.json`): `cases` contains fixture shape, actual trainable scalar count (linear weights plus seven attention tensors), calibration scalar count, warmup/sample counts, nearest-rank p99 nanoseconds and microseconds, and parameter/latency acceptance. Both fixtures execute 100 warmups and 1,000 individually clocked production `forward::read` calls. Fit/construction, validation, and result destruction are outside each timing window. The fitted case reuses the 200-label, seed-0, 60-iteration promotion recipe and asserts nonzero trained self weights; the second case covers maximum legal head dimensions (width 16, features 32, shortlist 64) at 256 rows. LegalSet permits varying row counts, so this does not cover every legal workload.

`memory.bytes` is Linux whole-runner lifetime peak RSS, including fitting and allocator overhead. At or below 8,388,608 bytes it proves a conservative upper bound; above that it is inconclusive for scorer-specific residency and leaves R090.3 unverified. No baseline subtraction or serialized-payload proxy is used. `runner_binary_bytes` measures the executable actually running; `server_binary_bytes` and scorer-specific bytes are null. Exact parameter/latency limits, one-unit excesses, RSS inconclusiveness, nearest-rank indexing, invalid-head refusal and out-of-distribution refusal are checked before measurement. Exit 0 means the measured upper bounds pass but build provenance still needs review; exit 1 means parameter/CPU failure; exit 2 means inconclusive memory or unsupported OS. The recorded standalone run meets these measured limits; its receipt provides the required release provenance. This does not establish served parent acceptance.

### R090.3 production dependency comparison after main integration

Current main `40fefdd6cb66c3c432e73da5966a1aea6f9a9c4e` was merged normally.
Against measured source `41cce5dd08d7f27aeacf23b6f01d177dfcbf7c59`, all 214
files compared across numeric production and test fixtures, shared decision
and contract types, shared test support, both crate manifests, root manifest,
lockfile, toolchain and Cargo configuration are byte-identical. The benchmark
changes only its literal spec-binding comment. The shared types crate also has
new agent-component model facts outside those unchanged roots; its whole crate
is therefore not byte-identical. This is a scoped scorer/fixture input comparison,
not a claim that a newly linked final-HEAD executable would have the same hash.
No unrelated rebuild was substituted for the exact measured-source receipts.

### MEDULLA-DECISION-ENGINE-R132 tests

- `MEDULLA-DECISION-ENGINE-R132.1`: `crates/medulla-decision-outcome-types/src/decision/entry_trust.rs::tests::entry_trust_round_trips`.
- `MEDULLA-DECISION-ENGINE-R132.2`: `crates/medulla-decision-outcome-types/src/decision/entry_trust.rs::tests::lower_tier_never_overrides_higher`.
- `MEDULLA-DECISION-ENGINE-R132.3`: `crates/medulla-compute/src/assemble/tests/entry_trust_wiring.rs::admit_runs_before_premises`.
- `MEDULLA-DECISION-ENGINE-R132.4`: `crates/medulla-compute/src/assemble/tests/entry_trust_refusal.rs::poisoned_only_premise_abstains`.
- `MEDULLA-DECISION-ENGINE-R132.5`: `crates/medulla-compute/src/assemble/tests/entry_trust_refusal.rs::refused_entries_are_recorded`.
- `MEDULLA-DECISION-ENGINE-R132.6`: `tests/decide/test_entry_trust_served.py::test_unverified_entry_cannot_override_verified`.

### MEDULLA-DECISION-ENGINE-R133 tests

- `MEDULLA-DECISION-ENGINE-R133.1`: `crates/medulla-decision-types/src/decision/run_governance.rs::tests::quota_round_trips`.
- `MEDULLA-DECISION-ENGINE-R133.2`: `crates/medulla-decision-types/src/decision/run_governance.rs::tests::request_only_tightens_quota`: omission uses D; tightening uses R; D < R <= C uses D; R > C refuses before clamping.
- `MEDULLA-DECISION-ENGINE-R133.3`: `crates/medulla-h-decide/src/decide/stat_tests/runtime_quota_tests.rs::decide_routes_runtime_quota`.
- `MEDULLA-DECISION-ENGINE-R133.4`: `crates/medulla-h-decide/src/decide/stat_tests/runtime_quota_tests.rs::missing_task_class_abstains_to_default`.
- `MEDULLA-DECISION-ENGINE-R133.5`: `crates/medulla-work-store/src/redb_store/work_item/commit.rs::tests::run_trace_is_linked_to_unit`.
- `MEDULLA-DECISION-ENGINE-R133.6`: `tests/decide/test_runtime_quota_served.py::test_quota_and_trace_end_to_end`: query quotas through read-only Decide, commit the trace through CommitWorkItemResult, then query the produced link; an invalid fence creates neither trace nor link.

### MEDULLA-DECISION-ENGINE-R134 tests

- `MEDULLA-DECISION-ENGINE-R134.1`: `crates/medulla-decision-types/src/decision/task_plan.rs::tests::task_plan_round_trips`.
- `MEDULLA-DECISION-ENGINE-R134.2`: `crates/medulla-decision-types/src/decision/task_plan.rs::tests::relevant_tools_fit_budget`: floor/top-k filtering precedes R045 coverage-per-token selection; include cost/id ties, a lower-relevance coverage winner, and uncovered-capability refusal.
- `MEDULLA-DECISION-ENGINE-R134.3`: `crates/medulla-decision-types/src/decision/task_plan.rs::tests::independent_subtasks_set_flag_and_operator`.
- `MEDULLA-DECISION-ENGINE-R134.4`: `crates/medulla-decision-types/src/decision/task_plan.rs::tests::plan_respects_dependencies`.
- `MEDULLA-DECISION-ENGINE-R134.5`: `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::decide_routes_task_plan`.
- `MEDULLA-DECISION-ENGINE-R134.6`: `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::uncovered_subtask_abstains`.
- `MEDULLA-DECISION-ENGINE-R134.7`: `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::premises_name_skill_and_flag`.
- `MEDULLA-DECISION-ENGINE-R134.8`: `tests/decide/test_task_plan_served.py::test_plan_end_to_end`.

### MEDULLA-DECISION-ENGINE-R135 tests

- `MEDULLA-DECISION-ENGINE-R135.1`: `crates/medulla-decision-types/src/decision/program_store.rs::tests::program_round_trips`.
- `MEDULLA-DECISION-ENGINE-R135.2`: `crates/medulla-decision-types/src/decision/program_store.rs::tests::literals_become_parameters`.
- `MEDULLA-DECISION-ENGINE-R135.3`: `crates/medulla-h-decide/src/decide/stat_tests/program_store_tests.rs::decide_routes_program_lookup`.
- `MEDULLA-DECISION-ENGINE-R135.4`: `crates/medulla-h-decide/src/decide/stat_tests/program_store_tests.rs::overlong_trace_is_refused`.
- `MEDULLA-DECISION-ENGINE-R135.5`: `crates/medulla-h-decide/src/decide/stat_tests/program_store_tests.rs::program_links_source_trace`.
- `MEDULLA-DECISION-ENGINE-R135.6`: `tests/decide/test_program_store_served.py::test_compile_then_lookup`.

### MEDULLA-DECISION-ENGINE-R136 tests

- `MEDULLA-DECISION-ENGINE-R136.1`: `crates/medulla-decision-outcome-types/src/decision/evolution_round.rs::tests::round_types_round_trip`.
- `MEDULLA-DECISION-ENGINE-R136.2`: `crates/medulla-decision-outcome-types/src/decision/evolution_round.rs::tests::verdict_follows_flip_fraction`, including five declared-direction and five opposite flips at 0.5 yielding `unverified`; add `crates/medulla-decision-outcome-types/src/decision/evolution_round.rs::tests::simultaneous_thresholds_are_unverified`.
- `MEDULLA-DECISION-ENGINE-R136.3`: `crates/medulla-decision-outcome-types/src/decision/evolution_round.rs::tests::stronger_models_go_to_high_impact_stages`.
- `MEDULLA-DECISION-ENGINE-R136.4`: `crates/medulla-h-decide/src/decide/stat_tests/evolution_round_tests.rs::decide_routes_round_kinds`.
- `MEDULLA-DECISION-ENGINE-R136.5`: `crates/medulla-h-decide/src/decide/stat_tests/evolution_round_tests.rs::empty_prediction_abstains`.
- `MEDULLA-DECISION-ENGINE-R136.6`: `crates/medulla-h-decide/src/decide/stat_tests/evolution_round_tests.rs::premises_name_counts_and_budget`.
- `MEDULLA-DECISION-ENGINE-R136.7`: `tests/decide/test_evolution_round_served.py::test_verdict_and_allocation`.

### MEDULLA-DECISION-ENGINE-R137 tests

- `MEDULLA-DECISION-ENGINE-R137.1`: `crates/medulla-decision-outcome-types/src/decision/context_budget.rs::tests::context_budget_round_trips`.
- `MEDULLA-DECISION-ENGINE-R137.2`: `crates/medulla-decision-outcome-types/src/decision/context_budget.rs::tests::over_window_prompt_is_split_within_limits`.
- `MEDULLA-DECISION-ENGINE-R137.3`: `crates/medulla-h-decide/src/decide/stat_tests/over_window_plan_tests.rs::decide_routes_context_budget`.
- `MEDULLA-DECISION-ENGINE-R137.4`: `crates/medulla-h-decide/src/decide/stat_tests/over_window_plan_tests.rs::too_many_items_is_refused`.
- `MEDULLA-DECISION-ENGINE-R137.5`: `crates/medulla-h-decide/src/decide/stat_tests/over_window_plan_tests.rs::premises_name_window_and_limits`.
- `MEDULLA-DECISION-ENGINE-R137.6`: `tests/decide/test_context_budget_served.py::test_over_window_plan_end_to_end`.

### MEDULLA-DECISION-ENGINE-R138 tests

- `MEDULLA-DECISION-ENGINE-R138.1`: `crates/medulla-decision-outcome-types/src/decision/red_team.rs::tests::red_team_records_round_trip`.
- `MEDULLA-DECISION-ENGINE-R138.2`: `crates/medulla-decision-outcome-types/src/decision/red_team.rs::tests::one_passing_scorer_compromises`.
- `MEDULLA-DECISION-ENGINE-R138.3`: `crates/medulla-h-decide/src/decide/stat_tests/red_team_tests.rs::decide_routes_probe_verdict`.
- `MEDULLA-DECISION-ENGINE-R138.4`: `crates/medulla-h-decide/src/decide/stat_tests/red_team_tests.rs::missing_scorer_abstains`.
- `MEDULLA-DECISION-ENGINE-R138.5`: `crates/medulla-h-decide/src/decide/stat_tests/red_team_tests.rs::premises_name_scorers_and_scores`.
- `MEDULLA-DECISION-ENGINE-R138.6`: `tests/decide/test_red_team_served.py::test_probe_verdict_end_to_end`.

### MEDULLA-DECISION-ENGINE-R137 package tests (planned)

These are acceptance targets, not execution evidence. Retain every existing R137.1–.6 test.

- `MEDULLA-DECISION-ENGINE-R137.7`: `crates/medulla-decision-types/src/decision/context_package.rs::tests::package_records_round_trip`. Round trips preserve every constraint, source/range/revision and measurement identity. Validation rejects duplicate candidate ids, inconsistent resolution groups and malformed ranges. Negative/nonfinite measurements and arithmetic overflow refuse before solving.
- `MEDULLA-DECISION-ENGINE-R137.8`: `crates/medulla-decision-policy/src/solve/tests/context_package.rs::matches_tiny_multiresolution_oracle` (planned). Exhaustive tiny models match the optimum and fewer-token tie-break. Independent window, output reservation, caller, money and latency cases each become the tightest bound; fractional and zero-capacity boundaries never overrun. Changing candidate order must not change objective or totals; remaining ties use the recorded stable order.
- `MEDULLA-DECISION-ENGINE-R137.9`: `crates/medulla-h-decide/src/decide/stat_tests/context_budget_tests.rs::decide_routes_package_selection` (planned). An authenticated served `Decide` request reaches package lowering, solver, independent verifier and sealed typed answer. The test rejects an unwired helper or local selection that replaces any step. Package selection, whole-prompt fit and R137.1–.6 recursive decomposition remain distinct operations.
- `MEDULLA-DECISION-ENGINE-R137.10`: `crates/medulla-h-decide/src/decide/stat_tests/context_budget_tests.rs::package_refusals_are_typed` (planned). Negative cases cover invalid/missing measurements, no fitting nonempty package, empty/zero-capacity results and stale revisions. Work exhaustion tests cover no incumbent and unacceptable gaps. Certificate tests corrupt the proof, digests and selection bindings. No refusal silently relaxes a ceiling or returns a certified selection.
- `MEDULLA-DECISION-ENGINE-R137.11`: `crates/medulla-h-decide/src/decide/stat_tests/context_budget_tests.rs::package_evidence_binds_inputs` (planned). Changes to unchosen candidates, token weights, tokenizer, cited ranges or revisions invalidate reuse. So do changes to active rates, output reservation, ceilings or objective. Independent recomputation checks selection, totals and certificate against the bound model. The record states the weakest evidence class and all defaults used.
- `MEDULLA-DECISION-ENGINE-R137.12`: `tests/decide/test_context_package_served.py::test_certified_package_end_to_end` (planned). The Medulla served receipt records source SHA, features/configuration, exact command, input/model/certificate digests and results. The linked Meristem receipt records its SHA, served caller and applied package/citations. Cover reserved output, monetary/latency ceilings, caller/window limits, fractional rounding, zero capacity, unknown rates, stale sources and rejected certificates. Unit-only solves, mock transports, greedy fits, planned test names and documentation commits do not establish this proof.

<!-- spec-definition-patch:comparative-analysis-r139 -->
### MEDULLA-DECISION-ENGINE-R139 tests (planned)

These are acceptance targets only, not implementation or execution evidence. Retain R137/R138 tests unchanged.

- `MEDULLA-DECISION-ENGINE-R139.1`: `crates/medulla-decision-types/src/decision/comparative_analysis.rs::tests::plan_round_trips_and_binds_all_stages`; preserve every revision, digest, candidate/order, threshold, budget, refusal and question kind; reject duplicate/over-limit candidates.
- `MEDULLA-DECISION-ENGINE-R139.2`: `crates/medulla-decision-types/src/decision/comparative_analysis.rs::tests::thresholds_and_budgets_select_only_admissible_stages`; cover inclusive 0.50/0.80, one/two synthesis calls, stable ties, zero discovery calls, extraction exclusion and total equality/overflow.
- `MEDULLA-DECISION-ENGINE-R139.3`: `crates/medulla-h-decide/src/decide/stat_tests/comparative_analysis_tests.rs::decide_returns_plan_without_model_execution`; run actual Decide dispatch and assert pinned inputs and zero discovery calls.
- `MEDULLA-DECISION-ENGINE-R139.4`: `crates/medulla-h-decide/src/decide/stat_tests/comparative_analysis_tests.rs::stale_inputs_and_stage_budgets_refuse_before_calls`; cover stale query/vector/universe, invalid weights/thresholds, stage and total exhaustion with typed refusals and no model call.
- `MEDULLA-DECISION-ENGINE-R139.5`: `crates/medulla-decision-types/src/decision/comparative_analysis.rs::tests::plan_digest_binds_inputs_and_budgets`; every bound mutation changes digest and Meristem receipt validation rejects a different plan or over-budget calls.
- `MEDULLA-DECISION-ENGINE-R139.6`: Medulla served target `tests/decide/test_comparative_analysis_served.py::test_plan_and_refusal_through_served_decide`; Meristem consumer target `tests/decide/test_comparative_analysis_consumer.py::test_served_consumer_obeys_plan_and_binds_invocation_receipt`. Require exact source SHAs and separate receipts; a Medulla-only test cannot establish Meristem invocation.
<!-- /spec-definition-patch -->

<!-- spec-definition-patch:owner-batch-20261012-decision-engine -->
## Reviewed residual acceptance targets

All targets below are planned acceptance, not executed tests or receipts. Use the real producer/consumer boundary named for each row and retain existing covered behavior.

- **`MEDULLA-DECISION-ENGINE-R135.7` / AU:18:** Through the served `handle_decide` → `ProgramLookup` path, prove one valid `PrePlanTestReceiptV1` is bound identically into the plan, verifier result and promoted `CompiledProgram`. Independently read back command/repository revisions, timestamps, result digest and artifact manifest. Missing, late, mismatched command/revision, nonzero exit, empty/unreadable manifest and stale evidence each block promotion. Meristem producer/verifier acceptance remains pending and is not inferred from `MERISTEM-HARNESS-R022.2`.

- **`MEDULLA-DECISION-ENGINE-R106.1` / AU:19:** Through served `Method::Decide`, test complete quorum, deterministic conflict resolution and explicit abstention. Each result must retain the full input-position set and evidence digests. Missing/duplicate position, stale evidence, wrong council/topology revision and cross-tenant ref refuse. Independently read back the sealed decision. Meristem position producer remains pending; `MERISTEM-CONTROL-R010` covers topology facts only.

- **`MEDULLA-DECISION-ENGINE-R137.13` / AU:25:** Through served `Method::Decide` and `run_full_rlm`, vary tool-output bytes and graph-node count independently and jointly at policy boundaries. Assert the selected ladder rung, mode and bound observation/policy digests. Missing, stale, invalid, altered-digest and cross-tenant signals refuse before execution. Keep R137.7-.12 package-selection cases separate; Meristem measurement producer/consumer acceptance remains pending.

- **`MEDULLA-DECISION-ENGINE-R134.9` planned served acceptance:** Case R134.9-ranked-plan. Served Decide(TaskPlan) returns multiple skill nodes. Each has stable parent links, cited context matches, pinned outcome refs, score/rank and plan digest. Equal scores use the declared tie-break. Refuse missing/stale context or outcome evidence and malformed/cyclic parent links. Meristem application of this rank is not asserted. Attach the proposed result at plan_over_skills -> AgentLibraryStore -> served TaskPlan. MERISTEM-HARNESS-R022.3 local ranking and MERISTEM-CONTEXT-R001 feedback authority remain existing adjacent obligations. The Meristem consumer that applies Medulla rank and the feedback-to-R140.102 receipt contract are not yet specified. Keep SPECIFIED pending those owner definitions and served evidence. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `ced4d0bb75fa260e4291aea5f71c28b407a95fc3e06047666e1278b043a3cdbd`.

- **`MEDULLA-DECISION-ENGINE-R137.14` planned served acceptance:** Case R137.14-paired-context. a complete held-out set within the declared tolerance permits the replacement verdict. A beyond-tolerance result, missing query or stale context/evaluator digest refuses. Undefined metric, aggregation or tolerance also refuses. An incomplete verdict set leaves the active raw-context revision unchanged. Meristem ContextManager.before_model_request -> run_agent is the existing replacement point. The Meristem held-out evaluator and receipt schema are absent. So is the consumer that atomically applies Medulla acceptance. R013.5/.10/.12/.13 remain adjacent coverage, not evidence of this gate. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `7e1405981aa8b697a73eb7e8c3c3170be8edefc674c23beef740884450fc1add`.

- **`MEDULLA-DECISION-ENGINE-R134.10` planned served acceptance:** Case R134.10-bounded-child-summary. valid summary-only children whose checked token total fits the parent remaining budget are linked to decision, child and summary digests. Refuse raw or oversized output and missing, duplicate or unknown children. Refuse tokenizer/budget revision mismatch or totals above the bound before parent synthesis. Meristem _fork_fanout -> FanoutResult and parallel workspace synthesis are the observed attachment points; Medulla owns TaskPlan authority. Meristem summary producer, parent budget field and consumer validation/readback receipt are missing. MERISTEM-CONTROL-R043.2/.5, MERISTEM-HARNESS-R020.4 and MERISTEM-HARNESS-R024.1 remain adjacent. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `742906721351687d032991b4e9c0255baf7a6c480e21ac7a616975d40e38ce84`.

- **`MEDULLA-DECISION-ENGINE-R137.15` planned served acceptance:** Case R137.15-signature-boundary. an admitted typed call records matching signature/model/tool/input/output digests. Refuse unknown or changed signatures and unadmitted tools. Refuse malformed input/output, missing parent span or revision mismatch before accepting a result. Meristem run_full_rlm -> RLMEnvironment.run_parallel_sub_calls is the observed execution point. Medulla resource limits and span instrumentation are adjacent only. No Meristem recursive signature producer/validator receipt or exact Medulla-to-AU call contract is named. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `4edb15328650ebf084f0ab1d073b092e5b684aafad9b7b1ca7f412859390b923`.

- **`MEDULLA-DECISION-ENGINE-R134.11` planned served acceptance:** Case R134.11-horizon-credit. a plan within the limit remains one horizon. A longer valid plan is split at dependency-safe boundaries and its checkpoint balances reconcile exactly. Refuse an unsplittable dependency component or absent/invalid budget rule. Refuse an over-credit horizon, missing checkpoint or stale plan revision before next launch. Medulla decompose_task/TaskPlan feeds Meristem bounded map primitives, but no named partition/credit receipt interface exists. Meristem durable checkpoint/debit semantics and the consumer that blocks launch on a missing receipt remain unspecified. Do not substitute Finance forecast horizons or solver budgets. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `c1bfd61241a0fd02ada2816f173a3b9568da5b48e490e76058fd740b356886da`.

- **`MEDULLA-DECISION-ENGINE-R059.1` planned served acceptance:** Case R059.1-backbone-evidence. complete independent evaluations for the same canonical skill identity produce a result that lists every backbone and policy revision. Refuse missing/duplicate backbone rows, candidate identity mismatch, reused non-independent evidence, incomplete dataset coverage or absent metric/transferability aggregation policy. Meristem supplies experience/skill candidates; Medulla R059 is the existing independent-outcome admission path. No shared canonical skill merge key, robust aggregation/threshold decision, or Meristem multi-backbone evaluation producer is specified. These decisions are explicit holds. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `0c7c2eed61e3cfe6fe6c863c75fb66092f3a8bea97e3942b635c7b69f715d3a9`.

- **`MEDULLA-DECISION-ENGINE-R134.12` planned served acceptance:** Case R134.12-paired-trajectories. selected width/depth and all trajectory/baseline ids bind to the same plan and input snapshot. Each arm has an outcome and the paired R018 verdict is readable. Refuse missing/mismatched arm, baseline, policy revision or result, and refuse any claim of improvement without the declared paired verdict. Existing points are Decide(TaskPlan), MERISTEM-CONTROL-R042/R043, MERISTEM-HARNESS-R020.4, and paired arms in MERISTEM-HARNESS-R018.1/.2/.5. Meristem trajectory generation, summary aggregation and Medulla selection-policy consumer receipt are absent. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `a28929a50cbfcb691664403ce456fbf9dc38d6bd64b34424932e4fa22f9cbda1`.

- **`MEDULLA-DECISION-ENGINE-R134.13` planned served acceptance:** Case R134.13-cited-synthesis. cited compatible findings produce a result that accounts for every expected analyst. Refuse missing/duplicate analysts and unsupported findings. Refuse unresolved conflict, invalid citation, plan mismatch or absent decision role. Abstain without recommendation. Medulla R134.5 served Decide(TaskPlan) is the planning point; Meristem R020.5 _dispatch_parallel_batch constructs worker slots. The exact Meristem finding producer/collector and aggregation consumer contract is missing. Existing fan-out clauses do not prove it. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `f5ddb0ac9b0329ac031c01c56c49ee03c922a991dbf2d0e29c111515424d3455`.

- **`MEDULLA-DECISION-ENGINE-R029.1` planned served acceptance:** Case R029.1-exact-provider. independently vary provider and reasoning-model ids while verifying the committed provider is executed. If the bound provider is unavailable, stale, unauthorized or mismatched, refuse before retrieval and record zero calls to alternate providers. Preserve ARBORA-OPS-R021.2.1 retrieval.search and its tenant-scoped retriever. Medulla must commit the provider binding. The Graft SDK/Arbora consumer contract that executes and receipts that exact binding is not defined. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `c9552392eb813763a83fc96fb48ad1bd18fd805ef608736f1dae5d48ab12c984`.

- **`MEDULLA-DECISION-ENGINE-R134.14` planned served acceptance:** Case R134.14-branch-readback. a complete branch set produces an ordered fusion receipt and independent readback for every outcome. A later branch cites the exact pinned prior outcome. Refuse missing/duplicate branch, changed parent/input/snapshot, omitted failure gap, absent fusion policy, or stale readback. Existing points are Decide R134.5, Meristem R020.5 worker execution and Meristem R022.4 outcome distillation. The Meristem branch-experience producer, committed-plan consumer and durable per-outcome receipt schema remain missing. Do not substitute R140.102 breadth adaptation. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `4fd31dd504f158f43bcf77d81a39c2419303a538b32553a77a39f6b5a575e9d2`.

- **`MEDULLA-DECISION-ENGINE-R020.1` planned served acceptance:** Case R020.1-agent-isolated-pools. a same-agent matching trajectory selects from the exploitation pool and excludes exploration. A declared no-match case may select from exploration. Refuse missing match policy/evidence and cross-tenant or cross-agent history. Bind result to the candidate snapshot. Medulla owns candidate selection and MERISTEM-HARNESS-R022.1/.3 provide adjacent experience inputs. No Meristem per-agent pool producer or context-match evidence contract exists. Exact match dimensions/threshold must be supplied by that owner policy before implementation. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `dafa8396b5a7c2a168253c7be219718c78186266610e5a91e7dc318d35fc552b`.

- **`MEDULLA-DECISION-ENGINE-R133.7` planned served acceptance:** Case R133.7-trace-critique. ingest and independently read back a nested trace, then critique it at the same authorized snapshot with complete ancestry. Refuse incomplete ancestry, cycle, stale cursor/snapshot, missing digest or unauthorized projection without returning protected fields. Terminal CommitWorkItemResult adapts traces, AddNode validates records and graph queries read them. The recursive node producer and named authorized critique-query operation/consumer are absent. Existing fencing/projection rules remain binding. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `9e0be988c182da3c7f440c14d2fc8e56f60426e543e3f53ba39f250f235e6c8d`.

- **`MEDULLA-DECISION-ENGINE-R100.1` planned served acceptance:** Case R100.1-feedback-join. an authorized complete event is read back as exactly one fit example bound to the expected R100 dataset digest. Replaying the same event is idempotent. Refuse duplicate identities with changed content or missing decision/option/outcome joins. Refuse invalid mapping policy, stale source revision or evidence mismatch before fitting. R100 persists examples, R101 applies a promoted adapter post-RLS/pre-ANN and R040 gates publish. The Meristem signal producer and outcome-to-label mapping policy are not specified. Do not treat an independent outcome record as the producer contract. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `eca485ffdff9cea5394130b53e12a0edc235e342ab01accc36ad9d06f496daa8`.

- **`MEDULLA-DECISION-ENGINE-R088.2` planned served acceptance:** Case R088.2-ordered-replay. replay a fixed event sequence and resume from its receipt/cursor to the same event order and sealed digest. Refuse gaps, out-of-order events, conflicting duplicate ids, changed snapshots or policy-dependent on-policy claims without R119 routing. DecisionEval provides AS OF and fold replay. The real-world event simulator and Meristem adaptive-harness producer/consumer are undefined. This row specifies Medulla replay envelope and refusal only, not an external event source. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `d01db99df0c6c22b37f82cd815e129956fa151e7da2b670b73348ca5058e629e`.

- **`MEDULLA-DECISION-ENGINE-R137.16` planned served acceptance:** Case R137.16-complexity-and-remaining-budget. vary complexity while holding model cost and remaining tokens fixed, then vary remaining tokens while holding complexity fixed. Verify the declared policy and certified package bind both decisions. Refuse missing/invalid policy, tokenizer or snapshot mismatch, negative/overflowing remainder, or package cost above remaining tokens. Planned Decide(ContextBudget) -> existing solver -> independent certificate verifier -> sealed ContextPackageSelection -> MERISTEM-CONTEXT-R004. R137.11/.12 name the consumer. The dispatch and receipt binding remain pending as recorded in R137.9. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `ccbc35ba3595763696c824434868079aeb401ce8b601041e913b323a2d2fb575`.

- **`MEDULLA-DECISION-ENGINE-R029.2` planned served acceptance:** Case R029.2-complete-source-partition. the same served compile journey includes relevant selected sources and omitted examples for irrelevance, over-budget and authority displacement. Every authorized source is accounted for once. Refuse incomplete enumeration, duplicate source, unknown reason, stale budget/authority snapshot or unauthorized disclosure. Served Decide(ContextBudget) -> certified package -> MERISTEM-CONTEXT-R004 context compile; R072 ontology selection and MEDULLA-SRC-AUTH-R010.6 provide distinct source inputs. The Meristem consumer receipt for the exhaustive partition is not named. Preserve the parent retrieval/authorization obligations. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `05fcbe47e588141000ec329927406270abd30b590d2624f22e9239079f829d38`.

- **`MEDULLA-DECISION-ENGINE-R134.15` planned served acceptance:** Case R134.15-weighted-critical-chain. a branched DAG returns the independently calculated longest weighted path and total, source-duration/weight provenance, and deterministic tie result. Refuse cycles and missing/invalid duration or weight. Refuse absent scale policy, stale source evidence or checked multiplication/sum overflow before emitting TaskPlan. Attach the proposed fields to served Decide(TaskPlan) -> plan_over_skills -> typed TaskPlan. The execution consumer schema for critical-chain fields is not defined. The decision producer and consumer remain SPECIFIED pending that schema. No implementation, served test, or receipt is claimed by this SPECIFIED definition. Source review `6e1f0c3ab1626a00c8392e8a304b4c4a28a86d0aee8fff1933317bbdb4893966`.

- **`MEDULLA-DECISION-ENGINE-R135.8` planned served acceptance:** For NL/graph, equal graph digests, compiler id/revision and sorted config refs share one semantic key. The output must be a complete program. Keep a separate WorkflowCompileReceiptV1 for each source, with its kind, revision, digest and mapping. A new source revision reuses the program and adds a receipt. Any changed semantic-key component changes the key. Refuse missing steps with exact location, stale revisions or partial publication. The served compiler/storage interface remains unspecified. No implementation, served test or receipt is claimed by this SPECIFIED definition. Source review `1ee5f2b5bd5bc45c9b9ff8ccaab8de944ed41eceab17a8c1e88cc86d4725e2dc`.
<!-- /spec-definition-patch -->

<!-- spec-definition-patch:reviewed-mapped-subsets-20261012 -->
## Planned mapped acceptance

No test named here has run. Names describe acceptance obligations, not evidence.

- **`MEDULLA-DECISION-ENGINE-R042.100` (AU:28. Source `9dac8e5f41f6948885215836733ce3116b8246ca5c9c4725a12a62bdb11cb400`):** In served Decide, prefer an adequately sampled higher-success path. Skip threshold-qualified repeated failures. Preserve order for missing, stale or undersampled history. Verify the advisory cannot trigger write-back and every score recomputes from its bound counts. Producer/consumer boundary. Medulla R042 remains evaluate-only connector tool-choice advisory; Graft SDK owns ordered transport fallback and current attempt outcomes. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R042.101` (AU:86. Source `38315c5f9a088c33c4b81b88b813ed851f97f454d788c9fb07185c88c752f979`):** For each paired fixture, show necessary/not-necessary/unknown results. At the Meristem call boundary prove every disagreement creates a linked trace event and matching decisions create none. Stale or insufficient evidence must not become a necessity verdict. Served Medulla and Meristem receipts remain separate and linked by decision/measurement ids. Producer/consumer boundary. R134.2 relevance is only a candidate prefilter. MERISTEM-HARNESS-R018.5 generic profile metrics do not measure tool necessity. Meristem run_agent per-tool spans are the consumer boundary. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R129.100` (AU:38. Source `1d54abebd7cbc92bd3d6c397f334b42d602b4c3b9f77e0091f1c683774701d72`):** Served tests cover correction/output mismatch refusal and proposal-only state. They cover approval plus invariant pass, invariant rejection, version replacement and revocation. A later invocation must use only the approved version. Every state transition reads back with actor, version and source-output lineage. Existing R129 invariant behavior stays mandatory. Producer/consumer boundary. Extend R129 promotion authority and Meristem future-invocation application; R034 contradiction-retraction remains separate. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R134.100` (AU:30. Source `8f2ff3030a71d67b20cd19301f0ed8cef75e975403595cec4060abf9fa8952c1`):** At served `agents.run` -> Meristem `_fork_fanout` -> Medulla aggregation, cover all-success results and named failure gaps. Refuse missing, duplicate or stale receipts and concurrent-budget excess. Independently read the artifact back. The output must bind the same selected operation/version and exact Meristem/Arbora receipt set. No extrapolated capacity claim is added. Producer/consumer boundary. Meristem owns FanoutResult production; Arbora `agents.run` owns durable instance receipts and capacity/refusal. Medulla owns selected operator and public application/readback contract only. Preserve existing R134.1/.3 and ARBORA-FLEET-R024.5-.9. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R134.101` (AU:94. Source `19331af6d24dacefccebddcae637919b39cb111b800e3bd239a68b42b8ad10c7`):** The served fan-out test sends multiple outputs and a named failed gap through the coordinator. Read back one structured artifact. Refuse absent coordinator, missing/duplicate child accounting, schema-invalid artifact or mismatched digest. Verify a non-coordinator cannot finalize and every child is accounted for. Producer/consumer boundary. Medulla-selected TaskPlan/AggregationOp -> Meristem `_fork_fanout` FanoutResult -> named coordinator -> one final artifact. Retains MERISTEM-CONTROL-R043.2/.4 named gaps. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R137.100` (AU:59. Source `0b55041f0517baa9de449e3d712356a0e252cd194365e09ae559e9067cc8eb1f`):** Through served Decide and the Meristem run-loop consumer, prove initial instructions precede execution. Withhold runtime refinement before its trigger, disclose it at the trigger and trace-link every disclosure. Stale or unauthorized instruction revisions refuse without application. Verify distinct initial/runtime tags and exact version readback. Producer/consumer boundary. Existing Medulla context/decision authority -> Meristem run loop. General context selection, reminder and trace counters do not cover disclosure. Meristem sole writer owns runtime application. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R134.102` (AU:24. Source `7cbecf865c6185e23ec202d541c5f8d33388338af413ac06209709c619fbe200`):** Served Decide matrix proves below-threshold and unstable/incomplete windows refuse, while a complete above-threshold window admits. Mutate a sample, threshold, metric or policy revision and require a new verdict. Meristem consumer demonstrates no delegation effect on refusal. Producer/consumer boundary. Medulla owns authoritative TaskPlan/delegation admission; Meristem executes the admitted plan. Existing retrieval quality-saturation logic is adjacent only. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R136.100` (AU:32. Source `1d12449ecff9c30c5999933324f1f705fd826539eba6182f3e6d2773c263fec3`):** Positive paired test uses identical test digest and environment across before/after revisions and proves fail then pass. Negative cases alter either revision/test digest, omit a receipt, pass before or fail after. Each refuses admission. Read back the admitted proposal with both receipt links. Producer/consumer boundary. Meristem CorrectionProposal/study_admit supplies trace and benchmark; Medulla decides admission. Generic benchmark lift does not replace paired proof. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R031.100` (AU:41. Source `8c69b66d62c833b4da482e38a51efa624c3e3fc924d6fd6d3e2ef114378a8527`):** For served Medulla admission, test a paper relevant to both goals and graph. Also test one relevant to only one, a below-threshold case and duplicate/stale snapshots. Only the both-relevant accepted record yields a selected-claim ingestion request. Read back the decision and source bindings. Do not claim six-hour schedule or provider fetching is implemented. Producer/consumer boundary. Arbora owns six-hour durable scheduler; Graft SDK owns paper fetching/provider receipts; Medulla owns relevance scoring/admission and selected-claim contract. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R036.100` (AU:46. Source `04556089bf2790a52c59ede225b6212ccd90c0f7ca1dfad07b869ac9de6a19a1`):** Test substitution for all four primitive classes. Include compatible/incompatible schema/effect changes. Keep trace-derived proposals inactive until normal admission. Served assembly receipt names each primitive/version and compatibility proof. Producer/consumer boundary. Medulla graph.assemble is composition authority; Meristem HarnessPort executes selected composition. Existing prompt-only optimization proposal path remains and typed HarnessPort is preserved. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R136.101` (AU:47. Source `9f72c0e6cb8d476050ffde54043ba0e37a063bfda05eb3e27020312cd9eeb69b`):** Start the served sequence with satisfied and unsatisfied criteria. Apply process-level feedback. Accept a nonregressing revision and refuse a previously satisfied criterion regression. Mutating rubric or prior report digest invalidates the comparison. Producer/consumer boundary. Meristem ContinuousEvaluationEngine produces per-criterion evidence/report revisions; Medulla evaluates the bound round verdict. Aggregate divergence and self-reflection are not criterion-level nonregression. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R121.100` (AU:54. Source `78c1b58ee2c97f19ad4834981d1b75adeb89c82881710aed055a0cc6f5c36760`):** The served Meristem/Medulla chain verifies a supported step and rejects an under-evidenced step. Downstream conclusions must be absent on failed/uncertain dependencies. Alter proof digest, evidence revision or uncertainty bounds and require re-verification. Trace readback shows the entire dependency chain. Producer/consumer boundary. Medulla owns proof/evidence/verdict; Meristem run execution gates continuation on the returned verdict. Existing Claim/Evidence and ordered trace digest semantics remain intact. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R138.100` (AU:55. Source `3d07d9e40d19b8d4502fcbf3fc2e682f67bcdeaa0730d5d698f7af32767bb785`):** Served admission accepts only a complete receipt matching the proposed artifact digest. Every edge case must be resolved and retested. Negative cases include skipped verifier, unresolved edge case, stale artifact, changed tests and failed recheck. None can activate/write code. Producer/consumer boundary. Meristem generates/proposes code and verifier evidence; Medulla admission consumes it. Existing governed write path retains materialization authority. ProbeVerdict R138.1-.6 is a separate red-team probe capability. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R136.102` (AU:56. Source `712793000145f8e6b1c3ac1de647e91256312eb601def299653a9671de2757ba`):** Serve one fully graded report admitted against a proposal. Refuse missing outcome/rubric, ungraded criteria, wrong artifact digest, stale rubric or absent grade evidence. Read back the proposal and prove no admission side effect on refusal. Producer/consumer boundary. Meristem study_admit/ContinuousEvaluationEngine supplies the report; Medulla decision verdict controls admission. Preserve R019/R021 evidence and per-criterion R136.101 behavior. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R138.101` (AU:83. Source `0c5f2631be262ba69472eb94841b7acfbcfd6d0cb379382c528ccd0efb4d22e7`):** Served review fixtures include one finding from each class, one evidence-backed low-impact false positive, and an injected high-impact finding. Verify five-class coverage, changed-range binding, evidence retention and filtering disposition. Reject missing revision, unsupported class, absent proof or silently omitted high-impact findings. Producer/consumer boundary. Medulla owns review verdict/schema. Named Arbora authenticated changeset-review producer/consumer must be served and linked separately. Existing R138.3 probe endpoint is not that consumer. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R036.101` (AU:107. Source `d179fc0eb33039df3a87efe8a59ce2a635f0e39f360c58f925011448d0401cdc`):** Served Medulla assembly accepts verified agent/skill facts and returns a plan with exact revisions and transformation order. Refuse absent/stale discovery receipts, incompatible transformation schemas or changed artifact digest. Separate Meristem discovery receipt and Overstory sandbox receipt must be linked for end-to-end closure. No runtime implementation is claimed. Producer/consumer boundary. Medulla composition authority -> Meristem PATH discovery and skill execution -> Overstory isolated generated-artifact frame. Existing DESIGN guidance does not provide these producer/consumer receipts. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.

- **`MEDULLA-DECISION-ENGINE-R014.100` (AU:108. Source `2dfb2ab7469f19faf03d91782fa276ed2c613dec2e1d1fce2c629e5b8974b476`):** Tiny exhaustive candidates prove the selected result is the smallest/cost-optimal eligible composition under the declared objective and constraints. Test equal-size/cost ties, unknown facts, inadequate smaller candidate and illegal candidate refusal. The output preserves separate size and cost measurements. No test imposes or removes shortcut comments. Producer/consumer boundary. Reuse R014 exact bounded solver/graph.assemble authority. Meristem records execution cost. Code-size measurement and candidate adequacy feed Medulla. Preserve legal/policy constraints and hold comment-style conflict. Require exact source revision, input/output digests, caller and independent readback where specified. These cases are planned only. No implementation or receipt is asserted.
<!-- /spec-definition-patch -->

<!-- spec-definition-patch:final-mappings-20261012 -->
## Final mapped planned acceptance

No cases listed here have run. They describe required future acceptance, not evidence.

- **`MEDULLA-DECISION-ENGINE-R139.100` / AU:58. Source `f13717cacb9e87ff76591467e17b5f9d76d44577f62d71b57d57c42bc89d37ba`:** Serve one graph paper, one graph codebase and one filesystem source in the same comparison. Assert exact ordered references, revisions and digests in readback. Refuse stale, unauthorized and mismatched graph refs before comparison, while filesystem-only intake remains valid. Meristem consumer receipts remain a separate dependency. Producer/consumer: Medulla owns authorized graph resolution and returns pinned source evidence. Meristem owns StudySource intake, comparison execution and proposal generation. Preserve MERISTEM-HARNESS-R019.2/.3/.5/.6 and do not treat capability lookup as graph-source intake. Parent `MEDULLA-DECISION-ENGINE-R139` remains intact.

- **`MEDULLA-DECISION-ENGINE-R019.100` / GRAPHOS:17. Source `0e4858352b73b18661ed21ef503cc4c4f36e72045c3ffeff8d2094d9f8f8e50a`:** Serve a decision with all three evidence streams and independently read back exact provenance. Refuse a recommendation commit with missing, stale, wrong-reviewer-scope or digest-mismatched approval and prove no decision revision/effect changed. Approve the exact revision, commit once, and verify reviewer provenance and idempotent retry under R019. Producer/consumer: Medulla produces decision/provenance and approval-binding inputs. Arbora owns shared invoke/A2A approval admission. Graft SDK owns governed source effects and receipts. R047 remains the related producer reference. Parent `MEDULLA-DECISION-ENGINE-R019` remains intact.

- **`MEDULLA-DECISION-ENGINE-R029.100` / PIPE:22. Source `0d3964cba3359a3b454f2fd7b61f2b893b3aec0d58613062b859c2201344980c`:** Through the real retrieval command, load a valid file. Assert its config/mode/plan digests are bound in the run record. Refuse missing file, invalid typed field, unknown mode and stale/mismatched mode digest before command effects. Repeat the same command/input and verify identical config, mode and decision bindings. Do not assert result determinism beyond declared source behavior. Producer/consumer: Medulla owns the typed plan/mode decision and digest; Meristem owns command-file loading and execution. Do not conflate plan selection with file loading or add a pipeline runtime orchestrator. Parent `MEDULLA-DECISION-ENGINE-R029` remains intact.

- **`MEDULLA-DECISION-ENGINE-R140.100`. Source `74416937af9a4934ae1f5d1a02e30336deb1535189a07f53fb81fb352f81779f`: Run `test_trajectory_refinement_served.py::test_complexity_selects_single_or_bounded_k` and `::test_complexity_policy_and_k_refusals`. Do not claim Meristem execution. Existing R134.5 Method::Decide TaskPlan accepts task-complexity evidence and returns a digest-bound K decision. The Meristem K execution receipt is pending. These are planned acceptance cases only. No served evidence is claimed.

- **`MEDULLA-DECISION-ENGINE-R140.101`. Source `be286cbd079c08493be1dcc0512aa562b6e2f2fec8d3566437837d91b5f36ba0`: Run `test_trajectory_refinement_served.py::test_projection_preserves_sources_and_links_candidate` and `::test_projection_refuses_bad_ranges_or_missing_parent`. Do not claim Meristem projection served. Attach point and owner dependency. The R134.5 plan binds Meristem projection inputs and the exact result parent set; Meristem R022.4 projection/reinsertion receipt is pending. These are planned acceptance cases only. No served evidence is claimed.

- **`MEDULLA-DECISION-ENGINE-R140.102`. Source `b56fbf074624786b4a8c2b499f985ecc2d14d1e8349d08781cf28aba07c3abef`: Run `test_trajectory_refinement_served.py::test_later_branch_adapts_from_pinned_outcome`, `::test_branch_history_refuses_stale_or_cross_tenant`, and `::test_no_history_uses_declared_initial_breadth`; Meristem receipt tests remain pending. Attach point and owner dependency. The R134.5 TaskPlan route commits `BreadthDecisionV1`; Meristem R020.5/R022.4 must bind it to branch requests and outcomes, a pending consumer contract. These are planned acceptance cases only. No served evidence is claimed.

- **`MEDULLA-DECISION-ENGINE-R140.103`. Source
- MEDULLA-DECISION-ENGINE-R140.104 planned proving test. tests/decide/test_trajectory_refinement_served.py::test_real_decide_route_emits_pinned_refinement_decisions. Enter Method::Decide and verify the TaskPlan decision/policy/source digest binding at the production route. Meristem receipts remain a hold. This test has not run.
- MEDULLA-DECISION-ENGINE-R140.105 planned hot-path integration test. tests/decide/test_trajectory_refinement_served.py::test_taskplan_decisions_reach_au_and_receipts_read_back. Enter the real route, observe the Meristem call and independently read back all decision-bound execution receipts. Blocked on Meristem R140.100-.102 consumer definitions and R140.103 policy/receipt. No test evidence is claimed. `6f67238075c0bf262188d97a1813b18bfb0c71e13c6689a583c788d956287612`: Run `test_pairwise_ranking_served.py::test_refuses_unselected_aggregation_policy`. No positive ranking case is specified until the source decision is made. Attach point and owner dependency. R134.5 TaskPlan/Decide is only the proposed boundary. Emit no ranking/revision until the owner supplies pairwise aggregation/cycle/incomplete-pair/tie policy and keep-count/revision bounds. Require the Meristem comparison receipt too. These are planned acceptance cases only. No served evidence is claimed.

- **`MEDULLA-DECISION-ENGINE-R141.100` planned cases. Source `ac1b5df0212beaade03e78502a494148692e1a0007c3fa68ee37a2bb6dbe8cce`: Named hold acceptance at Method::DecisionEval: test_refuses_unresolved_fidelity_metric refuses an absent/unreviewed fidelity formula or normalization. Test_discards_failed_or_stale_rewrite proves failed policy gates preserve the baseline. A positive Admit fixture is pending review of the fidelity metric semantics. Even after that review, activation/readback remains held until the authoritative memory mutation and atomic CAS path are identified. No active-memory change is claimed. No implementation or receipt is claimed.

- **`MEDULLA-DECISION-ENGINE-R141.101` planned cases
- MEDULLA-DECISION-ENGINE-R141.102 planned proving test. tests/decide/test_memory_change_admission_served.py::test_real_decision_eval_binds_memory_candidate_and_metrics. Enter Method::DecisionEval and verify pinned inputs/refusal. The memory mutation path remains unnamed.
- MEDULLA-DECISION-ENGINE-R141.103 planned hot-path integration test. tests/decide/test_memory_change_admission_served.py::test_memory_mutation_requires_current_decision_eval_receipt. Enter DecisionEval and cross the actual memory CAS gate. Independently read back the active revision. This remains blocked on approved metric formulas and the memory mutator map. No active-memory change or test result is claimed. Source `27a6a74820b2b646163d8f7bd58b94167b9f0a3e423b8bf5210fe6e758deb440`: Named hold acceptance at Method::DecisionEval: test_refuses_unresolved_metric_semantics refuses absent/unreviewed recall@k and answer-accuracy formulas. After review, test_binds_metrics_to_same_snapshots verifies exact numerators and positive denominators. Test_each_failed_metric_blocks_admission verifies independent threshold failures. These passing cases remain pending the formula decision. The authoritative memory mutation map and atomic gate also remain required before end-to-end admission. No implementation or receipt is claimed.

- **`MEDULLA-DECISION-ENGINE-R142.100`. Source
- MEDULLA-DECISION-ENGINE-R142.101 planned proving test. tests/decide/test_local_skill_certification_served.py::test_decision_eval_attestation_binds_au_receipt. Enter the real DecisionEval route and read back an attestation bound to the Meristem receipt and all revisions. Meristem receipt schema and Arbora R034.7-.12 consumption remain explicit holds.
- MEDULLA-DECISION-ENGINE-R142.102 planned hot-path integration test. tests/decide/test_local_skill_certification_served.py::test_current_attestation_is_checked_before_graphos_workflow_run. Enter the evaluation/normal-invocation path. Verify refusal before effects without a current receipt, then admission with the exact receipt. Meristem and Arbora owner contracts are pending. No test result is claimed. `5df3f5a45d486aeba5765a31b6c6283637adf2bd8ce63a87938eddd28b90f082`: Run `test_local_skill_certification_served.py::test_attests_matching_atomic_evaluation`, `::test_workflow_requires_current_atomic_attestations`, and `::test_stale_input_invalidates_attestation`; Meristem and Arbora served tests remain required. Attach point and owner dependency. DecisionEval validates Meristem local-model receipt and emits a Medulla attestation. Normal Arbora invocation must validate it through R034.7-.12. Meristem receipt schema and Arbora consumption remain pending owner contracts. These are planned acceptance cases only. No served evidence is claimed.
<!-- /spec-definition-patch -->


<!-- spec-definition-patch:owner-dispatch-20261012-tests -->
## Planned served acceptance — retrieval evaluation

No test in this section has run.

- `MEDULLA-DECISION-ENGINE-R100.100`: run two registered retrieval algorithms on the same served corpus/query/label revisions. Verify exact hits-at-k inputs/result digests and DecisionEval receipt readback. Exercise `RetrievalContextSearchV1` exact stored-text byte offsets, bounded spans, cursor staleness and authorization. Missing labels, incompatible settings/revisions and incomplete results refuse; Overstory consumes the same receipt.
- Verify `RetrievalContextSearchV1` half-open byte offsets slice the exact original stored UTF-8 bytes for composed/decomposed Unicode and multibyte text. If NFC indexing is used, verify its separate digest and normalized-to-original byte map. Assert empty query sets/zero denominator refuse, plus 100-span/8-KiB bounds and tenant filters. Missing labels, stale corpus, mismatched settings, incomplete outputs, expired cursor and denied context refuse without fabricated metric or protected-count leakage.
<!-- /spec-definition-patch -->

## Planned served acceptance — grounded answers and safety bounds

- `MEDULLA-DECISION-ENGINE-R100.101`: use a complete pinned expected-topic set and separately labeled coverage/support records. Verify exact expected/covered topic ids and summed per-scope counts. Supply one label-blind prediction per scored claim and independent non-unanimous votes. Independently recompute `sum(n_i*(p_i-q_i)^2)/sum(n_i)` and verify the denominator is positive. Hold labels fixed and vary `p_i`; Brier changes exactly while coverage does not. Refuse missing/extra predictions, claims, labels or topics, duplicate annotators, empty denominator, out-of-range probabilities and stale source/scorer/policy digests.
- `MEDULLA-DECISION-ENGINE-R001.100`: pass an in-bound recommendation and withhold one for each out-of-bound metric. Assert metric units, bound id, limit and measured value. Refuse absent/stale/non-finite/unit-incompatible facts and invalid comparison operator. The Arbora pre-surface served receipt is separately required and is not claimed by this Medulla test.

### MEDULLA-DECISION-ENGINE-R172 tests

- `MEDULLA-DECISION-ENGINE-R172.1`: `crates/medulla-decision-types/src/decision/capability_designation.rs::tests::designation_types_round_trip`; `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::designation_journey`.
- `MEDULLA-DECISION-ENGINE-R172.2`: `crates/medulla-decision-memory/src/memory/capability_index/tests.rs::incremental_upsert_and_tombstone_match_rebuild`; `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::designation_journey`.
- `MEDULLA-DECISION-ENGINE-R172.3`: `crates/medulla-decision-types/src/decision/capability_designation.rs::tests::eligibility_gates_match_fixtures`; `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::designation_journey`.
- `MEDULLA-DECISION-ENGINE-R172.4`: `crates/medulla-decision-rank/src/rank/capability_rank.rs::tests::blend_order_matches_fixtures`; `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::designation_journey`.
- `MEDULLA-DECISION-ENGINE-R172.5`: `crates/medulla-decision-types/src/decision/capability_designation.rs::tests::reward_ema_erasure_and_decay`; `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::designation_journey`.
- `MEDULLA-DECISION-ENGINE-R172.6`: `crates/medulla-decision-memory/src/memory/capability_index/tests.rs::version_pin_refuses_mixed_spaces`.
- `MEDULLA-DECISION-ENGINE-R172.7`: `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::decide_routes_capability_designation`.
- `MEDULLA-DECISION-ENGINE-R172.8`: `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::capability_designation_refusals`.
- `MEDULLA-DECISION-ENGINE-R172.9`: `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::premises_name_scores_and_knobs`.
- `MEDULLA-DECISION-ENGINE-R172.10`: `benches/decide_budgets.rs::capability_designation` (bench group `decide/capability_designation`).
- `MEDULLA-DECISION-ENGINE-R172.11`: `tests/decide/test_capability_designation_served.py::test_designation_end_to_end`.
- `MEDULLA-DECISION-ENGINE-R172.12`: Rollup closes when every child `.12.1` to `.12.6` passes its proving test, and `tests/decide/test_capability_designation_pack_served.py::test_designation_reads_imported_pack` passes.
- `MEDULLA-DECISION-ENGINE-R172.12.1`: `crates/medulla-decision-types/src/decision/capability_designation.rs::tests::pack_surface_fields_round_trip`.
- `MEDULLA-DECISION-ENGINE-R172.12.2`: `crates/medulla-decision-types/src/decision/capability_designation.rs::tests::target_class_subsumption_selects_tools`.
- `MEDULLA-DECISION-ENGINE-R172.12.3`: `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::projection_commit_upserts_pack_records`.
- `MEDULLA-DECISION-ENGINE-R172.12.4`: `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::pack_target_refusals`.
- `MEDULLA-DECISION-ENGINE-R172.12.5`: `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::premises_name_subclass_path_and_pack`.
- `MEDULLA-DECISION-ENGINE-R172.12`: Rollup closes when every child `.12.1` to `.12.6` passes its proving test.
- `MEDULLA-DECISION-ENGINE-R172.12.1`: `crates/medulla-decision-types/src/decision/capability_designation.rs::tests::pack_surface_fields_round_trip`.
- `MEDULLA-DECISION-ENGINE-R172.12.2`: `crates/medulla-decision-types/src/decision/capability_designation.rs::tests::target_class_subsumption_selects_tools`.
- `MEDULLA-DECISION-ENGINE-R172.12.3`: `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::projection_commit_upserts_pack_records`.
- `MEDULLA-DECISION-ENGINE-R172.12.4`: `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::pack_target_refusals`.
- `MEDULLA-DECISION-ENGINE-R172.12.5`: `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::premises_name_subclass_path_and_pack`.
- `MEDULLA-DECISION-ENGINE-R172.12.6`: `tests/decide/test_capability_designation_pack_served.py::test_designation_reads_imported_pack`.
- `MEDULLA-DECISION-ENGINE-R172.13`: `crates/medulla-compute/src/capability_index/tests.rs::capability_adapter_applies_before_rank`.

## MEDULLA-DECISION-ENGINE-R175 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Model | `MEDULLA-DECISION-ENGINE-R175.1` | A serde round trip keeps every field and rejects `pause_steps = 9`. |
| Behavior | `MEDULLA-DECISION-ENGINE-R175.2` | A seeded fixture returns the same ranking on two runs. |
| Wiring | `MEDULLA-DECISION-ENGINE-R175.3` | A dispatch test routes the new kind and reads the stored encoder. |
| Refusal | `MEDULLA-DECISION-ENGINE-R175.4` | Each case returns its typed reason and no ranking. |
| Evidence | `MEDULLA-DECISION-ENGINE-R175.5` | A record readback lists the premises and the claim. |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R175.6` | The served test passes, and the benchmark receipt meets the budget. |

Hot-path proof for `MEDULLA-DECISION-ENGINE-R175`: `tests/decide/test_recommend_served.py::test_pause_steps_lift_ndcg`.

## MEDULLA-DECISION-ENGINE-R173 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Model | `MEDULLA-DECISION-ENGINE-R173.1` | A serde round trip keeps every field and rejects `window_rounds = 0`. |
| Behavior | `MEDULLA-DECISION-ENGINE-R173.2` | Unit tests cover the third edit in rounds 2 to 4 and two edits only. |
| Wiring | `MEDULLA-DECISION-ENGINE-R173.3` | A test asserts the same verdict from `Decide` and from `ShaclValidate`. |
| Refusal | `MEDULLA-DECISION-ENGINE-R173.4` | Each case returns its typed reason and no verdict. |
| Evidence | `MEDULLA-DECISION-ENGINE-R173.5` | A record readback lists each field. |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R173.6` | The served test passes, and the benchmark receipt meets the budget. |

Hot-path proof for `MEDULLA-DECISION-ENGINE-R173`: `tests/decide/test_concentration_served.py::test_third_same_dimension_edit_reverts`.

## MEDULLA-DECISION-ENGINE-R134.110 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Model | `MEDULLA-DECISION-ENGINE-R134.110.1` | A serde round trip keeps every field and rejects `top_k = 51`. |
| Behavior | `MEDULLA-DECISION-ENGINE-R134.110.2` | A fixture asserts the fused order and an incremental update. |
| Wiring | `MEDULLA-DECISION-ENGINE-R134.110.3` | A served test changes the catalog and observes the new ranking. |
| Refusal | `MEDULLA-DECISION-ENGINE-R134.110.4` | Each case returns its typed reason and no tools. |
| Evidence | `MEDULLA-DECISION-ENGINE-R134.110.5` | A record readback lists the premise fields. |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R134.110.6` | The served test passes, and the benchmark receipt meets the budget. |

Hot-path proof for `MEDULLA-DECISION-ENGINE-R134.110`: `tests/decide/test_tool_assignment_served.py::test_role_assignment_uses_one_ranking`.

## MEDULLA-DECISION-ENGINE-R100.102 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Model | `MEDULLA-DECISION-ENGINE-R100.102.1` | A serde round trip keeps every field and rejects 257 steps. |
| Behavior | `MEDULLA-DECISION-ENGINE-R100.102.2` | A unit test computes positives, negatives and advantages for a fixture. |
| Wiring | `MEDULLA-DECISION-ENGINE-R100.102.3` | A dispatch test routes the write and reads the stored baseline. |
| Refusal | `MEDULLA-DECISION-ENGINE-R100.102.4` | Each case returns its code and writes nothing. |
| Evidence | `MEDULLA-DECISION-ENGINE-R100.102.5` | A test reads the receipt after one run. |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R100.102.6` | The served test passes, and the benchmark receipt meets the budget. |

Hot-path proof for `MEDULLA-DECISION-ENGINE-R100.102`: `tests/decide/test_run_credit_served.py::test_cited_rows_and_step_credit_recorded`.

## MEDULLA-DECISION-ENGINE-R174 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Model | `MEDULLA-DECISION-ENGINE-R174.1` | A serde round trip keeps every field and rejects an unknown state. |
| Behavior | `MEDULLA-DECISION-ENGINE-R174.2` | A unit test resumes a cycle from a checkpoint and activates once. |
| Wiring | `MEDULLA-DECISION-ENGINE-R174.3` | A served test submits a cycle and observes the job and the lease. |
| Refusal | `MEDULLA-DECISION-ENGINE-R174.4` | Each case returns its code and leaves the active pointer unchanged. |
| Evidence | `MEDULLA-DECISION-ENGINE-R174.5` | A test reads the receipt after one activation. |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R174.6` | The served test passes, and the measured overhead meets the budget. |

Hot-path proof for `MEDULLA-DECISION-ENGINE-R174`: `tests/decide/test_generation_cycle_served.py::test_cycle_resumes_and_activates`.

## MEDULLA-DECISION-ENGINE-R176 acceptance

| Case | Row | Proving test and observation |
|---|---|---|

| Model | `MEDULLA-DECISION-ENGINE-R176.1` | `crates/medulla-decision-outcome-types/src/decision/load_set/tests.rs::load_set_round_trips`: A serde round trip keeps every field and rejects `max_tools` above 128. |

| Behavior | `MEDULLA-DECISION-ENGINE-R176.2` | `crates/medulla-h-decide/src/decide/stat_tests/load_set.rs::bounded_union_in_stable_order`: A job over three packs returns a bounded, duplicate-free set in a stable order. |

| Wiring | `MEDULLA-DECISION-ENGINE-R176.3` | `crates/medulla-h-decide/src/decide/stat_tests/load_set.rs::served_decide_reaches_load_set`: A served `Decide` with a load-set question reaches the new arm. |

| Refusal | `MEDULLA-DECISION-ENGINE-R176.4` | `crates/medulla-h-decide/src/decide/stat_tests/load_set.rs::load_set_refusals`: Each case returns its code, and no partial set hides an abstention. |

| Evidence | `MEDULLA-DECISION-ENGINE-R176.5` | `crates/medulla-h-decide/src/decide/stat_tests/load_set.rs::premises_per_component`: Each chosen tool has one premise with its pack digest. |

| Hot-path proof | `MEDULLA-DECISION-ENGINE-R176.6` | `tests/decide/test_load_set_served.py::test_job_returns_bounded_tool_skill_agent_set`: The answer holds 32 tools or fewer, their skills and one agent start, and meets the time budget. |


Hot-path proof for `MEDULLA-DECISION-ENGINE-R176`: `tests/decide/test_load_set_served.py::test_job_returns_bounded_tool_skill_agent_set`.

## MEDULLA-DECISION-ENGINE-R177 acceptance

| Case | Row | Proving test and observation |
|---|---|---|

| Model | `MEDULLA-DECISION-ENGINE-R177.1` | `crates/medulla-decision-outcome-types/src/decision/usage_guidance/tests.rs::guidance_round_trips`: A serde round trip keeps every field and rejects a step without a tool. |

| Behavior | `MEDULLA-DECISION-ENGINE-R177.2` | `crates/medulla-decision-stats/src/decision/usage_guidance.rs::tests::steps_ordered_by_outcome`: A fixture connector's steps are ordered, and success rate breaks a tie between steps that may swap. `tests::order_is_deterministic` (L2) shuffles the input. |

| Wiring | `MEDULLA-DECISION-ENGINE-R177.3` | `crates/medulla-h-decide/src/decide/stat_tests/usage_guidance.rs::served_decide_reaches_guidance`: A served `Decide` with a guidance question reaches the new arm. |

| Refusal | `MEDULLA-DECISION-ENGINE-R177.4` | `crates/medulla-h-decide/src/decide/stat_tests/usage_guidance.rs::guidance_refusals`: Each case returns its code. |

| Evidence | `MEDULLA-DECISION-ENGINE-R177.5` | `crates/medulla-h-decide/src/decide/stat_tests/usage_guidance.rs::steps_cite_triples`: Each step lists its supporting triples. |

| Hot-path proof | `MEDULLA-DECISION-ENGINE-R177.6` | `tests/decide/test_usage_guidance_served.py::test_how_to_use_connector_answers_from_graph`: The answer lists the steps in the order with the higher success rate, with citations. |


Hot-path proof for `MEDULLA-DECISION-ENGINE-R177`: `tests/decide/test_usage_guidance_served.py::test_how_to_use_connector_answers_from_graph`.
## MEDULLA-DECISION-ENGINE-R174.7 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Model | `MEDULLA-DECISION-ENGINE-R174.7.1` | A serde round trip keeps every field and rejects an unknown scope, side or source. |
| Behavior | `MEDULLA-DECISION-ENGINE-R174.7.2` | Unit tests cover deterministic refit, keep and revert, resolution order and the memory cap. |
| Wiring | `MEDULLA-DECISION-ENGINE-R174.7.3` | A dispatch test routes a scoped fit, an activation and a served query. |
| Refusal | `MEDULLA-DECISION-ENGINE-R174.7.4` | Each case returns its code and leaves the served adapter unchanged. |
| Evidence | `MEDULLA-DECISION-ENGINE-R174.7.5` | A test reads the fit receipt, a rollback receipt and the verdict field. |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R174.7.6` | The served test passes, and the benchmark receipt meets each budget. |

Hot-path proof for `MEDULLA-DECISION-ENGINE-R174.7`: `tests/decide/test_scoped_adapter_served.py::test_outcome_adapter_lifts_and_reverts`.

## MEDULLA-DECISION-ENGINE-R174.8 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Model | `MEDULLA-DECISION-ENGINE-R174.8.1` | A serde round trip keeps every field and rejects 4,097 regions. |
| Behavior | `MEDULLA-DECISION-ENGINE-R174.8.2` | Unit tests fire each signal kind, show hysteresis and select only changed regions. |
| Wiring | `MEDULLA-DECISION-ENGINE-R174.8.3` | A dispatch test observes a monitor update, one plan and one per-shard swap. |
| Refusal | `MEDULLA-DECISION-ENGINE-R174.8.4` | Each case returns its code and leaves every shard generation unchanged. |
| Evidence | `MEDULLA-DECISION-ENGINE-R174.8.5` | A test reads the signal premises and the refresh receipt. |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R174.8.6` | The served test passes, and the benchmark receipt meets each budget. |

Hot-path proof for `MEDULLA-DECISION-ENGINE-R174.8`: `tests/decide/test_drift_refresh_served.py::test_vocabulary_drift_refreshes_changed_shards`.

## MEDULLA-DECISION-ENGINE-R174.9 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Model | `MEDULLA-DECISION-ENGINE-R174.9.1` | A serde round trip keeps every field and rejects 33 queries. |
| Behavior | `MEDULLA-DECISION-ENGINE-R174.9.2` | Unit tests cover canonical URLs, each dedupe outcome, the region mark and both budgets. |
| Wiring | `MEDULLA-DECISION-ENGINE-R174.9.3` | A dispatch test observes one virtual-graph read and one region mark. |
| Wiring | `MEDULLA-DECISION-ENGINE-R174.9.3.1` | A test reads a refresh through the federation web source with a recorded connector port. |
| Refusal | `MEDULLA-DECISION-ENGINE-R174.9.4` | Each case returns its code and writes no row and no region mark. |
| Evidence | `MEDULLA-DECISION-ENGINE-R174.9.5` | A test reads the result provenance and the receipt. |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R174.9.6` | The served test passes, and the measured overhead meets the budget. |

Hot-path proof for `MEDULLA-DECISION-ENGINE-R174.9`: `tests/decide/test_freshness_refresh_served.py::test_search_refresh_dedupes_and_feeds_refresh`.

## MEDULLA-DECISION-ENGINE-R802 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Refresh | `MEDULLA-DECISION-ENGINE-R174.9.7` | `tests/decide/test_freshness_refresh_served.py::test_stale_web_document_refetches_on_digest_change`: An equal digest writes nothing; a changed digest supersedes and marks the region. |
| Model | `MEDULLA-DECISION-ENGINE-R802.1` | `web_fetch_selection.rs::tests::question_round_trips`: A serde round trip keeps every field and rejects 33 fetches. |
| Behavior | `MEDULLA-DECISION-ENGINE-R802.2` | `crates/medulla-decision-stats/src/decision/web_fetch_selection.rs::tests::ranks_dedupes_and_skips_fresh`: Unit tests cover each skip reason, the order and the budget. |
| Wiring | `MEDULLA-DECISION-ENGINE-R802.3` | `tests/web_document_enrich_served.rs::test_web_page_enriches_once_for_both_backends`: A served `SelectWebPages` skips the held and duplicate pages. |
| Refusal | `MEDULLA-DECISION-ENGINE-R802.4` | `crates/medulla-decision-stats/src/decision/web_fetch_selection.rs::tests::web_fetch_refusals`: Each case returns its code and selects no candidate. |
| Evidence | `MEDULLA-DECISION-ENGINE-R802.5` | `crates/medulla-decision-stats/src/decision/web_fetch_selection.rs::tests::premises_per_candidate`: Each candidate has one entry. |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R802.6` | `tests/web_document_enrich_served.rs::test_web_page_enriches_once_for_both_backends`: The served test passes, and `web_fetch_selection` meets the budget. |

Hot-path proof for `MEDULLA-DECISION-ENGINE-R802`: `tests/web_document_enrich_served.rs::test_web_page_enriches_once_for_both_backends`.

## MEDULLA-DECISION-ENGINE-R803 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Model | `MEDULLA-DECISION-ENGINE-R803.1` | `crates/medulla-decision-types/src/decision/harness_contrast.rs::tests::decision_round_trips`: A serde round trip keeps every field. |
| Behavior | `MEDULLA-DECISION-ENGINE-R803.2` | `crates/medulla-decision-types/src/decision/harness_contrast.rs::tests::adoption_requires_positive_delta`: Unit tests cover adoption, tie-break and abstention. |
| Wiring | `MEDULLA-DECISION-ENGINE-R803.3` | `tests/decide/test_harness_contrast_served.py::test_contrast_decide_stores_record`: A served `Decide` call stores one record. |
| Refusal | `MEDULLA-DECISION-ENGINE-R803.4` | `crates/medulla-decision-types/src/decision/harness_contrast.rs::tests::contrast_refusals`: Each case returns its code and stores nothing. |
| Evidence | `MEDULLA-DECISION-ENGINE-R803.5` | `crates/medulla-decision-types/src/decision/harness_contrast.rs::tests::record_pins_evidence_and_replays`: The rerun digest matches, and the supersession citation resolves. |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R803.6` | `tests/decide/test_harness_contrast_served.py::test_contrast_hot_path_served`: The served test passes, and the benchmark meets the budget. |

Hot-path proof for `MEDULLA-DECISION-ENGINE-R803`: `tests/decide/test_harness_contrast_served.py::test_contrast_hot_path_served`.

## MEDULLA-DECISION-ENGINE-R036.102

`crates/medulla-raft-xshard/src/cross_shard_txn/prepare_record.rs::tests` covers the production codec: canonical round trip, wrong transaction/group, modified payload, unsupported version/owner, malformed envelope and legacy array recovery. The existing cross-shard harness must pass with the new writer. Codec tests alone do not qualify heterogeneous-owner admission or distributed recovery.

## Harness self-improvement: evaluation sets, optimizer loop, promotion and rollback

| Row | Proving test |
|---|---|
| `MEDULLA-DECISION-ENGINE-R804` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::judge_only_test_split_refuses` |
| `MEDULLA-DECISION-ENGINE-R804.1` | Retired; see `MEDULLA-DECISION-ENGINE-R813.1`. |
| `MEDULLA-DECISION-ENGINE-R804.2` | Retired; see `MEDULLA-DECISION-ENGINE-R810.2`. |
| `MEDULLA-DECISION-ENGINE-R804.3` | Retired; see `MEDULLA-DECISION-ENGINE-R810.3`. |
| `MEDULLA-DECISION-ENGINE-R804.4` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::judge_only_test_split_refuses` |
| `MEDULLA-DECISION-ENGINE-R804.5` | Retired; see `MEDULLA-DECISION-ENGINE-R810.5`. |
| `MEDULLA-DECISION-ENGINE-R804.6` | Retired; see `MEDULLA-DECISION-ENGINE-R810.6`. |
| `MEDULLA-DECISION-ENGINE-R805` | `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::promote_then_roll_back_every_subject_kind` |
| `MEDULLA-DECISION-ENGINE-R805.1` | `crates/medulla-decision-outcome-types/src/decision/promotion.rs::tests::records_round_trip` |
| `MEDULLA-DECISION-ENGINE-R805.2` | `crates/medulla-decision-stats/src/decision/promotion.rs::tests::regression_triggers_rollback`; `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::promote_then_roll_back_every_subject_kind` |
| `MEDULLA-DECISION-ENGINE-R805.3` | `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::promote_then_roll_back_every_subject_kind` |
| `MEDULLA-DECISION-ENGINE-R805.4` | `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::promotion_refusals` |
| `MEDULLA-DECISION-ENGINE-R805.5` | `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::pointer_history_replays` |
| `MEDULLA-DECISION-ENGINE-R805.6` | `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::served_promotion_journey` |

## Agent-memory substrate claims

| Case | Requirement | Proving test |
|---|---|---|
| Episodic-to-semantic promotion | `MEDULLA-DECISION-ENGINE-R651.7` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::episodes_promote_to_a_linked_semantic_fact`; `tests/memory/test_consolidate_served.py::test_episodes_promote_to_semantic_fact` |
| Reinforce and decay share one salience update | `MEDULLA-DECISION-ENGINE-R651.8` | `crates/medulla-graph-core/src/graph/txn_maintenance.rs::tests::reinforce_and_decay_share_importance_update`; `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::reinforce_then_decay_matches_the_half_life`; `tests/memory/test_consolidate_served.py::test_reinforce_and_decay_share_importance` |
| Contradiction records are tracked | `MEDULLA-DECISION-ENGINE-R700.7` | `crates/medulla-h-decide/src/decide/stat_tests/belief_revision_tests.rs::contradiction_is_tracked_and_status_is_ordered` |

## MEDULLA-DECISION-ENGINE-R810 to R813 acceptance

| Case | Row | Proving test and observation |
|---|---|---|
| Model | `MEDULLA-DECISION-ENGINE-R810.1` | `crates/medulla-program/tests/native_program.rs::program_contracts_round_trip_and_optimizer_catalog_is_closed`; `crates/medulla-h-jobs/src/jobs/program_tests.rs::program_optimize_job_end_to_end` |
| Behavior | `MEDULLA-DECISION-ENGINE-R810.2` | `crates/medulla-program/tests/native_program.rs::cancel_stops_within_one_candidate`; `crates/medulla-h-jobs/src/jobs/program_tests.rs::program_optimize_job_end_to_end` |
| Wiring | `MEDULLA-DECISION-ENGINE-R810.3` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::program_optimize_job_end_to_end` |
| Refusal | `MEDULLA-DECISION-ENGINE-R810.4` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::program_job_refusals` |
| Evidence | `MEDULLA-DECISION-ENGINE-R810.5` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::rerun_with_one_seed_gives_one_revision_digest` |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R810.6` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::served_program_job_journey` |
| Model | `MEDULLA-DECISION-ENGINE-R811.1` | `crates/medulla-decision-outcome-types/src/policy_evolution/tests.rs::every_policy_operation_and_receipt_round_trips`; `crates/medulla-h-decide/src/decide/stat_tests/policy_evolution_tests.rs::capture_train_evaluate_and_promote` |
| Behavior | `MEDULLA-DECISION-ENGINE-R811.2` | `crates/medulla-server-graph/src/dispatch/policy_evolution/gate/tests.rs::evaluation_overlapping_training_refuses`; `crates/medulla-h-decide/src/decide/stat_tests/policy_evolution_tests.rs::capture_train_evaluate_and_promote` |
| Wiring | `MEDULLA-DECISION-ENGINE-R811.3` | `crates/medulla-h-decide/src/decide/stat_tests/policy_evolution_tests.rs::capture_train_evaluate_and_promote` |
| Refusal | `MEDULLA-DECISION-ENGINE-R811.4` | `crates/medulla-h-decide/src/decide/stat_tests/policy_evolution_tests.rs::policy_evolution_refusals` |
| Evidence | `MEDULLA-DECISION-ENGINE-R811.5` | `crates/medulla-h-decide/src/decide/stat_tests/policy_evolution_tests.rs::receipts_read_back_field_for_field` |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R811.6` | `crates/medulla-h-decide/src/decide/stat_tests/policy_evolution_tests.rs::served_policy_evolution_journey` |
| Model | `MEDULLA-DECISION-ENGINE-R812.1` | Retired; see `MEDULLA-DECISION-ENGINE-R805.7`. |
| Behavior | `MEDULLA-DECISION-ENGINE-R812.2` | `crates/medulla-decision-stats/src/decision/promotion.rs::tests::challenger_win_loss_tie_and_judge_refusal`; `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::promote_then_roll_back_every_subject_kind` |
| Wiring | `MEDULLA-DECISION-ENGINE-R812.3` | Retired; see `MEDULLA-DECISION-ENGINE-R805.3`. |
| Refusal | `MEDULLA-DECISION-ENGINE-R812.4` | Retired; see `MEDULLA-DECISION-ENGINE-R805.4`. |
| Evidence | `MEDULLA-DECISION-ENGINE-R812.5` | Retired; see `MEDULLA-DECISION-ENGINE-R805.5`. |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R812.6` | Retired; see `MEDULLA-DECISION-ENGINE-R805.6`. |
| Model | `MEDULLA-DECISION-ENGINE-R813.1` | `crates/medulla-program/tests/native_program.rs::example_round_trips_per_grader_kind`; `crates/medulla-h-decide/src/decide/stat_tests/score_corpus_tests.rs::scores_build_a_corpus_and_compile` |
| Behavior | `MEDULLA-DECISION-ENGINE-R813.2` | `crates/medulla-program/src/corpus.rs::tests::each_grader_kind_and_the_skip`; `crates/medulla-h-decide/src/decide/stat_tests/score_corpus_tests.rs::scores_build_a_corpus_and_compile` |
| Wiring | `MEDULLA-DECISION-ENGINE-R813.3` | `crates/medulla-h-decide/src/decide/stat_tests/score_corpus_tests.rs::scores_build_a_corpus_and_compile` |
| Refusal | `MEDULLA-DECISION-ENGINE-R813.4` | `crates/medulla-h-decide/src/decide/stat_tests/score_corpus_tests.rs::score_refusals` |
| Evidence | `MEDULLA-DECISION-ENGINE-R813.5` | `crates/medulla-h-decide/src/decide/stat_tests/score_corpus_tests.rs::example_names_score_grader_author_and_run` |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R813.6` | `crates/medulla-h-decide/src/decide/stat_tests/score_corpus_tests.rs::served_scores_to_program_journey` |

Hot-path proof for `MEDULLA-DECISION-ENGINE-R810`: `tests/test_program_optimize_served.py::test_program_optimize_job_end_to_end`.

Hot-path proof for `MEDULLA-DECISION-ENGINE-R811`: `tests/test_policy_evolution_served.py::test_capture_train_evaluate_served`.

Hot-path proof for `MEDULLA-DECISION-ENGINE-R812`: `tests/test_program_champion_served.py::test_promote_then_rollback`.

Hot-path proof for `MEDULLA-DECISION-ENGINE-R813`: `tests/test_trace_scores_to_corpus_served.py::test_scores_build_corpus_and_compile`.
## Bounded explicit task graphs (R134.111)

`crates/medulla-decision-types/src/decision/task_plan/tests.rs` covers canonical ordering, round trips and unknown fields.
It covers invalid ids and edges, cycles, output coverage, byte limits and task/concurrency ceilings.
The four-task fixture has independent A and B. C depends only on B. The final task depends on A and C. Type tests do not claim scheduler execution.

`crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs` enters `handle_decide`
with a published feature schema. It verifies the typed plan, caller-claim
premises, fixed-policy provenance, digest binding and strict request refusals.
`tests/decide/test_task_plan_served.py::test_explicit_plan_end_to_end` uses the
actual wire client and a disposable published schema. No external model is used.
Meristem owns rolling execution evidence; Arbora owns authenticated admission evidence.

The shared `verify_task_plan_receipt` helper checks typed content and request bindings.
The served test refuses changed prompts, bounds, tenant, question, caller and digest.
`expected_principal` is required and uses the native persistence-id conversion.
`test_anonymous_task_plan_publication_and_restart` covers explicit isolated no-auth
Decide, local/anonymous provenance, native run ownership, paired publication and
live-owner readback after restart. It does not fabricate authenticated identity.
These checks do not authenticate a caller-supplied receipt or authorize execution.

The `bounded_task_plan` performance receipt for R134.111.6 remains pending.

Typed tool steps (R134.111.8 to R134.111.13) are covered in `crates/medulla-decision-types/src/decision/task_plan/tests.rs` and `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs`.
The bindings are static: no test claims that a path exists in a run-time output.

- `MEDULLA-DECISION-ENGINE-R134.111.8`: `task_plan_round_trips` keeps a tool step, its literals and its bindings. It accepts a mixed plan. It rejects an unknown call field and a `$ref` object with extra keys.
- `MEDULLA-DECISION-ENGINE-R134.111.9`: `binding_canonicalization_is_order_independent` yields one plan digest for reordered argument keys. `binding_bounds` accepts each ceiling and refuses one value past it.
- `MEDULLA-DECISION-ENGINE-R134.111.10`: `refuses_each_binding_defect` returns the reason, task id and argument pointer for all six reasons. A binding to a non-dependency refuses with `binding_not_dependency` and returns no plan.
- `MEDULLA-DECISION-ENGINE-R134.111.11`: `digest_covers_calls_and_bindings` changes the digest for a changed tool, literal, reference or server. `premises_name_calls_and_bindings` lists the premises. The receipt check refuses a receipt whose call differs from the proposal.
- `MEDULLA-DECISION-ENGINE-R134.111.12`: `decide_routes_task_plan_with_calls` enters `handle_decide` and returns the typed plan with calls. The Python test checks that the exported digest equals the new body's digest.
- `MEDULLA-DECISION-ENGINE-R134.111.13`: `tests/decide/test_task_plan_served.py::test_tool_step_bindings_end_to_end`.

- `MEDULLA-DECISION-ENGINE-R814`: See each child.
- `MEDULLA-DECISION-ENGINE-R814.1`: See each child.
- `MEDULLA-DECISION-ENGINE-R814.1.1`: `crates/medulla-rdf/tests/metadata_catalog_classes.rs::catalog_classes_roll_up_to_eg_core_and_align_to_w3c`, `::owner_steward_and_lineage_properties_specialise_prov`, `::catalog_shapes_accept_a_complete_catalog` and `::catalog_shapes_refuse_incomplete_or_open_vocabulary_records`.
- `MEDULLA-DECISION-ENGINE-R814.1.2`: `crates/medulla-types/tests/prompt_candidate.rs::candidate_round_trips_and_kind_follows_role` (planned; typed record is absent from this candidate).
- `MEDULLA-DECISION-ENGINE-R814.2`: `crates/medulla-h-decide/src/decide/stat_tests/prompt_candidate_tests.rs::assemble_selects_covering_candidate_and_abstains_without_one`.
- `MEDULLA-DECISION-ENGINE-R814.3`: `tests/decide/test_prompt_candidates_served.py::test_assemble_reads_candidates_from_imported_pack`.
- `MEDULLA-DECISION-ENGINE-R814.4`: `crates/medulla-h-decide/src/decide/stat_tests/prompt_candidate_tests.rs::unbound_prompt_refuses_and_rejected_connector_is_excluded`.
- `MEDULLA-DECISION-ENGINE-R814.5`: `tests/decide/test_prompt_candidates_served.py::test_record_names_prompt_connector_digest_and_capability`.
- `MEDULLA-DECISION-ENGINE-R814.6`: `tests/decide/test_prompt_candidates_served.py::test_dynamic_agent_gets_the_matching_connector_system_prompt`.
- `MEDULLA-DECISION-ENGINE-R814.7`: `tests/decide/test_prompt_candidates_served.py::test_system_prompt_query_lists_candidates_for_a_task`. Vector: `SELECT ?prompt ?connector ?digest WHERE { ?prompt a :PromptCandidate ; :systemPromptFor ?t ; cp:ofConnector ?connector ; cp:digest ?digest . ?t :requiresCapability ?need . <Y> :requiresCapability ?need }`. `:systemPromptFor` ranges over `:Task`, so the join goes through the task. The query returns the prompt of the connector that serves task Y and not the other connector's prompt.
- `MEDULLA-DECISION-ENGINE-R814.8`: `tests/decide/test_prompt_candidates_served.py::test_non_canonical_prompt_is_refused_with_field_path`.

## Model media declarations: source, generated consumers and qualification

R050.1 source fixtures preserve these six producer tests:

- `agent_component::tests::model_input_capabilities_publish_round_trip_and_bind_every_limit`
- `agent_component::tests::invalid_model_media_declarations_never_publish`
- `agent_component::input_capabilities::tests::selected_profile_enforces_exact_decoded_bounds`
- `agent_component::input_capabilities::tests::declarations_reject_unknown_media_duplicates_and_unbounded_limits`
- `connector_pack::digest::measured_model_tests::pack_model_input_capabilities_are_digest_bound`
- `server::persistence::agent_component::tests::model_input_capabilities_survive_owner_restart_and_tenant_reads`

Also run the existing `absent_shell_proficiency_keeps_digest` component V3/V4
vectors and `measured_pack_model_digest_preserves_absence_and_binds_provenance`
pack vector unchanged. Absent/null capabilities must not change those digests.

The signed pack-import module
`crates/medulla-h-admin/src/admin/connector_pack/tests/model_input_capabilities.rs`
adds two producer fixtures. `declared_model_media_facts_round_trip_through_pack_import`
imports a declaration and reads the complete facts back. It changes only the
byte bound and checks a new component digest/revision with retained old facts.
`nonvision_image_pack_refuses_without_owner_or_body_mutation` covers a fresh
catalog and an already published model. Upload the archive before recording the
baseline. Reject false-vision image facts as InvalidComponent. Compare the owner
snapshot: heads, components, imports, holders, and outbox. Compare blob and chunk
counts. A fixture declaration is not a claim that an image ran.

R050.2 runs `tests/test_generated_model_input_capabilities.py` with no engine.
Both strict generated model classes must accept and preserve all four media values.
They distinguish unknown from unrestricted input. After a successful positive
control, they reject unknown MIME or extra capability fields. Do not skip or
xfail missing generated fields. Check the client fixture in the same change; the
fixture alone is not full contract freshness.

R050.3 requires command/result/commit receipts for all native fixtures above;
formatting and source inspection are not compilation or execution. The planned
`model_media_capabilities_max_parts` benchmark uses four media types and 128
size-only parts. Total supplied size is 64 MiB. The benchmark then exceeds the
byte and count bounds.
Record hardware/profile, latency and peak allocations independently. The
benchmark is not implemented or executed by this producer slice. Selected
profile/run binding, atomic authority/admission, and consumer invocation remain
open. The served PNG journey also remains open under R036/downstream acceptance.
Do not infer it from these fixtures or generated status.

## Participant memory, assistant profiles and memory events

| Case | Requirement | Proving test |
|---|---|---|
| Participant class and record | `MEDULLA-DECISION-ENGINE-R617.7` | `crates/medulla-work-types/src/participant.rs::tests::participant_round_trips_and_iri_is_stable`; `crates/medulla-core/tests/participant_shape.rs::person_without_represents_fails_shape`; `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::participant_journey` |
| Session membership and speaking participant | `MEDULLA-DECISION-ENGINE-R617.8` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::ingest_adds_speakers_as_members`; `crates/medulla-work-types/src/participant.rs::tests::membership_edges_dedupe_speakers` |
| Served participant writes | `MEDULLA-DECISION-ENGINE-R617.9` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::participant_methods_are_served_and_classified`; `tests/memory/test_participant_memory_served.py::test_participant_put_and_member_round_trip` |
| Memory scope narrows and the ACL still applies | `MEDULLA-DECISION-ENGINE-R651.9` | `crates/medulla-types/src/memory_scope.rs::tests::scope_never_exceeds_visible_sessions`; `src/server/handlers/graph_ops/participant_memory_tests.rs::scope_narrows_recall_and_acl_still_applies` |
| Observation configuration per participant per session | `MEDULLA-DECISION-ENGINE-R651.10` | `crates/medulla-numeric/src/decision/participant_derive.rs::tests::targets_follow_flags_and_never_repeat`; `src/server/handlers/graph_ops/participant_memory_tests.rs::observation_flags_decide_which_perspectives_exist` |
| Appending a turn enqueues one derive job | `MEDULLA-DECISION-ENGINE-R651.11` | `src/server/handlers/graph_ops/participant_memory_tests.rs::turn_append_enqueues_one_ordered_job_per_key` |
| Derive worker writes conclusions as claims | `MEDULLA-DECISION-ENGINE-R651.12` | `src/server/handlers/graph_ops/participant_memory_tests.rs::derive_job_writes_claims_with_turn_evidence`; `tests/memory/test_participant_memory_served.py::test_turns_become_perspective_claims` |
| Claims carry subject and observer | `MEDULLA-DECISION-ENGINE-R652.9` | `crates/medulla-epistemic/src/claims.rs::tests::observer_sees_own_and_visible_system_claims_only`; `crates/medulla-h-decide/src/decide/stat_tests/claims_tests.rs::perspective_claims_hide_other_observers` |
| Participant class and record | `MEDULLA-DECISION-ENGINE-R617.7` | `crates/medulla-types/src/participant.rs::tests::participant_round_trips_and_iri_is_stable`; `crates/medulla-core/tests/participant_shape.rs::person_without_represents_fails_shape`; `src/server/handlers/graph_ops/participant_memory_tests.rs::participant_journey` |
| Session membership and speaking participant | `MEDULLA-DECISION-ENGINE-R617.8` | `src/server/handlers/graph_ops/participant_memory_tests.rs::ingest_adds_speakers_as_members`; `crates/medulla-types/src/participant.rs::tests::membership_edges_dedupe_speakers` |
| Served participant writes | `MEDULLA-DECISION-ENGINE-R617.9` | `src/server/handlers/graph_ops/participant_memory_tests.rs::participant_methods_are_served_and_classified`; `tests/memory/test_participant_memory_served.py::test_participant_put_and_member_round_trip` |
| Memory scope narrows and the ACL still applies | `MEDULLA-DECISION-ENGINE-R651.9` | `crates/medulla-types/src/memory_scope.rs::tests::scope_never_exceeds_visible_sessions`; `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::scope_narrows_recall_and_acl_still_applies` |
| Observation configuration per participant per session | `MEDULLA-DECISION-ENGINE-R651.10` | `crates/medulla-decision-stats/src/decision/participant_derive.rs::tests::targets_follow_flags_and_never_repeat`; `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::observation_flags_decide_which_perspectives_exist` |
| Appending a turn enqueues one derive job | `MEDULLA-DECISION-ENGINE-R651.11` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::turn_append_enqueues_one_ordered_job_per_key` |
| Derive worker writes conclusions as claims | `MEDULLA-DECISION-ENGINE-R651.12` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::derive_job_writes_claims_with_turn_evidence`; `tests/memory/test_participant_memory_served.py::test_turns_become_perspective_claims` |
| Claims carry subject and observer | `MEDULLA-DECISION-ENGINE-R652.9` | `crates/medulla-epistemic/src/claims.rs::tests::observer_sees_own_and_visible_system_claims_only`; `crates/medulla-h-decide/src/decide/stat_tests/claims_tests.rs::perspective_claims_hide_other_observers` |
| Scope narrows cross-session episode recall | `MEDULLA-DECISION-ENGINE-R155.7` | `src/server/handlers/graph_ops/participant_memory_tests.rs::scoped_recall_returns_only_scope_episodes` |
| Memory events enter the graph outbox | `MEDULLA-DECISION-ENGINE-R073.6` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::each_memory_change_emits_one_atomic_event` |
| Scope narrows cross-session episode recall | `MEDULLA-DECISION-ENGINE-R155.7` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::scoped_recall_returns_only_scope_episodes` |
| Memory events enter the graph outbox | `MEDULLA-DECISION-ENGINE-R073.6` | `src/server/handlers/graph_ops/participant_memory_tests.rs::each_memory_change_emits_one_atomic_event` |
| Pull and acknowledge memory events over the served outbox method | `MEDULLA-DECISION-ENGINE-R073.7` | `crates/medulla-h-jobs/src/mutation_outbox.rs::tests::pull_leases_then_ack_advances_cursor`; `tests/memory/test_participant_memory_served.py::test_events_pull_and_ack` |
| Participant questions, cards and derivation status (rollup) | `MEDULLA-DECISION-ENGINE-R655` | `tests/memory/test_participant_memory_served.py::test_ask_card_and_status_journey` |
| Model | `MEDULLA-DECISION-ENGINE-R655.1` | `crates/medulla-types/src/participant_memory.rs::tests::types_round_trip_and_write_vectors` |
| Behavior: ask about a participant | `MEDULLA-DECISION-ENGINE-R655.2` | `crates/medulla-server-served-tests/src/split_tests/medulla_h_graph_participant_ask_tests.rs::ask_cites_claims_and_turns_or_abstains`; `tests/memory/test_participant_memory_served.py::test_ask_about_participant` |
| Behavior: participant card | `MEDULLA-DECISION-ENGINE-R655.3` | `crates/medulla-decision-stats/src/decision/participant_card.rs::tests::card_respects_budget_and_floor`; `crates/medulla-server-served-tests/src/split_tests/medulla_h_graph_participant_card_tests.rs::claim_change_refreshes_card_once` |
| Wiring: card read and derivation status | `MEDULLA-DECISION-ENGINE-R655.4` | `crates/medulla-server-served-tests/src/split_tests/medulla_h_graph_participant_read_tests.rs::participant_read_is_served_and_classified`; `tests/memory/test_participant_memory_served.py::test_card_and_status_round_trip` |
| Refusal | `MEDULLA-DECISION-ENGINE-R655.5` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::participant_refusals_are_non_disclosing_and_change_no_state` |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R655.6` | `tests/memory/test_participant_memory_served.py::test_ask_card_and_status_journey` |
| Assistant profiles and document sets (rollup) | `MEDULLA-DECISION-ENGINE-R656` | `tests/memory/test_assistant_profile_served.py::test_profile_constrains_a_run` |
| Model | `MEDULLA-DECISION-ENGINE-R656.1` | `crates/medulla-types/src/assistant_profile.rs::tests::profile_round_trips_and_rejects_unknown_fields` |
| Behavior: profile and document-set store with sharing | `MEDULLA-DECISION-ENGINE-R656.2` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::profile_save_share_and_conflict`; `tests/memory/test_assistant_profile_served.py::test_profile_and_document_set_round_trip` |
| Behavior: document set as a retrieval filter | `MEDULLA-DECISION-ENGINE-R656.3` | `crates/medulla-server-served-tests/src/split_tests/medulla_h_graph_document_set_tests.rs::document_set_narrows_retrieval_and_acl_still_applies` |
| Wiring: a named profile constrains the run | `MEDULLA-DECISION-ENGINE-R656.4` | `crates/medulla-server-served-tests/src/split_tests/medulla_h_decide_harness_config_tests.rs::profile_ref_without_access_is_refused`; `tests/memory/test_assistant_profile_served.py::test_profile_constrains_a_run` |
| Refusal | `MEDULLA-DECISION-ENGINE-R656.5` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::profile_refusals_change_no_state` |
| Hot-path proof | `MEDULLA-DECISION-ENGINE-R656.6` | `tests/memory/test_assistant_profile_served.py::test_profile_constrains_a_run` |
| Assistant profile constrains the HarnessConfig request | `MEDULLA-DECISION-ENGINE-R128.16` | `crates/medulla-server-served-tests/src/split_tests/medulla_h_decide_harness_config_tests.rs::allowlist_narrows_tool_subset` |
| AgentAssemble respects the profile tool allowlist | `MEDULLA-DECISION-ENGINE-R036.107` | `crates/medulla-h-decide/src/decide/stat_tests/assemble_tests.rs::allowlist_removes_tools_and_admission_still_runs` |
| Participant memory knobs | `MEDULLA-DECISION-ENGINE-R179.8` | `crates/medulla-decision-types/src/decision/knob_families/participant.rs::tests::knob_defaults_are_inside_bounds`; `crates/medulla-h-decide/src/decide/stat_tests/knob_tests.rs::participant_knobs_are_registered_and_tunable` |
| Participant memory refusal codes | `MEDULLA-DECISION-ENGINE-R010.100.4` | `crates/medulla-decision-types/src/decision/record/outcome.rs::tests::codes_are_unique_and_stable` |

## Planned bindings, range 7

Each test below is planned: its path is the post-split location and its name is exact. A row binds it with a `// spec:` comment when it lands.

| Row | Planned test | Level |
|---|---|---|
| `MEDULLA-DECISION-ENGINE-R001` | `crates/medulla-h-decide/src/decide/stat_tests/resolution_tests.rs::every_resolution_kind_is_reachable_through_production_dispatch` | L3 |
| `MEDULLA-DECISION-ENGINE-R001.2` | `crates/medulla-h-decide/src/decide/stat_tests/assemble_resolution_tests.rs::unique_selection_resolves_as_constraint_or_entailment` | L3 |
| `MEDULLA-DECISION-ENGINE-R001.2.1` | `crates/medulla-compute/src/assemble/tests/uniqueness.rs::no_good_resolve_agrees_with_exhaustive_enumeration` | L1/L2 |
| `MEDULLA-DECISION-ENGINE-R001.2.1` | `crates/medulla-h-decide/src/decide/stat_tests/assemble_resolution_tests.rs::unique_selection_resolves_as_constraint_or_entailment` | L3 |
| `MEDULLA-DECISION-ENGINE-R001.2.2` | `crates/medulla-compute/src/assemble/tests/uniqueness.rs::coverage_alone_fixes_the_answer_as_entailment` | L1/L2 |
| `MEDULLA-DECISION-ENGINE-R001.2.2` | `crates/medulla-h-decide/src/decide/stat_tests/assemble_resolution_tests.rs::unique_selection_resolves_as_constraint_or_entailment` | L3 |
| `MEDULLA-DECISION-ENGINE-R010.100.1` | `crates/medulla-decision-types/src/decision/record/outcome.rs::codes_are_unique_and_stable` | L1/L2 |
| `MEDULLA-DECISION-ENGINE-R010.100.2` | `crates/medulla-decision-types/src/decision/record/outcome.rs::every_decision_refusal_enum_maps_to_a_registered_code` | L1/L2 |
| `MEDULLA-DECISION-ENGINE-R010.100.3` | `crates/medulla-result-types/src/retrieval_arbitration.rs::answer_grounding_round_trip` | L1/L2 |
| `MEDULLA-DECISION-ENGINE-R010.100.4` | `crates/medulla-decision-types/src/decision/record/outcome.rs::codes_are_unique_and_stable` | L1/L2 |
| `MEDULLA-DECISION-ENGINE-R014` | `crates/medulla-h-decide/src/decide/stat_tests/assemble_size_tests.rs::smallest_adequate_composition_is_served` | L3 |
| `MEDULLA-DECISION-ENGINE-R029` | `crates/medulla-h-decide/src/decide/stat_tests/retrieval_tests.rs::retrieval_plan_is_selected_and_bound_through_decide` | L3 |
| `MEDULLA-DECISION-ENGINE-R031` | `crates/medulla-h-decide/src/decide/stat_tests/consumer_tests.rs::enrichment_schedule_prefers_higher_net_value` | L3 |
| `MEDULLA-DECISION-ENGINE-R032` | `crates/medulla-h-decide/src/decide/stat_tests/resolve_entity_tests.rs::gold_set_guarantee_holds_or_the_decision_abstains` | L3 |
| `MEDULLA-DECISION-ENGINE-R032.2` | `crates/medulla-h-decide/src/decide/stat_tests/resolve_entity_tests.rs::gold_set_guarantee_holds_or_the_decision_abstains` | L3 |
| `MEDULLA-DECISION-ENGINE-R032.2.1` | `crates/medulla-decision-stats/src/decision/fit_calibrate.rs::isotonic_calibration_is_monotone_and_reproducible` | L1/L2 |
| `MEDULLA-DECISION-ENGINE-R032.2.1` | `crates/medulla-h-decide/src/decide/stat_tests/resolve_entity_tests.rs::gold_set_guarantee_holds_or_the_decision_abstains` | L3 |
| `MEDULLA-DECISION-ENGINE-R032.2.2` | `crates/medulla-h-decide/src/decide/stat_tests/resolve_entity_tests.rs::pairs_act_only_inside_the_conformal_set` | L3 |
| `MEDULLA-DECISION-ENGINE-R032.2.3` | `crates/medulla-h-decide/src/decide/stat_tests/resolve_entity_tests.rs::gold_set_guarantee_holds_or_the_decision_abstains` | L3 |
| `MEDULLA-DECISION-ENGINE-R033` | `crates/medulla-h-decide/src/decide/stat_tests/schema_mapping_tests.rs::columns_map_to_expected_properties_with_evidence_class` | L3 |
| `MEDULLA-DECISION-ENGINE-R033.2` | `crates/medulla-h-decide/src/decide/stat_tests/schema_mapping_tests.rs::columns_map_to_expected_properties_with_evidence_class` | L3 |
| `MEDULLA-DECISION-ENGINE-R033.2.1` | `crates/medulla-decision-stats/src/decision/schema_mapping.rs::score_and_class_follow_the_rule` | L1/L2 |
| `MEDULLA-DECISION-ENGINE-R033.2.1` | `crates/medulla-h-decide/src/decide/stat_tests/schema_mapping_tests.rs::columns_map_to_expected_properties_with_evidence_class` | L3 |
| `MEDULLA-DECISION-ENGINE-R033.2.2` | `crates/medulla-h-decide/src/decide/stat_tests/schema_mapping_tests.rs::columns_map_to_expected_properties_with_evidence_class` | L3 |
| `MEDULLA-DECISION-ENGINE-R039` | `crates/medulla-h-decide/src/decide/stat_tests/route_tests.rs::lower_cost_option_wins_within_the_tie_epsilon` | L3 |
| `MEDULLA-DECISION-ENGINE-R039.2` | `crates/medulla-decision-stats/src/decision/ladder.rs::cost_breaks_ties_within_epsilon_only` | L1/L2 |
| `MEDULLA-DECISION-ENGINE-R039.2` | `crates/medulla-h-decide/src/decide/stat_tests/route_tests.rs::lower_cost_option_wins_within_the_tie_epsilon` | L3 |
| `MEDULLA-DECISION-ENGINE-R044` | `crates/medulla-h-decide/src/decide/stat_tests/a2a_route_tests.rs::inbound_task_routes_through_a_recorded_decision` | L3 |
| `MEDULLA-DECISION-ENGINE-R044.2` | `crates/medulla-h-decide/src/decide/stat_tests/a2a_route_tests.rs::inbound_task_routes_through_a_recorded_decision` | L3 |
| `MEDULLA-DECISION-ENGINE-R044.2.2` | `crates/medulla-h-decide/src/decide/stat_tests/a2a_route_tests.rs::inbound_task_routes_through_a_recorded_decision` | L3 |
| `MEDULLA-DECISION-ENGINE-R050.2` | `tests/test_generated_models.py::test_model_input_capabilities_round_trip_and_refuse_unknown_mime` | L3 |
| `MEDULLA-DECISION-ENGINE-R050.3` | `crates/medulla-h-admin/src/admin/connector_pack/tests/model_input_capabilities.rs::four_media_types_at_128_parts_import` | L1/L2 |
| `MEDULLA-DECISION-ENGINE-R054.1` | `crates/medulla-capabilities/tests/decision_ontology.rs::generated_decision_ontology_is_byte_stable_and_covers_every_contract_class` | L1/L2 |
| `MEDULLA-DECISION-ENGINE-R066.4` | `crates/medulla-h-decide/src/decide/stat_tests/retention_tests.rs::graph_summaries_expire_on_policy_window` | L3 |
| `MEDULLA-DECISION-ENGINE-R072.100` | `crates/medulla-h-decide/src/decide/stat_tests/telemetry_tests.rs::served_decide_emits_one_span_per_stage` | L3 |
| `MEDULLA-DECISION-ENGINE-R073.6` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::each_memory_change_emits_one_atomic_event` | L1/L2 |
| `MEDULLA-DECISION-ENGINE-R073.7` | `crates/medulla-h-jobs/src/mutation_outbox.rs::pull_leases_then_ack_advances_cursor` | L1/L2 |
| `MEDULLA-DECISION-ENGINE-R073.7` | `tests/memory/test_participant_memory_served.py::test_events_pull_and_ack` | L3 |
| `MEDULLA-DECISION-ENGINE-R079` | `tests/web_document_enrich_served.rs::excluded_units_make_no_embedding_call` | L3 |
| `MEDULLA-DECISION-ENGINE-R079.2` | `tests/web_document_enrich_served.rs::excluded_units_make_no_embedding_call` | L3 |
## Planned bindings, range 2

Entries for rows whose named test is not yet in the tree.

- **MEDULLA-DECISION-ENGINE-R079.6:** `crates/medulla-h-admin/src/admin/connector_pack/tests/embedding_set.rs::prepare_builds_the_embedding_set_from_admitted_columns` (L1; calls `prepare` directly). Proves the row at its entry point; no receipt is asserted.
- `MEDULLA-DECISION-ENGINE-R079.4`: L3 `crates/medulla-h-admin/src/admin/connector_pack/tests/sql_columns.rs::sql_column_kinds_and_cardinality_control_embedding` drives `SourceIngest` with one column per kind and asserts each exclusion reason and the 100-sample free-text floor.
- `MEDULLA-DECISION-ENGINE-R081`: planned L1 `crates/medulla-graph-algos/src/algorithms/community.rs::tests::resolver_confidence_weights_bind_a_community` compares assignments across two confidence levels.
- `MEDULLA-DECISION-ENGINE-R085.1`: planned L1 `crates/medulla-decision-stats/src/decision/legal_option_filter.rs::tests::an_option_failing_the_predicate_is_excluded` plus the admissible pass-through case.
- `MEDULLA-DECISION-ENGINE-R085.2`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/consumer_tests.rs::a_caller_asserted_option_violating_a_shape_never_reaches_the_scorer`.
- `MEDULLA-DECISION-ENGINE-R088.1`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/belief_tests.rs::graph_belief_slices_match_direct_belief_as_of_queries` compares each slice to a direct AS OF query.
- `MEDULLA-DECISION-ENGINE-R090.1`: planned L1 `crates/medulla-decision-types/src/decision/statistical/scorer.rs::tests::a_budget_over_the_ceiling_is_refused` with the at-ceiling accept case.
- `MEDULLA-DECISION-ENGINE-R095`: planned L2 `crates/medulla-decision-policy/tests/solve/determinism.rs::the_certificate_digest_matches_the_committed_golden_vector`; the repeated-solves byte-identical test sits beside it.
- `MEDULLA-DECISION-ENGINE-R096`: planned L3 `crates/medulla-h-mining/src/mining/insight.rs::tests::a_weighted_request_with_a_non_default_quality_function_reaches_the_kernel`.
- `MEDULLA-DECISION-ENGINE-R097`: planned L1 `crates/medulla-server-api/src/request_replay.rs::tests::a_reused_nonce_is_rejected` and the blob-GC grace-period test beside the collector.
- `MEDULLA-DECISION-ENGINE-R099.1`: planned L3 `crates/medulla-h-admin/src/graph_schema/world_model_tests.rs::wikidata_schema_mappings_validate_against_world_model_shapes` validates the fixture and resolves each entity by key.
- `MEDULLA-DECISION-ENGINE-R099.2`: planned L3 `crates/medulla-h-ingest/src/web_ingest/tests.rs::entity_document_lowers_to_alignment_claims`.
- `MEDULLA-DECISION-ENGINE-R117.1.1`: L2 `tests/derive_golden.rs::uql_sql_promql_outputs_agree_on_golden_reference` asserts all fifteen functions agree within 1e-9 on the three surfaces, with the PromQL surface compiling to the shared `Kernel` series program and holding no evaluator of its own.
- `MEDULLA-DECISION-ENGINE-R117.1.2`: planned L1 `crates/medulla-numeric/src/evaluation/skill.rs::tests::bootstrap_interval_is_reproducible` and planned L3 `crates/medulla-h-decide/src/decide/stat_tests/evaluator_tests.rs::decision_eval_reports_feature_power`.
- `MEDULLA-DECISION-ENGINE-R118.1.2.1`: planned L1 `crates/medulla-decision-types/src/decision/reputation.rs::tests::source_reliability_round_trips`.
- `MEDULLA-DECISION-ENGINE-R118.1.2.2`: L1 `crates/medulla-exec/src/exec/reliability.rs::tests::insufficient_history_abstains_instead_of_using_the_prior`.
- `MEDULLA-DECISION-ENGINE-R118.1.2.3`: L1 `crates/medulla-h-query/src/query/explain_wire_b.rs::tests::caller_reliability_cannot_replace_the_governed_estimate`.
- `MEDULLA-DECISION-ENGINE-R118.1.2.4`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/reputation_tests.rs::three_readers_agree_on_sparse_abstention_and_dense_estimate`.
- `MEDULLA-DECISION-ENGINE-R119`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/replay_tests.rs::walk_forward_seals_trial_count_pbo_and_dm_figures` compares the sealed trial count, PBO and Diebold-Mariano figures to independently recomputed values and asserts the policy-dependent refusal.
- `MEDULLA-DECISION-ENGINE-R120`: planned L1 `tests/test_decision_static_contracts.py::test_only_persisted_writes_count_as_admission_anchors` runs a read helper, a replay helper and a persisted write path through the script.
- `MEDULLA-DECISION-ENGINE-R121.2`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/classes_tests.rs::graph_claim_and_evidence_control_answer_class`.
- `MEDULLA-DECISION-ENGINE-R122`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/learning_tests.rs::synthetic_origin_records_are_excluded_from_real_calibration`.
- `MEDULLA-DECISION-ENGINE-R124`: planned L1 `crates/medulla-numeric/tests/stat_kernels/clippy_policy.rs::the_declared_self_check_file_exists`.
- `MEDULLA-DECISION-ENGINE-R128.14`: planned L1 `tests/test_generated_models.py::test_harness_config_kind_is_a_contract_question_kind` asserts the `QuestionKind` literal includes `harness_config`.
- `MEDULLA-DECISION-ENGINE-R129.1`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/rule_promotion_tests.rs::attach_point_reads_published_invariants`.
- `MEDULLA-DECISION-ENGINE-R129.5`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/rule_promotion_tests.rs::contradicting_rule_is_rejected_and_compatible_rule_promotes`.
- `MEDULLA-DECISION-ENGINE-R102.1`: planned L1 `crates/medulla-catalog-types/src/graph_schema/shadow_branch.rs::tests::activation_requires_every_step_in_order` with the omitted-shadow-ingest and reordered-approval refusals.
- `MEDULLA-DECISION-ENGINE-R103`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/scorer_tests.rs::each_forecast_horizon_reports_calibrated_confidence_or_abstains`.
- `MEDULLA-DECISION-ENGINE-R104`: planned L1 `crates/medulla-nlq/src/decide_text/tests.rs::decide_text_and_uql_share_structured_error_shape` runs the same table-driven fixtures against both parsers.
- `MEDULLA-DECISION-ENGINE-R107`: planned L2 `crates/medulla-agent-types/src/agent_topology.rs::tests::topology_types_stay_within_twelve_and_goldens_match` and a Python round-trip in tests/test_generated_models.py.
- `MEDULLA-DECISION-ENGINE-R108`: planned L1 `crates/medulla-server-ops/src/api/admin/topology_shape.rs::tests::an_invalid_topology_is_rejected_at_publish`.
- `MEDULLA-DECISION-ENGINE-R111.1`: L1 `crates/medulla-decision-types/src/decision/statistical/topology.rs::tests::question_round_trips_and_omits_unread_facts`; L3 `crates/medulla-h-decide/src/decide/stat_tests/consumer_tests.rs::a_standalone_topology_decide_question_returns_a_typed_decision_record`.
- `MEDULLA-DECISION-ENGINE-R112`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/consumer_tests.rs::exhausted_capacity_at_a_priority_is_a_lease_named_infeasible_result`.
- `MEDULLA-DECISION-ENGINE-R114`: planned L5 `crates/medulla-h-decide/src/decide/stat_tests/consumer_tests.rs::subagent_allowance_reads_the_topology_facts_of_a_committed_record`.
## Planned bindings, range 4

Entries for rows whose named test is not yet in the tree.

- `MEDULLA-DECISION-ENGINE-R134.102`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/delegation_tests.rs::window_matrix_admits_or_refuses`.
- `MEDULLA-DECISION-ENGINE-R134.104`: planned L1 `crates/medulla-decision-outcome-types/src/decision/answer_ranking/tests.rs::ranking_record_round_trips_and_rejects_a_missing_tie_rule`.
- `MEDULLA-DECISION-ENGINE-R134.105`: planned L1 `crates/medulla-decision-outcome-types/src/decision/answer_ranking/tests.rs::four_candidates_yield_order_retention_and_revisions`.
- `MEDULLA-DECISION-ENGINE-R134.106`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/answer_ranking_tests.rs::served_ranking_round_trips_through_readback`.
- `MEDULLA-DECISION-ENGINE-R134.107`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/answer_ranking_tests.rs::each_refusal_returns_its_typed_reason_and_records_no_ranking`.
- `MEDULLA-DECISION-ENGINE-R134.108`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/answer_ranking_tests.rs::premises_name_pairs_and_order`.
- `MEDULLA-DECISION-ENGINE-R134.109`: planned L4 `tests/decide/test_answer_ranking_served.py::test_revisions_only_for_non_retained`.
- `MEDULLA-DECISION-ENGINE-R134.109`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/answer_ranking_tests.rs::served_ranking_round_trips_through_readback`.
- `MEDULLA-DECISION-ENGINE-R031.100`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/paper_admission_tests.rs::only_relevant_papers_are_admitted_with_claims`.
- `MEDULLA-DECISION-ENGINE-R036.100.1`: planned L1 `crates/medulla-decision-types/src/decision/primitive.rs::tests::primitive_contract_round_trips_and_kind_map_is_exhaustive`.
- `MEDULLA-DECISION-ENGINE-R036.100.2`: planned L2 `crates/medulla-compute/src/assemble/tests/primitive.rs::substitution_rules_cover_every_refusal`.
- `MEDULLA-DECISION-ENGINE-R036.100.3`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/assemble_tests.rs::primitive_refusals_abstain_with_stable_codes`.
- `MEDULLA-DECISION-ENGINE-R036.100.4`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/assemble_tests.rs::primitive_premises_and_inactive_proposals`.
- `MEDULLA-DECISION-ENGINE-R136.101`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/evolution_round_tests.rs::criterion_regression_holds_the_revision`.
- `MEDULLA-DECISION-ENGINE-R121.100`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/step_verification_tests.rs::failed_and_uncertain_steps_withhold_their_dependents`.
- `MEDULLA-DECISION-ENGINE-R138.100.1`: planned L1 `crates/medulla-decision-stats/src/decision/verifier_receipt.rs::tests::each_breach_is_reported`.
- `MEDULLA-DECISION-ENGINE-R138.100.1`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/generated_code_admission_tests.rs::admission_requires_a_complete_receipt`.
- `MEDULLA-DECISION-ENGINE-R138.100.2`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/generated_code_admission_tests.rs::admission_requires_a_complete_receipt`.
- `MEDULLA-DECISION-ENGINE-R136.102`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/evolution_round_tests.rs::incomplete_grading_refuses_before_review`.
- `MEDULLA-DECISION-ENGINE-R136.103`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/evolution_round_tests.rs::rising_judge_score_without_outcome_gain_refuses`.
- `MEDULLA-DECISION-ENGINE-R138.101.1`: planned L1 `crates/medulla-decision-stats/src/decision/changeset_review.rs::tests::only_evidenced_low_impact_findings_filter`.
- `MEDULLA-DECISION-ENGINE-R138.101.1`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/changeset_review_tests.rs::review_covers_five_classes_and_filters_by_impact`.
- `MEDULLA-DECISION-ENGINE-R138.101.2`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/changeset_review_tests.rs::review_covers_five_classes_and_filters_by_impact`.
- `MEDULLA-DECISION-ENGINE-R036.101`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/assemble_tests.rs::coding_agent_plan_orders_skills_and_refuses_stale_receipts`.
- `MEDULLA-DECISION-ENGINE-R014.100.1`: planned L1 `crates/medulla-compute/src/assemble/tests/smallest_adequate.rs::inadequate_and_unknown_adequacy_are_eliminated`.
- `MEDULLA-DECISION-ENGINE-R014.100.1`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/assemble_size_tests.rs::smallest_adequate_composition_is_served`.
- `MEDULLA-DECISION-ENGINE-R014.100.2`: planned L2 `crates/medulla-compute/src/assemble/tests/smallest_adequate.rs::lexicographic_size_then_cost_matches_brute_force`.
- `MEDULLA-DECISION-ENGINE-R014.100.2`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/assemble_size_tests.rs::smallest_adequate_composition_is_served`.
- `MEDULLA-DECISION-ENGINE-R147.1`: planned L1 `crates/medulla-decision-types/src/decision/statistical/distilled.rs::tests::distilled_scores_round_trip`.
- `MEDULLA-DECISION-ENGINE-R147.1`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/distilled_rank_tests.rs::stored_scores_rank_without_model_calls`.
- `MEDULLA-DECISION-ENGINE-R147.2`: planned L1 `crates/medulla-decision-stats/src/decision/distilled.rs::tests::no_query_time_calls`.
- `MEDULLA-DECISION-ENGINE-R147.2`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/distilled_rank_tests.rs::stored_scores_rank_without_model_calls`.
- `MEDULLA-DECISION-ENGINE-R147.3`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/distilled_rank_tests.rs::stored_scores_rank_without_model_calls`.
- `MEDULLA-DECISION-ENGINE-R147.4`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/distilled_rank_tests.rs::missing_score_refuses`.
- `MEDULLA-DECISION-ENGINE-R147.5`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/distilled_rank_tests.rs::distilled_evidence_binds_scorer`.
- `MEDULLA-DECISION-ENGINE-R147.6`: planned L4 `tests/decide/test_distilled_ranking_served.py::test_index_time_scoring_and_query_ranking`.
- `MEDULLA-DECISION-ENGINE-R148.1`: planned L1 `crates/medulla-decision-types/src/decision/command_config.rs::tests::config_round_trip`.
- `MEDULLA-DECISION-ENGINE-R148.1`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/command_config_tests.rs::command_config_journey`.
- `MEDULLA-DECISION-ENGINE-R148.2`: planned L1 `crates/medulla-decision-types/src/decision/command_config.rs::tests::replay_idempotence`.
- `MEDULLA-DECISION-ENGINE-R148.2`: planned L1 `crates/medulla-decision-types/src/decision/command_config.rs::tests::default_ordinal_selects_a_mode`.
- `MEDULLA-DECISION-ENGINE-R148.3`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/command_config_tests.rs::decide_routes_mode`.
- `MEDULLA-DECISION-ENGINE-R148.3`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/command_config_tests.rs::other_tenant_modes_are_not_read`.
- `MEDULLA-DECISION-ENGINE-R148.4`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/command_config_tests.rs::invalid_config_refuses`.
- `MEDULLA-DECISION-ENGINE-R148.5`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/command_config_tests.rs::config_evidence_binds_schema`.
- `MEDULLA-DECISION-ENGINE-R148.6`: planned L4 `tests/decide/test_command_config_served.py::test_mode_decision_and_replay`.
- `MEDULLA-DECISION-ENGINE-R149.1`: planned L1 `crates/medulla-work-types/src/memory.rs::tests::search_state_round_trips`.
- `MEDULLA-DECISION-ENGINE-R149.1`: planned L3 `tests/memory_sync_served.rs::search_state_survives_context_eviction`.
- `MEDULLA-DECISION-ENGINE-R149.2`: planned L1 `crates/medulla-work-types/src/memory.rs::tests::eviction_readback_validates_revision_and_arrays`.
- `MEDULLA-DECISION-ENGINE-R149.2`: planned L3 `tests/memory_sync_served.rs::search_state_survives_context_eviction`.
- `MEDULLA-DECISION-ENGINE-R149.3`: planned L3 `tests/memory_sync_served.rs::search_state_wire_reaches_the_gateway`.
- `MEDULLA-DECISION-ENGINE-R149.4`: planned L3 `tests/memory_sync_served.rs::incomplete_state_refuses`.
- `MEDULLA-DECISION-ENGINE-R149.5`: planned L3 `tests/memory_sync_served.rs::state_evidence_lists_four_digests`.
- `MEDULLA-DECISION-ENGINE-R149.6`: planned L3 `tests/memory_sync_served.rs::search_state_survives_context_eviction`.
- `MEDULLA-DECISION-ENGINE-R150.1`: planned L1 `crates/medulla-work-types/src/memory.rs::tests::notes_round_trip`.
- `MEDULLA-DECISION-ENGINE-R150.1`: planned L3 `tests/memory_sync_served.rs::session_notes_recalled_with_flags`.
- `MEDULLA-DECISION-ENGINE-R150.2`: planned L1 `crates/medulla-decision-stats/src/decision/session_note.rs::tests::capture_and_apply`.
- `MEDULLA-DECISION-ENGINE-R150.2`: planned L3 `tests/memory_sync_served.rs::session_notes_recalled_with_flags`.
- `MEDULLA-DECISION-ENGINE-R150.3`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/session_note_tests.rs::capture_requires_a_successful_run`.
- `MEDULLA-DECISION-ENGINE-R150.4`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/session_note_tests.rs::unauthorized_refuses`.
- `MEDULLA-DECISION-ENGINE-R150.5`: planned L3 `tests/memory_sync_served.rs::session_note_evidence_lists_flags`.
- `MEDULLA-DECISION-ENGINE-R150.6`: planned L4 `tests/decide/test_session_notes_served.py::test_capture_and_reuse`.
- `MEDULLA-DECISION-ENGINE-R151.1`: planned L1 `crates/medulla-decision-types/src/decision/importance_penalty.rs::tests::importance_round_trip`.
- `MEDULLA-DECISION-ENGINE-R151.1`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/importance_penalty_tests.rs::fit_with_importance_keeps_important_weights`.
- `MEDULLA-DECISION-ENGINE-R151.2`: planned L1 `crates/medulla-decision-stats/src/decision/importance_penalty.rs::tests::penalty_respects_importance`.
- `MEDULLA-DECISION-ENGINE-R151.2`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/importance_penalty_tests.rs::fit_with_importance_keeps_important_weights`.
- `MEDULLA-DECISION-ENGINE-R151.3`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/importance_penalty_tests.rs::fit_applies_the_penalty_from_the_request`.
- `MEDULLA-DECISION-ENGINE-R151.4`: L3 `crates/medulla-h-decide/src/decide/stat_tests/importance_penalty_tests.rs::invalid_importance_refuses`.
- `MEDULLA-DECISION-ENGINE-R151.5`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/importance_penalty_tests.rs::penalty_evidence`.
- `MEDULLA-DECISION-ENGINE-R151.6`: L3 `crates/medulla-h-decide/src/decide/stat_tests/importance_penalty_tests.rs::fit_with_importance_keeps_important_weights`.
- `MEDULLA-DECISION-ENGINE-R139.100`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/comparative_analysis_tests.rs::graph_refs_resolve_and_refuse`.
- `MEDULLA-DECISION-ENGINE-R019.100.1`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/commit_tests.rs::evidence_streams_are_sealed_and_read_back`.
- `MEDULLA-DECISION-ENGINE-R019.100.2`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/commit_tests.rs::missing_stale_and_mismatched_approval_refuse_without_write`.
- `MEDULLA-DECISION-ENGINE-R029.100`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/retrieval_tests.rs::inference_mode_and_config_digests_are_bound`.
- `MEDULLA-DECISION-ENGINE-R140.100`: planned L2 `crates/medulla-decision-types/src/decision/task_plan/trajectory_budget.rs::tests::score_and_branch_golden_vectors`.
- `MEDULLA-DECISION-ENGINE-R140.100`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/trajectory_tests.rs::trajectory_decisions_journey`.
- `MEDULLA-DECISION-ENGINE-R140.101`: planned L2 `crates/medulla-decision-types/src/decision/task_plan/trajectory_projection.rs::tests::order_and_candidate_id_are_deterministic`.
- `MEDULLA-DECISION-ENGINE-R140.101`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/trajectory_tests.rs::trajectory_decisions_journey`.
- `MEDULLA-DECISION-ENGINE-R140.102`: planned L1 `crates/medulla-decision-types/src/decision/task_plan/branch_breadth.rs::tests::mapping_and_clamp`.
- `MEDULLA-DECISION-ENGINE-R140.102`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/trajectory_tests.rs::trajectory_decisions_journey`.
- `MEDULLA-DECISION-ENGINE-R140.103`: planned L2 `crates/medulla-decision-types/src/decision/task_plan/pairwise.rs::tests::copeland_cycle_and_tie_fixture`.
- `MEDULLA-DECISION-ENGINE-R140.103`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/trajectory_tests.rs::trajectory_decisions_journey`.
- `MEDULLA-DECISION-ENGINE-R140.104`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/trajectory_tests.rs::decide_returns_pinned_trajectory_decisions`.
- `MEDULLA-DECISION-ENGINE-R140.105`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/trajectory_tests.rs::trajectory_decisions_journey`.
- `MEDULLA-DECISION-ENGINE-R141.100`: planned L1 `crates/medulla-decision-stats/src/decision/memory_rewrite.rs::tests::fidelity_is_set_overlap_rounded_half_to_even`.
- `MEDULLA-DECISION-ENGINE-R141.100`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/memory_change_tests.rs::rewrite_admit_and_discard_through_decision_eval`.
- `MEDULLA-DECISION-ENGINE-R141.101`: planned L1 `crates/medulla-decision-stats/src/decision/memory_benchmark.rs::tests::recall_and_accuracy_use_exact_counts`.
- `MEDULLA-DECISION-ENGINE-R141.101`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/memory_change_tests.rs::benchmark_receipt_binds_both_snapshots`.
- `MEDULLA-DECISION-ENGINE-R141.102`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/memory_change_tests.rs::rewrite_admit_and_discard_through_decision_eval`.
- `MEDULLA-DECISION-ENGINE-R141.103`: planned L4 `tests/decide/test_memory_change_served.py::test_consolidate_requires_current_admit_receipt`.
- `MEDULLA-DECISION-ENGINE-R141.104`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/memory_change_tests.rs::consolidate_requires_a_current_admit_receipt`.
- `MEDULLA-DECISION-ENGINE-R142.100`: planned L1 `crates/medulla-decision-types/src/decision/local_certification.rs::tests::attestation_round_trip_and_digest`.
- `MEDULLA-DECISION-ENGINE-R142.100`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/certification_tests.rs::certification_journey`.
- `MEDULLA-DECISION-ENGINE-R142.101`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/certification_tests.rs::attests_atomic_and_gates_workflow`.
- `MEDULLA-DECISION-ENGINE-R142.102`: planned L4 `tests/decide/test_local_skill_certification_served.py::test_certification_journey`.
- `MEDULLA-DECISION-ENGINE-R143.1`: planned L1 `crates/medulla-decision-types/src/decision/instruction_disclosure.rs::tests::disclosure_events_round_trip`.
- `MEDULLA-DECISION-ENGINE-R143.1`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/instruction_disclosure_tests.rs::disclosure_journey`.
- `MEDULLA-DECISION-ENGINE-R143.2`: planned L2 `crates/medulla-decision-stats/src/decision/instruction_disclosure.rs::tests::triggers_produce_distinct_versions`.
- `MEDULLA-DECISION-ENGINE-R143.2`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/instruction_disclosure_tests.rs::disclosure_journey`.
- `MEDULLA-DECISION-ENGINE-R143.3`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/instruction_disclosure_tests.rs::decide_routes_disclosure_events`.
- `MEDULLA-DECISION-ENGINE-R143.4`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/instruction_disclosure_tests.rs::invalid_trigger_or_hash_refuses`.
- `MEDULLA-DECISION-ENGINE-R143.5`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/instruction_disclosure_tests.rs::disclosure_binds_version_and_hash`.
- `MEDULLA-DECISION-ENGINE-R143.6`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/instruction_disclosure_tests.rs::disclosure_journey`.
- `MEDULLA-DECISION-ENGINE-R144.1`: planned L1 `crates/medulla-decision-types/src/decision/indexless_retrieval.rs::tests::operations_and_budget_round_trip`.
- `MEDULLA-DECISION-ENGINE-R144.1`: planned L3 `tests/served_indexless_retrieval.rs::indexless_retrieval_reports_cost_and_refuses_over_budget`.
- `MEDULLA-DECISION-ENGINE-R144.2`: planned L1 `crates/medulla-decision-types/src/decision/indexless_retrieval.rs::tests::budget_enforcement`.
- `MEDULLA-DECISION-ENGINE-R144.2`: planned L3 `tests/served_indexless_retrieval.rs::indexless_retrieval_reports_cost_and_refuses_over_budget`.
- `MEDULLA-DECISION-ENGINE-R144.3`: planned L3 `tests/served_indexless_retrieval.rs::indexless_routes_cost_report`.
- `MEDULLA-DECISION-ENGINE-R144.4`: planned L3 `tests/served_indexless_retrieval.rs::budget_exhaustion_refuses`.
- `MEDULLA-DECISION-ENGINE-R144.5`: planned L3 `tests/served_indexless_retrieval.rs::cost_reports_bind_budget`.
- `MEDULLA-DECISION-ENGINE-R144.6`: planned L4 `tests/decide/test_indexless_retrieval_served.py::test_budget_enforcement_and_cost_reporting`.
- `MEDULLA-DECISION-ENGINE-R144.7`: planned L3 `tests/served_indexless_retrieval.rs::consumer_budget_clamped_to_tenant_ceiling`.
- `MEDULLA-DECISION-ENGINE-R144.7`: planned L3 `tests/decide/test_indexless_retrieval_served.py::test_consumer_budget_clamped_to_tenant_ceiling`.
- `MEDULLA-DECISION-ENGINE-R146.1`: planned L1 `crates/medulla-decision-types/src/decision/temporal_query.rs::tests::temporal_context_round_trip`.
- `MEDULLA-DECISION-ENGINE-R146.1`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/temporal_basis_tests.rs::three_clock_confusion_selects_a_basis_and_builds_a_fallback`.
- `MEDULLA-DECISION-ENGINE-R146.2`: planned L1 `crates/medulla-decision-stats/src/decision/temporal_basis.rs::tests::confusion_detection`.
- `MEDULLA-DECISION-ENGINE-R146.2`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/temporal_basis_tests.rs::three_clock_confusion_selects_a_basis_and_builds_a_fallback`.
- `MEDULLA-DECISION-ENGINE-R146.3`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/temporal_basis_tests.rs::three_clock_confusion_selects_a_basis_and_builds_a_fallback`.
- `MEDULLA-DECISION-ENGINE-R146.4`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/temporal_basis_tests.rs::missing_time_refuses`.
- `MEDULLA-DECISION-ENGINE-R146.5`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/temporal_basis_tests.rs::temporal_confusion_evidence`.
- `MEDULLA-DECISION-ENGINE-R146.6`: planned L4 `tests/decide/test_temporal_basis_served.py::test_three_clock_confusion_and_fallback`.
- `MEDULLA-DECISION-ENGINE-R100.100.1`: planned L1 `crates/medulla-decision-stats/src/decision/ranking_eval.rs::tests::hits_at_k_counts_queries_with_a_relevant_top_k_item`.
- `MEDULLA-DECISION-ENGINE-R100.100.1`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/retrieval_eval_tests.rs::ranking_evaluation_is_sealed_and_readable`.
- `MEDULLA-DECISION-ENGINE-R100.100.2`: planned L3 `crates/medulla-h-decide/src/decide/stat_tests/retrieval_eval_tests.rs::ranking_evaluation_is_sealed_and_readable`.
- `MEDULLA-DECISION-ENGINE-R100.100.3`: planned L1 `crates/medulla-decision-stats/src/decision/context_search.rs::tests::offsets_slice_the_original_bytes_for_composed_and_decomposed_text`.
- `MEDULLA-DECISION-ENGINE-R100.100.3`: planned L3 `tests/served_context_search.rs::context_search_is_authorized_and_bounded`.
- `MEDULLA-DECISION-ENGINE-R100.100.4`: planned L3 `tests/served_context_search.rs::context_search_is_authorized_and_bounded`.
- `MEDULLA-DECISION-ENGINE-R100.100.4`: planned L3 `tests/decide/test_context_search_served.py::test_context_search_round_trip`.
- `MEDULLA-DECISION-ENGINE-R152.1`: planned L1 `crates/medulla-types/src/context_facts.rs::tests::context_fact_round_trip`.
- `MEDULLA-DECISION-ENGINE-R152.1`: planned L3 `crates/medulla-h-graph/src/graph_ops/context_facts_tests.rs::served_context_facts_returns_cited_facts_within_budget`.
- `MEDULLA-DECISION-ENGINE-R152.2`: planned L2 `crates/medulla-types/src/context_facts.rs::tests::selection_is_deterministic_and_within_budget`.
- `MEDULLA-DECISION-ENGINE-R152.2`: planned L3 `crates/medulla-h-graph/src/graph_ops/context_facts_tests.rs::served_context_facts_returns_cited_facts_within_budget`.
- `MEDULLA-DECISION-ENGINE-R152.3`: planned L3 `crates/medulla-h-graph/src/graph_ops/context_facts_tests.rs::context_facts_respects_visibility`.
- `MEDULLA-DECISION-ENGINE-R152.4`: planned L3 `crates/medulla-h-graph/src/graph_ops/context_facts_tests.rs::context_facts_refusals`.
- `MEDULLA-DECISION-ENGINE-R152.5`: planned L3 `crates/medulla-h-graph/src/graph_ops/context_facts_tests.rs::every_fact_carries_provenance`.
- `MEDULLA-DECISION-ENGINE-R152.6`: planned L3 `crates/medulla-h-graph/src/graph_ops/context_facts_tests.rs::served_context_facts_returns_cited_facts_within_budget`.
- `MEDULLA-DECISION-ENGINE-R152.6`: planned L4 `tests/decide/test_context_facts_served.py::test_context_facts_after_discover`.
- `MEDULLA-DECISION-ENGINE-R153.1`: planned L1 `crates/medulla-types/src/source_toc.rs::tests::toc_round_trip`.
- `MEDULLA-DECISION-ENGINE-R153.1`: planned L3 `crates/medulla-h-graph/src/graph_ops/read_span_tests.rs::served_ingest_then_bounded_span_read`.
- `MEDULLA-DECISION-ENGINE-R153.2`: planned L1 `crates/medulla-document/src/toc.rs::tests::toc_offsets_slice_the_original_bytes`.
- `MEDULLA-DECISION-ENGINE-R153.2`: planned L3 `crates/medulla-h-graph/src/graph_ops/read_span_tests.rs::served_ingest_then_bounded_span_read`.
- `MEDULLA-DECISION-ENGINE-R153.3`: planned L3 `crates/medulla-h-graph/src/graph_ops/read_span_tests.rs::ingest_stores_a_toc_and_read_span_returns_its_digest`.
- `MEDULLA-DECISION-ENGINE-R153.4`: planned L3 `crates/medulla-h-graph/src/graph_ops/read_span_tests.rs::read_span_refusals`.
- `MEDULLA-DECISION-ENGINE-R153.5`: planned L3 `crates/medulla-h-graph/src/graph_ops/read_span_tests.rs::span_response_binds_toc_and_source_digests`.
- `MEDULLA-DECISION-ENGINE-R153.6`: planned L3 `crates/medulla-h-graph/src/graph_ops/read_span_tests.rs::served_ingest_then_bounded_span_read`.
- `MEDULLA-DECISION-ENGINE-R153.6`: planned L4 `tests/retrieval/test_offline_toc_served.py::test_read_span_after_ingest`.
- `MEDULLA-DECISION-ENGINE-R154.1`: planned L1 `crates/medulla-work-types/src/memory.rs::tests::memory_knobs_cover_every_policy_field`.
- `MEDULLA-DECISION-ENGINE-R154.1`: planned L3 `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::recall_stays_stable_as_the_corpus_doubles`.
- `MEDULLA-DECISION-ENGINE-R154.2`: planned L3 `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::recall_stays_stable_as_the_corpus_doubles`.
- `MEDULLA-DECISION-ENGINE-R154.2`: planned L1 `tests/test_check_decide_budgets.py::test_memory_scaling_growth_above_2_2_fails`.
- `MEDULLA-DECISION-ENGINE-R154.4`: planned L3 `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::remember_refuses_over_tenant_capacity`.
## Planned bindings, range 5

Rows `MEDULLA-DECISION-ENGINE-R154.5` to `MEDULLA-DECISION-ENGINE-R172.12`. Every entry below is planned: it names no existing test, only the exact post-split path where the test is added with its rows. The tests already in the tree for this range are `MEDULLA-DECISION-ENGINE-R164.1`, `MEDULLA-DECISION-ENGINE-R166.1` and `MEDULLA-DECISION-ENGINE-R167.1`, which cite them in the requirement row. A root slice test appears once and lists every row it verifies.

| Planned test | Level | Requirements | Setup and assertion |
|---|---|---|---|
| `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::recall_stays_stable_as_the_corpus_doubles` | L3 | `MEDULLA-DECISION-ENGINE-R154.6` | asserts result-set equality across sizes |
| `tests/memory/test_parameter_free_scaling_served.py::test_recall_stable_as_corpus_grows` | L4 | `MEDULLA-DECISION-ENGINE-R154.6` | runs it through the client |
| `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::later_session_recalls_earlier_episode_in_order` | L3 | `MEDULLA-DECISION-ENGINE-R155`, `MEDULLA-DECISION-ENGINE-R155.1`, `MEDULLA-DECISION-ENGINE-R155.2`, `MEDULLA-DECISION-ENGINE-R155.6` | asserts episode immutability, time order and a hidden future episode |
| `crates/medulla-work-types/src/memory.rs::tests::episode_round_trip_and_digest` | L1 | `MEDULLA-DECISION-ENGINE-R155.1` | asserts a serde round trip, a stable digest and rejection of `end_ms` below `start_ms` |
| `crates/medulla-decision-memory/src/memory/episodes.rs::tests::episodes_sort_by_time_and_hide_future` | L1 | `MEDULLA-DECISION-ENGINE-R155.2` | `tests::overlapping_episodes_are_invalid` (L1) |
| `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::remember_seals_an_episode_and_recall_filters_by_time` | L3 | `MEDULLA-DECISION-ENGINE-R155.3` | runs under tenant and principal context with a cross-tenant negative case |
| `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::episode_refusals` | L3 | `MEDULLA-DECISION-ENGINE-R155.4` | submits each bad write and asserts the code and an unchanged store |
| `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::recall_result_binds_episode_digests` | L3 | `MEDULLA-DECISION-ENGINE-R155.5` | recomputes each digest from the stored records and compares every field |
| `tests/memory/test_cross_session_retrieval_served.py::test_later_session_recalls_earlier_episode` | L4 | `MEDULLA-DECISION-ENGINE-R155.6` | runs it through the client |
| `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::scoped_recall_returns_only_scope_episodes` | L3 | `MEDULLA-DECISION-ENGINE-R155.7` | seals three sessions, scopes to two and asserts the result |
| `crates/medulla-h-decide/src/decide/stat_tests/skill_workflow_tests.rs::served_compile_then_lookup_by_skill_digest` | L3 | `MEDULLA-DECISION-ENGINE-R156`, `MEDULLA-DECISION-ENGINE-R156.1`, `MEDULLA-DECISION-ENGINE-R156.2`, `MEDULLA-DECISION-ENGINE-R156.3` | The test compiles a golden draft, commits it, looks it up by digest and asserts the steps, `ResolutionKind::Constraint` and `EvidenceClass::Claim` |
| `crates/medulla-decision-types/src/decision/skill_workflow.rs::tests::draft_round_trips` | L1 | `MEDULLA-DECISION-ENGINE-R156.1` | round-trips a draft and rejects 41 steps |
| `crates/medulla-decision-plan/src/plan/skill_workflow.rs::tests::golden_draft_compiles_in_order` | L1 | `MEDULLA-DECISION-ENGINE-R156.2` | asserts step order and the error branch |
| `crates/medulla-h-decide/src/decide/stat_tests/skill_workflow_tests.rs::other_tenant_cannot_look_up_the_program` | L3 | `MEDULLA-DECISION-ENGINE-R156.3` | is the cross-tenant negative case |
| `crates/medulla-h-decide/src/decide/stat_tests/skill_workflow_tests.rs::incomplete_drafts_refuse` | L3 | `MEDULLA-DECISION-ENGINE-R156.4` | submits one draft per code |
| `crates/medulla-h-decide/src/decide/stat_tests/skill_workflow_tests.rs::golden_skill_drafts_have_fixed_program_digests` | L3 | `MEDULLA-DECISION-ENGINE-R156.5` | compiles each golden draft and asserts step order, error branching, types and the program digest |
| `tests/skills/test_skill_to_workflow_compilation_served.py::test_draft_compiles_commits_and_looks_up` | L4 | `MEDULLA-DECISION-ENGINE-R156.6` | asserts the stored program and the skill digest |
| `crates/medulla-h-decide/src/decide/stat_tests/delegation_quality_tests.rs::quality_gate_routes` | L3 | `MEDULLA-DECISION-ENGINE-R157`, `MEDULLA-DECISION-ENGINE-R157.1`, `MEDULLA-DECISION-ENGINE-R157.2`, `MEDULLA-DECISION-ENGINE-R157.3` | passes |
| `crates/medulla-decision-types/src/decision/delegation_quality.rs::tests::types_round_trip` | L1 | `MEDULLA-DECISION-ENGINE-R157.1` | Proves: Delegation quality model |
| `crates/medulla-decision-types/src/decision/delegation_quality.rs::tests::quality_gate_admission` | L1 | `MEDULLA-DECISION-ENGINE-R157.2` | covers the equal-to-threshold edge, the below-threshold case and the window replacement |
| `crates/medulla-h-decide/src/decide/stat_tests/delegation_quality_tests.rs::stale_window_refuses` | L3 | `MEDULLA-DECISION-ENGINE-R157.4` | asserts each code and that no artifact exists after a refusal |
| `crates/medulla-h-decide/src/decide/stat_tests/delegation_quality_tests.rs::quality_evidence_binds` | L3 | `MEDULLA-DECISION-ENGINE-R157.5` | reads the artifact and asserts every listed field and class |
| `tests/decide/test_delegation_quality_served.py::test_quality_gate_admits_and_refuses` | L4 | `MEDULLA-DECISION-ENGINE-R157.6` | asserts admit above threshold and refusal below it |
| `crates/medulla-h-decide/src/decide/stat_tests/run_adjustment_tests.rs::run_adjustment_journey` | L3 | `MEDULLA-DECISION-ENGINE-R158`, `MEDULLA-DECISION-ENGINE-R158.1`, `MEDULLA-DECISION-ENGINE-R158.2` | Served proof: `tests/decide/test_error_adjustment_served.py::test_per_run_adjustment` (L4) |
| `tests/decide/test_error_adjustment_served.py::test_per_run_adjustment` | L4 | `MEDULLA-DECISION-ENGINE-R158`, `MEDULLA-DECISION-ENGINE-R158.6` | Proves: Per-run error frequency drives model and width adjustment (rollup) |
| `crates/medulla-decision-types/src/decision/run_adjustment.rs::tests::adjustment_types_round_trip` | L1 | `MEDULLA-DECISION-ENGINE-R158.1` | Proves: Adjustment records |
| `crates/medulla-decision-types/src/decision/run_adjustment.rs::tests::adjustment_rules` | L1 | `MEDULLA-DECISION-ENGINE-R158.2` | covers 0.03, 0.01 and 0.004 error rates, the top tier, width 1 and the host ceiling |
| `crates/medulla-h-decide/src/decide/stat_tests/run_adjustment_tests.rs::per_run_adjustment_routes` | L3 | `MEDULLA-DECISION-ENGINE-R158.3` | seeds ledger records and asserts the options |
| `crates/medulla-h-decide/src/decide/stat_tests/run_adjustment_tests.rs::other_tenant_runs_are_not_counted` | L3 | `MEDULLA-DECISION-ENGINE-R158.3` | Proves: Run adjustment family wiring |
| `crates/medulla-h-decide/src/decide/stat_tests/run_adjustment_tests.rs::invalid_policy_refuses` | L3 | `MEDULLA-DECISION-ENGINE-R158.4` | covers each code and the abstention |
| `crates/medulla-h-decide/src/decide/stat_tests/run_adjustment_tests.rs::adjustment_evidence_binds` | L3 | `MEDULLA-DECISION-ENGINE-R158.5` | asserts each premise and the before and after values |
| `crates/medulla-h-decide/src/decide/stat_tests/path_preference_tests.rs::served_decide_prefers_the_higher_success_path` | L3 | `MEDULLA-DECISION-ENGINE-R159`, `MEDULLA-DECISION-ENGINE-R159.1`, `MEDULLA-DECISION-ENGINE-R159.2`, `MEDULLA-DECISION-ENGINE-R159.3` | The test seeds outcomes through `DecisionLog` and asserts the preferred path, `ResolutionKind::Statistical` and `EvidenceClass::Observation` |
| `crates/medulla-decision-types/src/decision/path_success.rs::tests::path_identity_round_trip` | L1 | `MEDULLA-DECISION-ENGINE-R159.1` | asserts a stable `path_id` and a round trip of every type |
| `crates/medulla-decision-rank/src/rank/path_success.rs::tests::success_preference_gates` | L1 | `MEDULLA-DECISION-ENGINE-R159.2` | asserts the eligibility, preference, skip and tie rules |
| `crates/medulla-h-decide/src/decide/stat_tests/path_preference_tests.rs::other_tenant_outcomes_never_count` | L3 | `MEDULLA-DECISION-ENGINE-R159.3` | is the cross-tenant negative case |
| `crates/medulla-h-decide/src/decide/stat_tests/path_preference_tests.rs::unmeasured_and_stale_paths_refuse` | L3 | `MEDULLA-DECISION-ENGINE-R159.4` | asserts each code, the elimination premise and the abstention when no path remains |
| `crates/medulla-h-decide/src/decide/stat_tests/path_preference_tests.rs::path_success_evidence_binds` | L3 | `MEDULLA-DECISION-ENGINE-R159.5` | asserts the identity, the numerator and denominator, the window and the preference decision |
| `tests/decide/test_path_success_served.py::test_success_history_drives_preference` | L4 | `MEDULLA-DECISION-ENGINE-R159.6` | asserts the preferred path and the skip through the wire client |
| `crates/medulla-h-admin/src/admin/package_validation_tests.rs::invalid_packages_refuse_publication` | L3 | `MEDULLA-DECISION-ENGINE-R160`, `MEDULLA-DECISION-ENGINE-R160.1`, `MEDULLA-DECISION-ENGINE-R160.2` | passes |
| `crates/medulla-decision-types/src/decision/package_validation.rs::tests::validation_types_round_trip` | L1 | `MEDULLA-DECISION-ENGINE-R160.1` | Proves: Package validation model |
| `crates/medulla-decision-types/src/decision/package_validation.rs::tests::validation_gates_publication` | L1 | `MEDULLA-DECISION-ENGINE-R160.2` | covers a valid history, a failing run and a digest mismatch |
| `crates/medulla-h-admin/src/admin/package_validation_tests.rs::package_validation_routes` | L3 | `MEDULLA-DECISION-ENGINE-R160.3` | asserts that a valid package publishes and the catalog returns it |
| `crates/medulla-h-admin/src/admin/package_validation_tests.rs::validation_fails_refuse_publication` | L3 | `MEDULLA-DECISION-ENGINE-R160.4` | asserts each code and that no receipt exists after a refusal |
| `crates/medulla-h-admin/src/admin/package_validation_tests.rs::validation_evidence_binds` | L3 | `MEDULLA-DECISION-ENGINE-R160.5` | reads the receipt and asserts each field and the class |
| `tests/catalog/test_package_validation_served.py::test_invalid_packages_refuse_publication` | L4 | `MEDULLA-DECISION-ENGINE-R160.6` | asserts the invalid refusal and the recorded valid receipt |
| `crates/medulla-h-decide/src/decide/stat_tests/correction_admission_tests.rs::receipts_gate_correction_admission` | L3 | `MEDULLA-DECISION-ENGINE-R161`, `MEDULLA-DECISION-ENGINE-R161.1`, `MEDULLA-DECISION-ENGINE-R161.2`, `MEDULLA-DECISION-ENGINE-R161.6` | passes |
| `crates/medulla-decision-types/src/decision/trace_fix_binding.rs::tests::binding_types_round_trip` | L1 | `MEDULLA-DECISION-ENGINE-R161.1` | Proves: Correction binding model |
| `crates/medulla-decision-types/src/decision/trace_fix_binding.rs::tests::receipt_binding_gates_admission` | L1 | `MEDULLA-DECISION-ENGINE-R161.2` | covers the pass case and each broken link |
| `crates/medulla-h-decide/src/decide/stat_tests/correction_admission_tests.rs::trace_to_test_routes` | L3 | `MEDULLA-DECISION-ENGINE-R161.3` | asserts the admit outcome, `ResolutionKind::Constraint`, `EvidenceClass::Claim` and one cross-tenant negative case |
| `crates/medulla-h-decide/src/decide/stat_tests/correction_admission_tests.rs::incomplete_binding_refuses` | L3 | `MEDULLA-DECISION-ENGINE-R161.4` | asserts each code and that the store is unchanged |
| `crates/medulla-h-decide/src/decide/stat_tests/correction_admission_tests.rs::binding_evidence_records` | L3 | `MEDULLA-DECISION-ENGINE-R161.5` | asserts each premise and the class |
| `crates/medulla-h-decide/src/decide/stat_tests/field_shielding_tests.rs::undeclared_fields_withheld_journey` | L3 | `MEDULLA-DECISION-ENGINE-R162`, `MEDULLA-DECISION-ENGINE-R162.1`, `MEDULLA-DECISION-ENGINE-R162.2`, `MEDULLA-DECISION-ENGINE-R162.6` | passes |
| `crates/medulla-decision-types/src/decision/field_shielding.rs::tests::types_round_trip` | L1 | `MEDULLA-DECISION-ENGINE-R162.1` | Proves: Field shielding model |
| `crates/medulla-decision-types/src/decision/field_shielding.rs::tests::shielding_removes_nested` | L1 | `MEDULLA-DECISION-ENGINE-R162.2` | covers nested shielded paths, depth edges and an undeclared sibling |
| `crates/medulla-h-decide/src/decide/stat_tests/field_shielding_tests.rs::shielding_routes` | L3 | `MEDULLA-DECISION-ENGINE-R162.3` | asserts the allowed and withheld sets, `ResolutionKind::Constraint` and one cross-tenant negative case |
| `crates/medulla-h-decide/src/decide/stat_tests/field_shielding_tests.rs::undeclared_fields_refuse` | L3 | `MEDULLA-DECISION-ENGINE-R162.4` | asserts each code and the named path |
| `crates/medulla-h-decide/src/decide/stat_tests/field_shielding_tests.rs::shielding_evidence_binds` | L3 | `MEDULLA-DECISION-ENGINE-R162.5` | asserts each premise and the class |
| `crates/medulla-h-decide/src/decide/stat_tests/graph_intake_tests.rs::mixed_source_resolution` | L3 | `MEDULLA-DECISION-ENGINE-R163`, `MEDULLA-DECISION-ENGINE-R163.1`, `MEDULLA-DECISION-ENGINE-R163.2`, `MEDULLA-DECISION-ENGINE-R163.6` | passes |
| `crates/medulla-decision-types/src/decision/graph_intake.rs::tests::intake_types_round_trip` | L1 | `MEDULLA-DECISION-ENGINE-R163.1` | Proves: Graph intake model |
| `crates/medulla-decision-types/src/decision/graph_intake.rs::tests::resolved_source_keeps_authority_not_body` | L1 | `MEDULLA-DECISION-ENGINE-R163.2` | asserts the resolved type holds no body |
| `crates/medulla-h-decide/src/decide/stat_tests/graph_intake_tests.rs::graph_intake_routes` | L3 | `MEDULLA-DECISION-ENGINE-R163.3` | asserts the resolved set, revisions, `ResolutionKind::Constraint` and one cross-tenant negative case |
| `crates/medulla-h-decide/src/decide/stat_tests/graph_intake_tests.rs::missing_graph_data_refuses` | L3 | `MEDULLA-DECISION-ENGINE-R163.4` | asserts each code |
| `crates/medulla-h-decide/src/decide/stat_tests/graph_intake_tests.rs::graph_intake_evidence_binds` | L3 | `MEDULLA-DECISION-ENGINE-R163.5` | asserts each premise and the weakest-class rule |
| `crates/medulla-h-decide/src/decide/stat_tests/branch_outcomes_tests.rs::breadth_adapts_from_prior_attempts` | L3 | `MEDULLA-DECISION-ENGINE-R164`, `MEDULLA-DECISION-ENGINE-R164.1`, `MEDULLA-DECISION-ENGINE-R164.2`, `MEDULLA-DECISION-ENGINE-R164.6` | asserts breadth 5 after the successes, the sealed premises and a cross-tenant negative case |
| `crates/medulla-decision-stats/src/decision/branch_outcomes.rs::tests::breadth_adapts_from_outcomes` | L1 | `MEDULLA-DECISION-ENGINE-R164.2` | covers raise, narrow, hold and clamp |
| `crates/medulla-h-decide/src/decide/stat_tests/branch_outcomes_tests.rs::breadth_adapts_from_history` | L3 | `MEDULLA-DECISION-ENGINE-R164.3` | records outcomes, decides under tenant and principal context and asserts the breadth |
| `crates/medulla-h-decide/src/decide/stat_tests/branch_outcomes_tests.rs::inconsistent_outcomes_refuse` | L3 | `MEDULLA-DECISION-ENGINE-R164.4` | submits each case and asserts the code and the `Abstention` resolution kind |
| `crates/medulla-h-decide/src/decide/stat_tests/branch_outcomes_tests.rs::outcome_evidence_records` | L3 | `MEDULLA-DECISION-ENGINE-R164.5` | asserts the premises, the trigger branch and the evidence class |
| `crates/medulla-h-decide/src/decide/stat_tests/tool_necessity_tests.rs::necessity_journey` | L3 | `MEDULLA-DECISION-ENGINE-R165`, `MEDULLA-DECISION-ENGINE-R165.1`, `MEDULLA-DECISION-ENGINE-R165.2` | Served proof: `tests/decide/test_tool_necessity_served.py::test_necessity_mismatch_trace` (L4) |
| `tests/decide/test_tool_necessity_served.py::test_necessity_mismatch_trace` | L4 | `MEDULLA-DECISION-ENGINE-R165`, `MEDULLA-DECISION-ENGINE-R165.6` | Proves: Per-model tool necessity and decision comparison (rollup) |
| `crates/medulla-decision-types/src/decision/tool_necessity.rs::tests::necessity_types_round_trip` | L1 | `MEDULLA-DECISION-ENGINE-R165.1` | Proves: Tool necessity records |
| `crates/medulla-decision-types/src/decision/tool_necessity.rs::tests::necessity_comparison_detects_mismatch` | L1 | `MEDULLA-DECISION-ENGINE-R165.2` | covers both mismatch reasons and the sample floor |
| `crates/medulla-h-decide/src/decide/stat_tests/tool_necessity_tests.rs::necessity_comparison_routes` | L3 | `MEDULLA-DECISION-ENGINE-R165.3` | seeds run traces and asserts the options |
| `crates/medulla-h-decide/src/decide/stat_tests/tool_necessity_tests.rs::other_tenant_traces_are_not_read` | L3 | `MEDULLA-DECISION-ENGINE-R165.3` | Proves: Tool necessity family wiring |
| `crates/medulla-h-decide/src/decide/stat_tests/tool_necessity_tests.rs::unknown_measurements_refuse` | L3 | `MEDULLA-DECISION-ENGINE-R165.4` | covers each code |
| `crates/medulla-h-decide/src/decide/stat_tests/tool_necessity_tests.rs::mismatch_links_to_the_model_call` | L3 | `MEDULLA-DECISION-ENGINE-R165.5` | asserts the premises and the call id |
| `crates/medulla-h-graph/src/graph_ops/workflow_registry_tests.rs::register_read_and_refuse_unknown` | L3 | `MEDULLA-DECISION-ENGINE-R166`, `MEDULLA-DECISION-ENGINE-R166.3` | passes |
| `crates/medulla-h-graph/src/graph_ops/workflow_registry_tests.rs::unknown_identity_refuses_without_default` | L3 | `MEDULLA-DECISION-ENGINE-R166.4` | asserts each code and that a refused `Register` leaves the graph unchanged |
| `tests/workflows/test_registry_served.py::test_register_read_and_unknown_refusal` | L4 | `MEDULLA-DECISION-ENGINE-R166.6` | asserts readback equality and the unknown-identity code |
| `crates/medulla-h-decide/src/decide/stat_tests/skill_upgrade_tests.rs::upgrade_admission_journey` | L3 | `MEDULLA-DECISION-ENGINE-R167`, `MEDULLA-DECISION-ENGINE-R167.1`, `MEDULLA-DECISION-ENGINE-R167.2`, `MEDULLA-DECISION-ENGINE-R167.6` | passes |
| `crates/medulla-decision-outcome-types/src/decision/skill_upgrade.rs::tests::additive_only_diff` | L1 | `MEDULLA-DECISION-ENGINE-R167.2` | covers addition, removal, type change and optional-to-required |
| `crates/medulla-h-decide/src/decide/stat_tests/skill_upgrade_tests.rs::compatible_and_migrated_upgrades_admit` | L3 | `MEDULLA-DECISION-ENGINE-R167.3` | asserts the admit, `ResolutionKind::Constraint`, `EvidenceClass::Claim` and one cross-tenant negative case |
| `crates/medulla-h-decide/src/decide/stat_tests/skill_upgrade_tests.rs::breaking_upgrade_without_migration_refuses` | L3 | `MEDULLA-DECISION-ENGINE-R167.4` | asserts each code and that the store is unchanged |
| `crates/medulla-h-decide/src/decide/stat_tests/skill_upgrade_tests.rs::upgrade_evidence_names_digests_and_migration` | L3 | `MEDULLA-DECISION-ENGINE-R167.5` | asserts both premises |
| `tests/lifecycle/test_agent_lifecycle_served.py::test_agent_lifecycle_roundtrip` | L4 | `MEDULLA-DECISION-ENGINE-R168`, `MEDULLA-DECISION-ENGINE-R168.1`, `MEDULLA-DECISION-ENGINE-R168.2`, `MEDULLA-DECISION-ENGINE-R168.6` | passes |
| `crates/medulla-agent-types/src/agent_component.rs::tests::lifecycle_boundaries_are_listed` | L1 | `MEDULLA-DECISION-ENGINE-R168.1` | asserts each boundary maps to one type |
| `crates/medulla-agent-types/src/agent_component.rs::tests::lifecycle_transition_table` | L1 | `MEDULLA-DECISION-ENGINE-R168.2` | covers each allowed and each refused pair |
| `tests/lifecycle/test_agent_lifecycle_served.py::test_malformed_lifecycle_refuses` | L4 | `MEDULLA-DECISION-ENGINE-R168.4` | asserts each code and that no revision is written |
| `tests/lifecycle/test_agent_lifecycle_served.py::test_transition_entry_names_states_and_digest` | L4 | `MEDULLA-DECISION-ENGINE-R168.5` | reads the entry and asserts both states and the digest |
| `crates/medulla-h-decide/src/decide/stat_tests/feedback_loop_tests.rs::feedback_loop_stops` | L3 | `MEDULLA-DECISION-ENGINE-R169`, `MEDULLA-DECISION-ENGINE-R169.1`, `MEDULLA-DECISION-ENGINE-R169.2`, `MEDULLA-DECISION-ENGINE-R169.6` | passes |
| `crates/medulla-decision-types/src/decision/checker_feedback.rs::tests::records_round_trip` | L1 | `MEDULLA-DECISION-ENGINE-R169.1` | Proves: Feedback record model |
| `crates/medulla-decision-types/src/decision/checker_feedback.rs::tests::next_round_rule` | L1 | `MEDULLA-DECISION-ENGINE-R169.2` | covers no findings, the bound edge and a continue |
| `crates/medulla-h-decide/src/decide/stat_tests/feedback_loop_tests.rs::continuation_survives_a_fresh_harness` | L3 | `MEDULLA-DECISION-ENGINE-R169.3` | commits a round and opens a fresh `Harness` over the same store |
| `crates/medulla-h-decide/src/decide/stat_tests/feedback_loop_tests.rs::feedback_refusals_do_not_advance` | L3 | `MEDULLA-DECISION-ENGINE-R169.4` | asserts each code and an unchanged continuation |
| `crates/medulla-h-decide/src/decide/stat_tests/feedback_loop_tests.rs::feedback_evidence_names_both_digests` | L3 | `MEDULLA-DECISION-ENGINE-R169.5` | asserts the premises for every round |
| `crates/medulla-h-decide/src/decide/stat_tests/team_coordination_tests.rs::fallback_and_team_record` | L3 | `MEDULLA-DECISION-ENGINE-R170`, `MEDULLA-DECISION-ENGINE-R170.1`, `MEDULLA-DECISION-ENGINE-R170.2`, `MEDULLA-DECISION-ENGINE-R170.6` | passes |
| `crates/medulla-decision-types/src/decision/team_coordination.rs::tests::channel_signal_round_trip` | L1 | `MEDULLA-DECISION-ENGINE-R170.1` | Proves: Team vocabulary and channel signal |
| `crates/medulla-h-decide/src/decide/stat_tests/team_coordination_tests.rs::unavailable_channel_selects_team` | L3 | `MEDULLA-DECISION-ENGINE-R170.2` | covers an available channel, an unavailable channel and a stale signal |
| `tests/decide/test_team_coordination_served.py::test_team_nodes_validate_and_query` | L4 | `MEDULLA-DECISION-ENGINE-R170.3` | writes a team, reads members and task statuses by graph query and checks the shape refusal |
| `crates/medulla-h-decide/src/decide/stat_tests/team_coordination_tests.rs::team_refusals` | L3 | `MEDULLA-DECISION-ENGINE-R170.4` | asserts each code and that no team node appears |
| `crates/medulla-h-decide/src/decide/stat_tests/team_coordination_tests.rs::team_evidence_names_signal_and_members` | L3 | `MEDULLA-DECISION-ENGINE-R170.5` | asserts both premises and the class |
| `crates/medulla-h-decide/src/decide/stat_tests/planning_admission_tests.rs::served_decide_admits_only_wired_features` | L3 | `MEDULLA-DECISION-ENGINE-R171`, `MEDULLA-DECISION-ENGINE-R171.1`, `MEDULLA-DECISION-ENGINE-R171.2`, `MEDULLA-DECISION-ENGINE-R171.3` | The test asserts admitted, unwired-refused and stale-view cases, with `ResolutionKind::Constraint` and `EvidenceClass::Observation` |
| `crates/medulla-decision-types/src/decision/planning_admission.rs::tests::admission_request_round_trips` | L1 | `MEDULLA-DECISION-ENGINE-R171.1` | asserts a round trip and rejects 65 features |
| `crates/medulla-decision-plan/src/plan/planning_admission.rs::tests::set_difference_orders_differences` | L1 | `MEDULLA-DECISION-ENGINE-R171.2` | asserts a wired plan admits and an unwired plan lists differences in feature order |
| `crates/medulla-h-decide/src/decide/stat_tests/planning_admission_tests.rs::other_tenant_repository_is_invisible` | L3 | `MEDULLA-DECISION-ENGINE-R171.3` | is the cross-tenant negative case |
| `crates/medulla-h-decide/src/decide/stat_tests/planning_admission_tests.rs::admission_refusals` | L3 | `MEDULLA-DECISION-ENGINE-R171.4` | submits one request per code and asserts the stable code, the named feature and no admission |
| `crates/medulla-h-decide/src/decide/stat_tests/planning_admission_tests.rs::premises_name_view_and_each_attach_point` | L3 | `MEDULLA-DECISION-ENGINE-R171.5` | asserts the view premise and one attach premise per feature |
| `tests/planning/test_wiring_admission_served.py::test_unwired_feature_refused` | L4 | `MEDULLA-DECISION-ENGINE-R171.6` | covers the admitted, unwired-refused and stale-view cases |
| `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::designation_journey` | L3 | `MEDULLA-DECISION-ENGINE-R172`, `MEDULLA-DECISION-ENGINE-R172.1`, `MEDULLA-DECISION-ENGINE-R172.2`, `MEDULLA-DECISION-ENGINE-R172.3`, `MEDULLA-DECISION-ENGINE-R172.4`, `MEDULLA-DECISION-ENGINE-R172.5`, `MEDULLA-DECISION-ENGINE-R172.5.1` | It writes components, asks a filtered question, logs outcomes and asks again |
| `crates/medulla-decision-types/src/decision/capability_designation.rs::tests::designation_types_round_trip` | L1 | `MEDULLA-DECISION-ENGINE-R172.1` | A serde round trip keeps every field |
| `crates/medulla-decision-memory/src/memory/capability_index/tests.rs::incremental_upsert_and_tombstone_match_rebuild` | L1 | `MEDULLA-DECISION-ENGINE-R172.2`, `MEDULLA-DECISION-ENGINE-R172.2.3` | An index built by N incremental upserts and deletes returns the same top-k as a full rebuild |
| `crates/medulla-decision-memory/src/memory/capability_index/tests.rs::postings_intersect_and_union_subtypes` | L2 | `MEDULLA-DECISION-ENGINE-R172.2.1` | A property test compares `filtered_pool` with a brute-force filter over random records |
| `crates/medulla-decision-memory/src/memory/capability_index/tests.rs::sq8_scan_with_rescore_matches_exact_top_k` | L2 | `MEDULLA-DECISION-ENGINE-R172.2.2` | It asserts recall@5 of at least 0.95 against an exact f32 scan on 2000 seeded vectors of dimension 64 |
| `crates/medulla-decision-types/src/decision/capability_designation.rs::tests::eligibility_gates_match_fixtures` | L1 | `MEDULLA-DECISION-ENGINE-R172.3` | Fixtures cover a subtype match, a policy exclusion, a commons record, a foreign-tenant record and the combined filters |
| `crates/medulla-decision-rank/src/rank/capability_rank.rs::tests::blend_order_matches_fixtures` | L2 | `MEDULLA-DECISION-ENGINE-R172.4` | Fixtures show planted top-1 recovery and type-coherent top-k |
| `crates/medulla-decision-types/src/decision/capability_designation.rs::tests::reward_ema_erasure_and_decay` | L1 | `MEDULLA-DECISION-ENGINE-R172.5` | Fixtures show a bounded EMA and a reset on a material re-embed |
| `crates/medulla-persistence-records/src/agent_library/open_tests.rs::capability_reward_survives_reopen` | L1 | `MEDULLA-DECISION-ENGINE-R172.5.1` | It writes a reward, reopens the store and asserts the same value |
| `crates/medulla-decision-memory/src/memory/capability_index/tests.rs::version_pin_refuses_mixed_spaces` | L1 | `MEDULLA-DECISION-ENGINE-R172.6` | A mismatched version or dim refuses on upsert and on question |
| `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::decide_routes_capability_designation` | L3 | `MEDULLA-DECISION-ENGINE-R172.7` | It asserts typed designations, `ResolutionKind::Statistical`, `EvidenceClass::Observation`, and a cross-tenant negative case |
| `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::capability_outcome_updates_reward_and_refuses` | L3 | `MEDULLA-DECISION-ENGINE-R172.7.1` | It asserts the new reward and the trace fields: prior, new, alpha, generation and source ids |
| `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::committed_library_write_is_visible_to_next_question` | L3 | `MEDULLA-DECISION-ENGINE-R172.7.2` | It publishes a component, asks, withdraws it, asks again and asserts both answers, plus a cross-tenant negative case |
| `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::capability_designation_refusals` | L3 | `MEDULLA-DECISION-ENGINE-R172.8` | Each case asserts its code, the outcome, and that no reward, index epoch or decision record changed |
| `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::premises_name_scores_and_knobs` | L3 | `MEDULLA-DECISION-ENGINE-R172.9` | It asserts every score term and knob premise for every designation, and that the outcome trace links its source ids |
| `benches/decide_budgets.rs::capability_designation` (bench group `decide/capability_designation`) | L6 | `MEDULLA-DECISION-ENGINE-R172.10` | group `decide/capability_designation` (L6) reports p50, p99, recall@5 and resident bytes |
| `tests/decide/test_capability_designation_served.py::test_designation_end_to_end` | L4 | `MEDULLA-DECISION-ENGINE-R172.11` | runs against a loopback engine with authentication |
| `tests/decide/test_capability_designation_pack_served.py::test_designation_reads_imported_pack` | L4 | `MEDULLA-DECISION-ENGINE-R172.12` | It imports a pack, asks for a target class and asserts the executing tool and subclass path |
## Planned bindings, range 6

Each entry names a test that does not exist yet, at its post-split path. The test is written with its row; until then the row stays SPECIFIED.

| Row | Planned test | State |
|---|---|---|
| `MEDULLA-DECISION-ENGINE-R172.12.1` | `crates/medulla-decision-types/src/decision/capability_designation.rs::tests::pack_surface_fields_round_trip` | (planned) |
| `MEDULLA-DECISION-ENGINE-R172.12.1` | `tests/decide/test_capability_designation_pack_served.py::test_designation_reads_imported_pack` | (planned) |
| `MEDULLA-DECISION-ENGINE-R172.12.2` | `crates/medulla-decision-types/src/decision/capability_designation.rs::tests::target_class_subsumption_selects_tools` | (planned) |
| `MEDULLA-DECISION-ENGINE-R172.12.2` | `tests/decide/test_capability_designation_pack_served.py::test_designation_reads_imported_pack` | (planned) |
| `MEDULLA-DECISION-ENGINE-R172.12.3` | `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::projection_commit_upserts_pack_records` | (planned) |
| `MEDULLA-DECISION-ENGINE-R172.12.4` | `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::pack_target_refusals` | (planned) |
| `MEDULLA-DECISION-ENGINE-R172.12.5` | `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::premises_name_subclass_path_and_pack` | (planned) |
| `MEDULLA-DECISION-ENGINE-R172.12.6` | `tests/decide/test_capability_designation_pack_served.py::test_designation_reads_imported_pack` | (planned) |
| `MEDULLA-DECISION-ENGINE-R172.13` | `crates/medulla-compute/src/capability_index/tests.rs::capability_adapter_applies_before_rank` | (planned) |
| `MEDULLA-DECISION-ENGINE-R172.13` | `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::designation_journey` | (planned) |
| `MEDULLA-DECISION-ENGINE-R175` | `crates/medulla-h-decide/src/decide/stat_tests/item_recommendation_tests.rs::pause_steps_lift_ndcg` | (planned) |
| `MEDULLA-DECISION-ENGINE-R175.2` | `crates/medulla-decision-stats/src/decision/recommend.rs::tests::seeded_fixture_ranks_identically_twice` | (planned) |
| `MEDULLA-DECISION-ENGINE-R175.2` | `crates/medulla-h-decide/src/decide/stat_tests/item_recommendation_tests.rs::pause_steps_lift_ndcg` | (planned) |
| `MEDULLA-DECISION-ENGINE-R175.3` | `crates/medulla-h-decide/src/decide/stat_tests/item_recommendation_tests.rs::decide_routes_item_recommendation_to_the_stored_encoder` | (planned) |
| `MEDULLA-DECISION-ENGINE-R175.4` | `crates/medulla-h-decide/src/decide/stat_tests/item_recommendation_tests.rs::item_recommendation_refusals` | (planned) |
| `MEDULLA-DECISION-ENGINE-R175.5` | `crates/medulla-h-decide/src/decide/stat_tests/item_recommendation_tests.rs::record_lists_premises_and_claim` | (planned) |
| `MEDULLA-DECISION-ENGINE-R175.6` | `crates/medulla-h-decide/src/decide/stat_tests/item_recommendation_tests.rs::pause_steps_lift_ndcg` | (planned) |
| `MEDULLA-DECISION-ENGINE-R175.7` | `crates/medulla-decision-outcome-types/src/decision/recommend/tests.rs::request_and_answer_round_trip` | (planned) |
| `MEDULLA-DECISION-ENGINE-R175.7` | `crates/medulla-h-decide/src/decide/stat_tests/item_recommendation_tests.rs::pause_steps_lift_ndcg` | (planned) |
| `MEDULLA-DECISION-ENGINE-R175.8` | `crates/medulla-h-decide/src/decide/stat_tests/item_recommendation_tests.rs::fit_stores_an_encoder_keyed_by_catalog_digest` | (planned) |
| `MEDULLA-DECISION-ENGINE-R173` | `crates/medulla-h-decide/src/decide/stat_tests/concentration_tests.rs::concentration_journey` | (planned) |
| `MEDULLA-DECISION-ENGINE-R173` | `tests/decide/test_concentration_served.py::test_third_same_dimension_edit_reverts` | (planned) |
| `MEDULLA-DECISION-ENGINE-R173.3` | `crates/medulla-h-decide/src/decide/stat_tests/concentration_tests.rs::decide_matches_shacl` | (planned) |
| `MEDULLA-DECISION-ENGINE-R173.3` | `crates/medulla-h-decide/src/decide/stat_tests/concentration_tests.rs::other_tenant_edits_are_not_counted` | (planned) |
| `MEDULLA-DECISION-ENGINE-R173.5` | `crates/medulla-h-decide/src/decide/stat_tests/concentration_tests.rs::record_lists_counted_edits` | (planned) |
| `MEDULLA-DECISION-ENGINE-R173.6` | `tests/decide/test_concentration_served.py::test_third_same_dimension_edit_reverts` | (planned) |
| `MEDULLA-DECISION-ENGINE-R134.110` | `crates/medulla-h-decide/src/decide/stat_tests/tool_relevance_tests.rs::served_ranking_selects_covering_subset` | (planned) |
| `MEDULLA-DECISION-ENGINE-R134.110.1` | `crates/medulla-decision-types/src/decision/tool_relevance.rs::tests::assignment_round_trips` | (planned) |
| `MEDULLA-DECISION-ENGINE-R134.110.1` | `crates/medulla-h-decide/src/decide/stat_tests/tool_relevance_tests.rs::served_ranking_selects_covering_subset` | (planned) |
| `MEDULLA-DECISION-ENGINE-R134.110.2` | `crates/medulla-decision-stats/src/decision/tool_relevance.rs::tests::fused_order_and_visible_only_scores` | (planned) |
| `MEDULLA-DECISION-ENGINE-R134.110.2` | `crates/medulla-h-decide/src/decide/stat_tests/tool_relevance_tests.rs::served_ranking_selects_covering_subset` | (planned) |
| `MEDULLA-DECISION-ENGINE-R134.110.3` | `crates/medulla-h-decide/src/decide/stat_tests/tool_relevance_tests.rs::catalog_change_reorders_the_ranking` | (planned) |
| `MEDULLA-DECISION-ENGINE-R134.110.4` | `crates/medulla-h-decide/src/decide/stat_tests/tool_relevance_tests.rs::each_refusal_returns_its_code_and_no_tools` | (planned) |
| `MEDULLA-DECISION-ENGINE-R134.110.5` | `crates/medulla-h-decide/src/decide/stat_tests/tool_relevance_tests.rs::record_lists_scores_rank_and_revision` | (planned) |
| `MEDULLA-DECISION-ENGINE-R134.110.6` | `tests/decide/test_tool_assignment_served.py::test_role_assignment_uses_one_ranking` | (planned) |
| `MEDULLA-DECISION-ENGINE-R134.110.6` | `crates/medulla-h-decide/src/decide/stat_tests/tool_relevance_tests.rs::catalog_change_reorders_the_ranking` | (planned) |
| `MEDULLA-DECISION-ENGINE-R134.110.7` | `crates/medulla-h-decide/src/decide/stat_tests/tool_relevance_tests.rs::index_follows_library_generation_and_hides_invisible_tools` | (planned) |
| `MEDULLA-DECISION-ENGINE-R100.102` | `crates/medulla-h-decide/src/decide/stat_tests/run_credit_tests.rs::run_outcome_credits_steps_and_records_hard_negatives` | (planned) |
| `MEDULLA-DECISION-ENGINE-R100.102.1` | `crates/medulla-decision-types/src/decision/statistical/run_credit.rs::tests::run_outcome_round_trips` | (planned) |
| `MEDULLA-DECISION-ENGINE-R100.102.1` | `crates/medulla-h-decide/src/decide/stat_tests/run_credit_tests.rs::run_outcome_credits_steps_and_records_hard_negatives` | (planned) |
| `MEDULLA-DECISION-ENGINE-R100.102.2` | `crates/medulla-decision-stats/src/decision/run_credit.rs::tests::baseline_moves_by_the_weighted_difference` | (planned) |
| `MEDULLA-DECISION-ENGINE-R100.102.2` | `crates/medulla-h-decide/src/decide/stat_tests/run_credit_tests.rs::run_outcome_credits_steps_and_records_hard_negatives` | (planned) |
| `MEDULLA-DECISION-ENGINE-R100.102.3` | `crates/medulla-h-decide/src/decide/stat_run_credit.rs::record_run` | (planned) |
| `MEDULLA-DECISION-ENGINE-R100.102.3` | `crates/medulla-h-decide/src/decide/stat_tests/run_credit_tests.rs::run_outcome_credits_steps_and_records_hard_negatives` | (planned) |
| `MEDULLA-DECISION-ENGINE-R100.102.4` | `crates/medulla-h-decide/src/decide/stat_run_credit.rs::record_run` | (planned) |
| `MEDULLA-DECISION-ENGINE-R100.102.4` | `crates/medulla-h-decide/src/decide/stat_tests/run_credit_tests.rs::run_outcome_refusals_write_nothing` | (planned) |
| `MEDULLA-DECISION-ENGINE-R100.102.5` | `crates/medulla-h-decide/src/decide/stat_tests/run_credit_tests.rs::run_credit_receipt_reads_back_after_one_run` | (planned) |
| `MEDULLA-DECISION-ENGINE-R100.102.6` | `tests/decide/test_run_credit_served.py::test_cited_rows_and_step_credit_recorded` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174` | `crates/medulla-h-decide/src/decide/stat_tests/generation_cycle_tests.rs::cycle_publishes_reembeds_evaluates_and_activates` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.1` | `crates/medulla-decision-types/src/decision/statistical/retrieval_generation.rs::tests::cycle_round_trips` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.1` | `crates/medulla-h-decide/src/decide/stat_tests/generation_cycle_tests.rs::cycle_publishes_reembeds_evaluates_and_activates` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.2` | `crates/medulla-h-decide/src/decide/stat_tests/generation_cycle_tests.rs::resume_activates_once` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.2` | `crates/medulla-h-decide/src/decide/stat_tests/generation_cycle_tests.rs::cycle_publishes_reembeds_evaluates_and_activates` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.3` | `crates/medulla-h-decide/src/decide/stat_tests/generation_cycle_tests.rs::submit_creates_a_job_and_holds_a_lease` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.4` | `crates/medulla-h-decide/src/decide/stat_tests/generation_cycle_tests.rs::refusal_codes` | |
| `MEDULLA-DECISION-ENGINE-R174.5` | `crates/medulla-h-decide/src/decide/stat_tests/generation_cycle_tests.rs::receipt_holds_model_counts_metrics_and_pointers` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.6` | `tests/decide/test_generation_cycle_served.py::test_cycle_resumes_and_activates` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.6` | `crates/medulla-h-decide/src/decide/stat_tests/generation_cycle_tests.rs::cycle_publishes_reembeds_evaluates_and_activates` | (planned) |
| `MEDULLA-DECISION-ENGINE-R176` | `crates/medulla-h-decide/src/decide/stat_tests/load_set_tests.rs::served_decide_reaches_load_set` | (planned) |
| `MEDULLA-DECISION-ENGINE-R176` | `tests/decide/test_load_set_served.py::test_job_returns_bounded_tool_skill_agent_set` | (planned) |
| `MEDULLA-DECISION-ENGINE-R176.2` | `crates/medulla-decision-outcome-types/src/decision/load_set.rs::tests::bounded_union_in_stable_order` | (planned) |
| `MEDULLA-DECISION-ENGINE-R176.2` | `crates/medulla-h-decide/src/decide/stat_tests/load_set_tests.rs::served_decide_reaches_load_set` | (planned) |
| `MEDULLA-DECISION-ENGINE-R176.3` | `crates/medulla-h-decide/src/decide/stat_tests/load_set_tests.rs::served_decide_reaches_load_set` | (planned) |
| `MEDULLA-DECISION-ENGINE-R176.3` | `crates/medulla-h-decide/src/decide/stat_tests/load_set_tests.rs::other_tenant_packs_never_enter_the_set` | (planned) |
| `MEDULLA-DECISION-ENGINE-R176.4` | `crates/medulla-h-decide/src/decide/stat_tests/load_set_tests.rs::load_set_refusals` | (planned) |
| `MEDULLA-DECISION-ENGINE-R176.5` | `crates/medulla-h-decide/src/decide/stat_tests/load_set_tests.rs::premises_per_component` | (planned) |
| `MEDULLA-DECISION-ENGINE-R176.6` | `tests/decide/test_load_set_served.py::test_job_returns_bounded_tool_skill_agent_set` | (planned) |
| `MEDULLA-DECISION-ENGINE-R177` | `crates/medulla-h-decide/src/decide/stat_tests/usage_guidance_tests.rs::served_decide_reaches_guidance` | (planned) |
| `MEDULLA-DECISION-ENGINE-R177` | `tests/decide/test_usage_guidance_served.py::test_how_to_use_connector_answers_from_graph` | (planned) |
| `MEDULLA-DECISION-ENGINE-R177.2` | `crates/medulla-decision-stats/src/decision/usage_guidance.rs::tests::steps_ordered_by_outcome` | (planned) |
| `MEDULLA-DECISION-ENGINE-R177.2` | `crates/medulla-decision-stats/src/decision/usage_guidance.rs::tests::order_is_deterministic` | (planned) |
| `MEDULLA-DECISION-ENGINE-R177.2` | `crates/medulla-h-decide/src/decide/stat_tests/usage_guidance_tests.rs::served_decide_reaches_guidance` | (planned) |
| `MEDULLA-DECISION-ENGINE-R177.3` | `crates/medulla-h-decide/src/decide/stat_tests/usage_guidance_tests.rs::served_decide_reaches_guidance` | (planned) |
| `MEDULLA-DECISION-ENGINE-R177.3` | `crates/medulla-h-decide/src/decide/stat_tests/usage_guidance_tests.rs::other_tenant_outcomes_never_order_steps` | (planned) |
| `MEDULLA-DECISION-ENGINE-R177.4` | `crates/medulla-h-decide/src/decide/stat_tests/usage_guidance_tests.rs::guidance_refusals` | (planned) |
| `MEDULLA-DECISION-ENGINE-R177.5` | `crates/medulla-h-decide/src/decide/stat_tests/usage_guidance_tests.rs::steps_cite_triples` | (planned) |
| `MEDULLA-DECISION-ENGINE-R177.6` | `tests/decide/test_usage_guidance_served.py::test_how_to_use_connector_answers_from_graph` | (planned) |
| `MEDULLA-DECISION-ENGINE-R178.2` | `tools/xtask/src/decide_budgets.rs::tests::a_planted_regression_fails` | (planned) |
| `MEDULLA-DECISION-ENGINE-R178.3` | `tests/test_check_decide_budgets.py::test_workflow_invokes_the_check` | (planned) |
| `MEDULLA-DECISION-ENGINE-R179.1` | `crates/medulla-decision-types/src/decision/knobs.rs::tests::knob_specs_round_trip_and_defaults_lie_within_bounds` | (planned) |
| `MEDULLA-DECISION-ENGINE-R179.2` | `crates/medulla-h-decide/src/decide/stat_tests/knob_tests.rs::decide_reads_the_pinned_knob_value_and_falls_back_to_the_default` | (planned) |
| `MEDULLA-DECISION-ENGINE-R179.3` | `crates/medulla-h-decide/src/decide/stat_tests/knob_tests.rs::a_promoted_knob_revision_changes_the_served_decision_and_rolls_back` | (planned) |
| `MEDULLA-DECISION-ENGINE-R179.4` | `crates/medulla-h-decide/src/decide/stat_tests/knob_tests.rs::knob_revision_refusals` | (planned) |
| `MEDULLA-DECISION-ENGINE-R179.5` | `crates/medulla-h-decide/src/decide/stat_tests/knob_tests.rs::record_lists_every_knob_read` | (planned) |
| `MEDULLA-DECISION-ENGINE-R179.6` | `tests/decide/test_knob_evolution_served.py::test_promote_and_roll_back_a_knob` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.7` | `crates/medulla-h-decide/src/decide/stat_tests/scoped_adapter_tests.rs::outcome_adapter_lifts_and_reverts` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.7.1` | `crates/medulla-decision-types/src/decision/statistical/retrieval_adapter.rs::tests::scoped_fit_request_round_trips` | |
| `MEDULLA-DECISION-ENGINE-R174.7.2` | `crates/medulla-decision-stats/src/decision/adapter.rs::tests::scoped_refit_replays_bit_for_bit` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.7.2` | `crates/medulla-h-decide/src/decide/stat_tests/scoped_adapter_tests.rs::outcome_adapter_lifts_and_reverts` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.7.2.1` | `crates/medulla-decision-stats/src/decision/adapter.rs::tests::monitor_rolls_back_on_degrading_outcomes` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.7.2.1` | `crates/medulla-h-decide/src/decide/stat_tests/scoped_adapter_tests.rs::outcome_adapter_lifts_and_reverts` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.7.3` | `crates/medulla-h-decide/src/decide/stat_tests/scoped_adapter_tests.rs::scoped_fit_activation_and_rollback_move_the_pointer` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.7.3.1` | `crates/medulla-h-decide/src/decide/stat_tests/scoped_adapter_tests.rs::discover_applies_the_resolved_adapter` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.7.3.2` | `crates/medulla-h-decide/src/decide/stat_tests/scoped_adapter_tests.rs::index_adapter_materializes_without_an_endpoint_call` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.7.4` | `crates/medulla-h-decide/src/decide/stat_tests/scoped_adapter_tests.rs::scoped_adapter_refusals` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.7.5` | `crates/medulla-h-decide/src/decide/stat_tests/scoped_adapter_tests.rs::fit_and_rollback_receipts_and_verdict_field` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.7.6` | `tests/decide/test_scoped_adapter_served.py::test_outcome_adapter_lifts_and_reverts` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.7.6` | `crates/medulla-h-decide/src/decide/stat_tests/scoped_adapter_tests.rs::outcome_adapter_lifts_and_reverts` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.8` | `crates/medulla-h-decide/src/decide/stat_tests/drift_refresh_tests.rs::vocabulary_drift_refreshes_changed_shards` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.8.1` | `crates/medulla-decision-types/src/decision/statistical/retrieval_drift.rs::tests::refresh_plan_round_trips` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.8.1` | `crates/medulla-h-decide/src/decide/stat_tests/drift_refresh_tests.rs::vocabulary_drift_refreshes_changed_shards` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.8.2` | `crates/medulla-decision-stats/src/decision/drift.rs::tests::shift_signal_fires_after_two_evaluations` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.8.2` | `crates/medulla-h-decide/src/decide/stat_tests/drift_refresh_tests.rs::vocabulary_drift_refreshes_changed_shards` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.8.2.1` | `crates/medulla-decision-stats/src/decision/drift.rs::tests::quality_decay_and_staleness_fire` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.8.2.1` | `crates/medulla-h-decide/src/decide/stat_tests/drift_refresh_tests.rs::vocabulary_drift_refreshes_changed_shards` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.8.3` | `crates/medulla-h-decide/src/decide/stat_tests/drift_refresh_tests.rs::monitor_update_schedules_one_plan` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.8.3.1` | `crates/medulla-h-decide/src/decide/stat_tests/drift_refresh_tests.rs::concurrent_readers_see_one_generation_per_shard` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.8.4` | `crates/medulla-h-decide/src/decide/stat_tests/drift_refresh_tests.rs::refresh_refusals` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.8.5` | `crates/medulla-h-decide/src/decide/stat_tests/drift_refresh_tests.rs::signal_premises_and_refresh_receipt` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.8.6` | `tests/decide/test_drift_refresh_served.py::test_vocabulary_drift_refreshes_changed_shards` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.8.6` | `crates/medulla-h-decide/src/decide/stat_tests/drift_refresh_tests.rs::vocabulary_drift_refreshes_changed_shards` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.9` | `crates/medulla-h-decide/src/decide/stat_tests/freshness_refresh_tests.rs::search_refresh_dedupes_and_feeds_drift_refresh` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.9.1` | `crates/medulla-decision-types/src/decision/statistical/retrieval_refresh.rs::tests::refresh_round_trips` | |
| `MEDULLA-DECISION-ENGINE-R174.9.2` | `crates/medulla-decision-types/src/decision/statistical/retrieval_refresh.rs::tests::canonical_urls_and_dedupe_outcomes` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.9.2` | `crates/medulla-h-decide/src/decide/stat_tests/freshness_refresh_tests.rs::search_refresh_dedupes_and_feeds_drift_refresh` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.9.3` | `crates/medulla-h-decide/src/decide/stat_tests/freshness_refresh_tests.rs::refresh_reads_the_virtual_graph_and_marks_one_region` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.9.3.1` | `crates/medulla-h-decide/src/decide/stat_tests/freshness_refresh_tests.rs::registered_web_source_serves_a_refresh` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.9.4` | `crates/medulla-h-decide/src/decide/stat_tests/freshness_refresh_tests.rs::freshness_refusals` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.9.5` | `crates/medulla-h-decide/src/decide/stat_tests/freshness_refresh_tests.rs::result_provenance_and_receipt` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.9.6` | `tests/decide/test_freshness_refresh_served.py::test_search_refresh_dedupes_and_feeds_refresh` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.9.6` | `crates/medulla-h-decide/src/decide/stat_tests/freshness_refresh_tests.rs::search_refresh_dedupes_and_feeds_drift_refresh` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.9.7` | `crates/medulla-h-decide/src/decide/stat_tests/freshness_refresh_tests.rs::stale_web_document_refetches_on_digest_change` | (planned) |
| `MEDULLA-DECISION-ENGINE-R174.9.7` | `tests/decide/test_freshness_refresh_served.py::test_stale_web_document_refetches_on_digest_change` | (planned) |
| `MEDULLA-DECISION-ENGINE-R614` | `tests/decide/test_semantic_enrichment_served.py::test_card_rank_classify_project_journey` | (planned) |
| `MEDULLA-DECISION-ENGINE-R614.2` | `tests/decide/test_semantic_enrichment_served.py::test_card_rank_classify_project_journey` | (planned) |
| `MEDULLA-DECISION-ENGINE-R614.2.1` | `crates/medulla-enrich/tests/capability_card.rs::card_store_accepts_grounded_and_refuses_ungrounded` | (planned) |
| `MEDULLA-DECISION-ENGINE-R614.2.2` | `crates/medulla-enrich/tests/proposal_rank.rs::value_orders_and_breaks_ties_by_id` | (planned) |
| `MEDULLA-DECISION-ENGINE-R614.2.3` | `crates/medulla-enrich/tests/topic_classify.rs::classify_picks_nearest_topic_and_respects_floor` | (planned) |
| `MEDULLA-DECISION-ENGINE-R614.2.4` | `crates/medulla-enrich/tests/projection.rs::projection_emits_stable_edges_and_skips_unresolved` | (planned) |
| `MEDULLA-DECISION-ENGINE-R614.3` | `tests/decide/test_semantic_enrichment_served.py::test_card_rank_classify_project_journey` | (planned) |
| `MEDULLA-DECISION-ENGINE-R614.4` | `tests/decide/test_semantic_enrichment_served.py::test_refusals_write_nothing` | (planned) |
| `MEDULLA-DECISION-ENGINE-R614.5` | `tests/decide/test_semantic_enrichment_served.py::test_evidence_properties_read_back` | (planned) |
| `MEDULLA-DECISION-ENGINE-R614.6` | `tests/decide/test_semantic_enrichment_served.py::test_card_rank_classify_project_journey` | (planned) |
| `MEDULLA-DECISION-ENGINE-R615` | `crates/medulla-h-decide/src/decide/stat_tests/writeback_queue_tests.rs::high_tier_proposal_queues_and_releases_after_approval` | (planned) |
| `MEDULLA-DECISION-ENGINE-R615.2` | `crates/medulla-h-decide/src/decide/stat_tests/writeback_queue_tests.rs::high_tier_proposal_queues_and_releases_after_approval` | (planned) |
| `MEDULLA-DECISION-ENGINE-R615.2.1` | `crates/medulla-decision-stats/src/decision/writeback.rs::tests::preflight_refuses_conflicts_and_queues_high_tier` | (planned) |
| `MEDULLA-DECISION-ENGINE-R615.2.1` | `crates/medulla-h-decide/src/decide/stat_tests/writeback_queue_tests.rs::high_tier_proposal_queues_and_releases_after_approval` | (planned) |
| `MEDULLA-DECISION-ENGINE-R615.2.2` | `crates/medulla-decision-stats/src/decision/writeback.rs::tests::inventory_and_findings_become_deduplicated_proposals` | (planned) |
| `MEDULLA-DECISION-ENGINE-R615.2.2` | `crates/medulla-h-decide/src/decide/stat_tests/writeback_queue_tests.rs::high_tier_proposal_queues_and_releases_after_approval` | (planned) |
| `MEDULLA-DECISION-ENGINE-R615.3` | `crates/medulla-h-decide/src/decide/stat_tests/writeback_queue_tests.rs::high_tier_proposal_queues_and_releases_after_approval` | (planned) |
| `MEDULLA-DECISION-ENGINE-R615.4` | `crates/medulla-h-decide/src/decide/stat_writeback.rs::release` | (planned) |
| `MEDULLA-DECISION-ENGINE-R615.4` | `crates/medulla-h-decide/src/decide/stat_tests/writeback_queue_tests.rs::unapproved_release_refuses_without_partial_write` | (planned) |
| `MEDULLA-DECISION-ENGINE-R615.5` | `crates/medulla-h-decide/src/decide/stat_tests/writeback_queue_tests.rs::release_and_refusal_leave_audit_rows` | (planned) |
| `MEDULLA-DECISION-ENGINE-R615.6` | `crates/medulla-h-decide/src/decide/stat_tests/writeback_queue_tests.rs::high_tier_proposal_queues_and_releases_after_approval` | (planned) |
| `MEDULLA-DECISION-ENGINE-R615.6` | `tests/decide/test_writeback_queue_served.py::test_queue_approve_release` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::source_ingest_transcript_becomes_episodic_memory` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.2` | `crates/medulla-work-types/src/memory.rs::tests::conversation_projects_to_ordered_episodic_records` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.2` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::source_ingest_transcript_becomes_episodic_memory` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.2.1` | `crates/medulla-work-types/src/memory.rs::tests::later_layer_overrides_earlier_fact` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.2.1` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::source_ingest_transcript_becomes_episodic_memory` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.2.2` | `crates/medulla-decision-stats/src/decision/memory.rs::tests::prune_plan_skips_held_and_respects_the_step_cap` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.2.2` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::source_ingest_transcript_becomes_episodic_memory` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.2.3` | `crates/medulla-decision-stats/src/decision/memory.rs::tests::expired_records_are_hidden_and_listed_for_prune` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.2.3` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::source_ingest_transcript_becomes_episodic_memory` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.2.4` | `crates/medulla-decision-stats/src/decision/memory.rs::tests::covering_set_respects_tokens_and_items` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.2.4` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::source_ingest_transcript_becomes_episodic_memory` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.3` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::source_ingest_transcript_becomes_episodic_memory` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.4` | `crates/medulla-graph-core/src/graph/txn_maintenance.rs::tests::evict_below_skips_legal_hold` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.4` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::held_memory_is_refused_and_logged` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.5` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::prune_report_records_examined_removed_held` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.6` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::source_ingest_transcript_becomes_episodic_memory` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.6` | `tests/memory/test_transcript_memory_served.py::test_chat_log_becomes_recallable_memory` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.7` | `crates/medulla-work-types/src/participant.rs::tests::participant_round_trips_and_iri_is_stable` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.7` | `crates/medulla-core/tests/participant_shape.rs::person_without_represents_fails_shape` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.7` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::participant_journey` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.8` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::ingest_adds_speakers_as_members` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.8` | `crates/medulla-work-types/src/participant.rs::tests::membership_edges_dedupe_speakers` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.9` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::participant_methods_are_served_and_classified` | (planned) |
| `MEDULLA-DECISION-ENGINE-R617.9` | `tests/memory/test_participant_memory_served.py::test_participant_put_and_member_round_trip` | (planned) |
| `MEDULLA-DECISION-ENGINE-R750` | `crates/medulla-h-decide/src/decide/stat_tests/thought_search_tests.rs::served_decide_chooses_expansion_then_stops` | (planned) |
| `MEDULLA-DECISION-ENGINE-R750.750` | `crates/medulla-decision-types/src/decision/thought_search.rs::tests::thought_types_round_trip` | (planned) |
| `MEDULLA-DECISION-ENGINE-R750.750` | `crates/medulla-h-decide/src/decide/stat_tests/thought_search_tests.rs::served_decide_chooses_expansion_then_stops` | (planned) |
| `MEDULLA-DECISION-ENGINE-R750.751` | `crates/medulla-decision-stats/src/decision/thought_search.rs::tests::strategies_pick_the_documented_leaves` | (planned) |
| `MEDULLA-DECISION-ENGINE-R750.751` | `crates/medulla-h-decide/src/decide/stat_tests/thought_search_tests.rs::served_decide_chooses_expansion_then_stops` | (planned) |
| `MEDULLA-DECISION-ENGINE-R750.752` | `crates/medulla-h-decide/src/decide/stat_tests/thought_search_tests.rs::thought_search_refusals` | (planned) |
| `MEDULLA-DECISION-ENGINE-R750.753` | `crates/medulla-h-decide/src/decide/stat_tests/thought_search_tests.rs::served_decide_chooses_expansion_then_stops` | (planned) |
| `MEDULLA-DECISION-ENGINE-R750.754` | `crates/medulla-h-decide/src/decide/stat_tests/thought_search_tests.rs::served_decide_chooses_expansion_then_stops` | (planned) |
| `MEDULLA-DECISION-ENGINE-R750.754` | `crates/medulla-h-decide/src/decide/stat_tests/thought_search_tests.rs::premises_name_strategy_ledger_and_nodes` | (planned) |
| `MEDULLA-DECISION-ENGINE-R750.754` | `crates/medulla-h-decide/src/decide/stat_tests/thought_search_tests.rs::other_tenant_policy_is_not_read` | (planned) |
| `MEDULLA-DECISION-ENGINE-R751` | `crates/medulla-h-decide/src/decide/stat_tests/reward_decomposition_tests.rs::served_decide_decomposes_a_trajectory_reward` | (planned) |
| `MEDULLA-DECISION-ENGINE-R751` | `crates/medulla-h-decide/src/decide/stat_tests/curriculum_promotion_tests.rs::served_decide_promotes_on_a_cleared_window` | (planned) |
| `MEDULLA-DECISION-ENGINE-R751.750` | `crates/medulla-decision-types/src/decision/reward.rs::tests::reward_types_round_trip` | (planned) |
| `MEDULLA-DECISION-ENGINE-R751.750` | `crates/medulla-h-decide/src/decide/stat_tests/reward_decomposition_tests.rs::served_decide_decomposes_a_trajectory_reward` | (planned) |
| `MEDULLA-DECISION-ENGINE-R751.751` | `crates/medulla-decision-stats/src/decision/reward.rs::tests::rewards_sum_exactly_and_floor_applies` | (planned) |
| `MEDULLA-DECISION-ENGINE-R751.751` | `crates/medulla-h-decide/src/decide/stat_tests/reward_decomposition_tests.rs::served_decide_decomposes_a_trajectory_reward` | (planned) |
| `MEDULLA-DECISION-ENGINE-R751.752` | `crates/medulla-h-decide/src/decide/stat_tests/reward_decomposition_tests.rs::reward_refusals` | (planned) |
| `MEDULLA-DECISION-ENGINE-R751.752` | `crates/medulla-h-decide/src/decide/stat_tests/curriculum_promotion_tests.rs::curriculum_refusals` | (planned) |
| `MEDULLA-DECISION-ENGINE-R178.1` | `tests/test_check_decide_budgets.py::test_bench_groups_cover_every_family` | (planned) |
| `MEDULLA-DECISION-ENGINE-R751.753` | `crates/medulla-h-decide/src/decide/stat_tests/reward_decomposition_tests.rs::served_decide_decomposes_a_trajectory_reward` | L3 |
| `MEDULLA-DECISION-ENGINE-R751.754` | `crates/medulla-h-decide/src/decide/stat_tests/curriculum_promotion_tests.rs::served_decide_promotes_on_a_cleared_window` | L3 |
| `MEDULLA-DECISION-ENGINE-R751.754` | `crates/medulla-h-decide/src/decide/stat_tests/curriculum_promotion_tests.rs::other_tenant_outcomes_never_count` | L3 |
| `MEDULLA-DECISION-ENGINE-R751.754` | `crates/medulla-decision-stats/src/decision/curriculum.rs::tests::promotion_needs_a_full_window` | L1 |
| `MEDULLA-DECISION-ENGINE-R751.755` | `crates/medulla-h-decide/src/decide/stat_tests/reward_decomposition_tests.rs::served_decide_decomposes_a_trajectory_reward` | L3 |
| `MEDULLA-DECISION-ENGINE-R751.755` | `crates/medulla-h-decide/src/decide/stat_tests/reward_decomposition_tests.rs::premises_name_trajectory_and_depth` | L3 |
| `MEDULLA-DECISION-ENGINE-R751.755` | `crates/medulla-h-decide/src/decide/stat_tests/reward_decomposition_tests.rs::other_tenant_trajectory_is_not_read` | L3 |
| `MEDULLA-DECISION-ENGINE-R752` | `crates/medulla-h-decide/src/decide/stat_tests/population_tests.rs::served_decide_reads_drift_and_picks_width` | L3 |
| `MEDULLA-DECISION-ENGINE-R752` | `crates/medulla-h-decide/src/decide/stat_tests/autonomy_tests.rs::served_decide_requires_confirmation_for_irreversible_actions` | L3 |
| `MEDULLA-DECISION-ENGINE-R752.750` | `crates/medulla-decision-types/src/decision/population.rs::tests::population_types_round_trip` | L1 |
| `MEDULLA-DECISION-ENGINE-R752.750` | `crates/medulla-h-decide/src/decide/stat_tests/population_tests.rs::served_decide_reads_drift_and_picks_width` | L3 |
| `MEDULLA-DECISION-ENGINE-R752.751` | `crates/medulla-decision-stats/src/decision/population.rs::tests::wasserstein_matches_hand_computed_cases` | L1 |
| `MEDULLA-DECISION-ENGINE-R752.751` | `crates/medulla-decision-stats/src/decision/population.rs::tests::width_never_exceeds_the_cap` | L1 |
| `MEDULLA-DECISION-ENGINE-R752.751` | `crates/medulla-decision-stats/src/decision/population.rs::tests::drift_and_width_replay_golden_vectors` | L2 |
| `MEDULLA-DECISION-ENGINE-R752.751` | `crates/medulla-h-decide/src/decide/stat_tests/population_tests.rs::served_decide_reads_drift_and_picks_width` | L3 |
| `MEDULLA-DECISION-ENGINE-R752.752` | `crates/medulla-h-decide/src/decide/stat_tests/population_tests.rs::population_refusals` | L3 |
| `MEDULLA-DECISION-ENGINE-R752.752` | `crates/medulla-h-decide/src/decide/stat_tests/autonomy_tests.rs::autonomy_refusals` | L3 |
| `MEDULLA-DECISION-ENGINE-R752.753` | `crates/medulla-h-decide/src/decide/stat_tests/population_tests.rs::served_decide_reads_drift_and_picks_width` | L3 |
| `MEDULLA-DECISION-ENGINE-R752.754` | `crates/medulla-h-decide/src/decide/stat_tests/population_tests.rs::served_decide_reads_drift_and_picks_width` | L3 |
| `MEDULLA-DECISION-ENGINE-R752.754` | `crates/medulla-h-decide/src/decide/stat_tests/population_tests.rs::other_tenant_inputs_are_not_read` | L3 |
| `MEDULLA-DECISION-ENGINE-R752.755` | `crates/medulla-h-decide/src/decide/stat_tests/autonomy_tests.rs::served_decide_requires_confirmation_for_irreversible_actions` | L3 |
| `MEDULLA-DECISION-ENGINE-R752.755` | `crates/medulla-h-decide/src/decide/stat_tests/autonomy_tests.rs::trust_clears_after_the_clean_run_and_resets_on_failure` | L3 |
| `MEDULLA-DECISION-ENGINE-R752.755` | `crates/medulla-h-decide/src/decide/stat_tests/autonomy_tests.rs::other_tenant_outcomes_never_raise_trust` | L3 |
| `MEDULLA-DECISION-ENGINE-R753` | `crates/medulla-h-decide/src/decide/stat_tests/trajectory_join_tests.rs::served_decide_joins_and_distills_trajectories` | L3 |
| `MEDULLA-DECISION-ENGINE-R753.750` | `crates/medulla-decision-types/src/decision/trajectory_verdict.rs::tests::verdict_types_round_trip` | L1 |
| `MEDULLA-DECISION-ENGINE-R753.750` | `crates/medulla-h-decide/src/decide/stat_tests/trajectory_join_tests.rs::served_decide_joins_and_distills_trajectories` | L3 |
| `MEDULLA-DECISION-ENGINE-R753.751` | `crates/medulla-decision-stats/src/decision/trajectory_join.rs::tests::join_is_deterministic_under_ties` | L1 |
| `MEDULLA-DECISION-ENGINE-R753.751` | `crates/medulla-decision-stats/src/decision/step_verify.rs::tests::recovery_follows_attempts` | L1 |
| `MEDULLA-DECISION-ENGINE-R753.751` | `crates/medulla-decision-stats/src/decision/trajectory_join.rs::tests::join_ignores_input_order` | L2 |
| `MEDULLA-DECISION-ENGINE-R753.751` | `crates/medulla-h-decide/src/decide/stat_tests/trajectory_join_tests.rs::served_decide_joins_and_distills_trajectories` | L3 |
| `MEDULLA-DECISION-ENGINE-R753.752` | `crates/medulla-h-decide/src/decide/stat_tests/trajectory_join_tests.rs::join_refusals` | L3 |
| `MEDULLA-DECISION-ENGINE-R753.752` | `crates/medulla-h-decide/src/decide/stat_tests/step_verification_tests.rs::verification_and_admission_refusals` | L3 |
| `MEDULLA-DECISION-ENGINE-R753.753` | `crates/medulla-h-decide/src/decide/stat_tests/trajectory_join_tests.rs::served_decide_joins_and_distills_trajectories` | L3 |
| `MEDULLA-DECISION-ENGINE-R753.754` | `crates/medulla-h-decide/src/decide/stat_tests/trajectory_join_tests.rs::served_decide_joins_and_distills_trajectories` | L3 |
| `MEDULLA-DECISION-ENGINE-R753.754` | `crates/medulla-h-decide/src/decide/stat_tests/trajectory_join_tests.rs::derivation_names_every_source` | L3 |
| `MEDULLA-DECISION-ENGINE-R753.754` | `crates/medulla-h-decide/src/decide/stat_tests/trajectory_join_tests.rs::other_tenant_trajectories_are_not_read` | L3 |
| `MEDULLA-DECISION-ENGINE-R753.755` | `crates/medulla-h-decide/src/decide/stat_tests/step_verification_tests.rs::served_decide_binds_the_verdict_to_the_trace` | L3 |
| `MEDULLA-DECISION-ENGINE-R753.755` | `crates/medulla-h-decide/src/decide/stat_tests/step_verification_tests.rs::served_decide_rejects_a_compromised_candidate` | L3 |
| `MEDULLA-DECISION-ENGINE-R753.755` | `crates/medulla-h-decide/src/decide/stat_tests/step_verification_tests.rs::other_tenant_traces_are_not_read` | L3 |
| `MEDULLA-DECISION-ENGINE-R035.750` | `crates/medulla-h-decide/src/decide/stat_tests/route_tests.rs::first_matching_route_stage_wins` | L3 |
| `MEDULLA-DECISION-ENGINE-R035.750.1` | `crates/medulla-decision-stats/src/decision/route.rs::tests::fast_path_wins_below_the_token_bound` | L1 |
| `MEDULLA-DECISION-ENGINE-R035.750.1` | `crates/medulla-h-decide/src/decide/stat_tests/route_tests.rs::first_matching_route_stage_wins` | L3 |
| `MEDULLA-DECISION-ENGINE-R035.750.2` | `crates/medulla-h-decide/src/decide/stat_tests/route_tests.rs::a_proven_team_is_reused_before_the_learned_policy` | L3 |
| `MEDULLA-DECISION-ENGINE-R035.751` | `crates/medulla-h-decide/src/decide/stat_tests/route_tests.rs::route_journey_covers_every_stage` | L3 |
| `MEDULLA-DECISION-ENGINE-R046.750` | `crates/medulla-h-decide/src/decide/stat_tests/topology_tests.rs::admission_enforces_caps_and_replays` | L3 |
| `MEDULLA-DECISION-ENGINE-R134.751` | `crates/medulla-h-decide/src/decide/stat_tests/tool_relevance_tests.rs::catalog_above_the_cap_refuses_and_cap_size_is_served` | L3 |
| `MEDULLA-DECISION-ENGINE-R650` | `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::descriptor_closure_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R650.2` | `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::descriptor_closure_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R650.2.1` | `crates/medulla-agent-types/src/agent_component/capability_descriptor.rs::tests::reliability_is_derived_from_reward_and_never_stored` | L1 |
| `MEDULLA-DECISION-ENGINE-R650.2.1` | `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::descriptor_closure_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R650.2.2` | `crates/medulla-agent-types/src/agent_component/capability_descriptor.rs::tests::power_descriptor_is_stable_for_equal_inputs` | L2 |
| `MEDULLA-DECISION-ENGINE-R650.2.2` | `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::descriptor_closure_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R650.2.3` | `crates/medulla-agent-types/src/agent_component/capability_descriptor.rs::tests::closure_is_deterministic_across_runs_and_threads` | L2 |
| `MEDULLA-DECISION-ENGINE-R650.3` | `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::designation_returns_descriptor_and_closure` | L3 |
| `MEDULLA-DECISION-ENGINE-R650.4` | `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::descriptor_refusals_and_tenant_isolation` | L3 |
| `MEDULLA-DECISION-ENGINE-R650.5` | `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::descriptor_evidence_reads_back_through_decision_log` | L3 |
| `MEDULLA-DECISION-ENGINE-R650.6` | `crates/medulla-h-decide/src/decide/stat_tests/capability_designation_tests.rs::descriptor_closure_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R651` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::remember_recall_consolidate_and_export_round_trip` | L3 |
| `MEDULLA-DECISION-ENGINE-R651.1` | `crates/medulla-work-types/src/memory.rs::tests::memory_record_round_trips_through_node_props` | L1 |
| `MEDULLA-DECISION-ENGINE-R651.1` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::remember_recall_consolidate_and_export_round_trip` | L3 |
| `MEDULLA-DECISION-ENGINE-R651.2` | `crates/medulla-decision-stats/src/decision/memory.rs::tests::plan_is_deterministic_across_thread_counts` | L2 |
| `MEDULLA-DECISION-ENGINE-R651.2` | `crates/medulla-decision-stats/src/decision/memory.rs::tests::near_duplicates_merge_to_the_highest_importance` | L1 |
| `MEDULLA-DECISION-ENGINE-R651.2` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::remember_recall_consolidate_and_export_round_trip` | L3 |
| `MEDULLA-DECISION-ENGINE-R651.2.1` | `crates/medulla-decision-stats/src/decision/memory.rs::tests::mementos_respect_the_token_cap_and_keep_sources` | L1 |
| `MEDULLA-DECISION-ENGINE-R651.2.1` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::remember_recall_consolidate_and_export_round_trip` | L3 |
| `MEDULLA-DECISION-ENGINE-R651.2.2` | `crates/medulla-decision-stats/src/decision/memory.rs::tests::cka_is_one_for_identical_snapshots_and_drops_for_shuffled` | L1 |
| `MEDULLA-DECISION-ENGINE-R651.2.2` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::remember_recall_consolidate_and_export_round_trip` | L3 |
| `MEDULLA-DECISION-ENGINE-R651.2.3` | `crates/medulla-decision-stats/src/decision/memory.rs::tests::referenced_entries_are_never_trajectory_candidates` | L1 |
| `MEDULLA-DECISION-ENGINE-R651.2.3` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::remember_recall_consolidate_and_export_round_trip` | L3 |
| `MEDULLA-DECISION-ENGINE-R651.3` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::memory_methods_are_served_and_classified` | L3 |
| `MEDULLA-DECISION-ENGINE-R651.3` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::remember_recall_consolidate_and_export_round_trip` | L3 |
| `MEDULLA-DECISION-ENGINE-R651.4` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::memory_refusals` | L3 |
| `MEDULLA-DECISION-ENGINE-R651.5` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::consolidation_and_export_evidence_reads_back` | L3 |
| `MEDULLA-DECISION-ENGINE-R651.6` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::remember_recall_consolidate_and_export_round_trip` | L3 |
| `MEDULLA-DECISION-ENGINE-R651.6` | `tests/memory/test_memory_served.py::test_remember_recall_plan_export` | L4 |
| `MEDULLA-DECISION-ENGINE-R652` | `crates/medulla-h-decide/src/decide/stat_tests/claims_tests.rs::claim_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R652.1` | `crates/medulla-types/src/claims.rs::tests::claim_types_round_trip_and_deny_unknown_fields` | L1 |
| `MEDULLA-DECISION-ENGINE-R652.1` | `crates/medulla-h-decide/src/decide/stat_tests/claims_tests.rs::claim_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R652.2` | `crates/medulla-epistemic/src/claims.rs::tests::lifecycle_table_and_log_odds_are_deterministic` | L2 |
| `MEDULLA-DECISION-ENGINE-R652.2` | `crates/medulla-h-decide/src/decide/stat_tests/claims_tests.rs::claim_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R652.3` | `crates/medulla-h-decide/src/decide/stat_tests/claims_tests.rs::claim_ops_reach_the_handler` | L3 |
| `MEDULLA-DECISION-ENGINE-R652.3` | `tests/decide/test_claims_served.py::test_claim_lifecycle_served` | L4 |
| `MEDULLA-DECISION-ENGINE-R652.4` | `crates/medulla-h-decide/src/decide/stat_tests/claims_tests.rs::claim_refusals_change_no_state` | L3 |
| `MEDULLA-DECISION-ENGINE-R652.5` | `crates/medulla-h-decide/src/decide/stat_tests/claims_tests.rs::claim_evidence_reads_back_through_a_second_principal` | L3 |
| `MEDULLA-DECISION-ENGINE-R652.6` | `crates/medulla-h-decide/src/decide/stat_tests/claims_tests.rs::claim_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R652.6` | `tests/decide/test_claims_served.py::test_claim_lifecycle_served` | L4 |
| `MEDULLA-DECISION-ENGINE-R652.7` | `crates/medulla-epistemic/src/claims.rs::tests::contradiction_pairs_and_feed_grades_are_deterministic` | L2 |
| `MEDULLA-DECISION-ENGINE-R652.7` | `crates/medulla-h-decide/src/decide/stat_tests/claims_tests.rs::claim_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R652.8` | `crates/medulla-epistemic/src/claims.rs::tests::seal_detects_any_field_change` | L1 |
| `MEDULLA-DECISION-ENGINE-R652.8` | `crates/medulla-h-decide/src/decide/stat_tests/claims_tests.rs::claim_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R653` | `crates/medulla-h-decide/src/decide/stat_tests/loops_tests.rs::loop_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R653.1` | `crates/medulla-types/src/loops.rs::tests::loop_types_round_trip_and_deny_unknown_fields` | L1 |
| `MEDULLA-DECISION-ENGINE-R653.1` | `crates/medulla-h-decide/src/decide/stat_tests/loops_tests.rs::loop_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R653.2` | `crates/medulla-statechart/src/loops.rs::tests::lease_epochs_fence_and_queue_order_is_total` | L2 |
| `MEDULLA-DECISION-ENGINE-R653.2` | `crates/medulla-h-decide/src/decide/stat_tests/loops_tests.rs::loop_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R653.3` | `crates/medulla-h-decide/src/decide/stat_tests/loops_tests.rs::loop_ops_reach_the_handler` | L3 |
| `MEDULLA-DECISION-ENGINE-R653.3` | `tests/decide/test_loops_served.py::test_loop_claim_served` | L4 |
| `MEDULLA-DECISION-ENGINE-R653.4` | `crates/medulla-h-decide/src/decide/stat_tests/loops_tests.rs::loop_refusals_change_no_state` | L3 |
| `MEDULLA-DECISION-ENGINE-R653.5` | `crates/medulla-h-decide/src/decide/stat_tests/loops_tests.rs::loop_evidence_reads_back_through_a_second_principal` | L3 |
| `MEDULLA-DECISION-ENGINE-R653.6` | `crates/medulla-h-decide/src/decide/stat_tests/loops_tests.rs::loop_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R653.6` | `tests/decide/test_loops_served.py::test_loop_claim_served` | L4 |
| `MEDULLA-DECISION-ENGINE-R653.7` | `crates/medulla-statechart/src/loops.rs::tests::settle_once_and_offer_rate_formula` | L1 |
| `MEDULLA-DECISION-ENGINE-R653.7` | `crates/medulla-h-decide/src/decide/stat_tests/loops_tests.rs::loop_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R653.8` | `crates/medulla-statechart/src/loops.rs::tests::miner_bounds_and_ratchet_verdict` | L2 |
| `MEDULLA-DECISION-ENGINE-R653.8` | `crates/medulla-h-decide/src/decide/stat_tests/loops_tests.rs::loop_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R654` | `crates/medulla-h-decide/src/decide/stat_tests/skill_evolution_tests.rs::paired_rollouts_promote_a_skill_revision` | L3 |
| `MEDULLA-DECISION-ENGINE-R654.1` | `crates/medulla-decision-types/src/decision/skill_evolution.rs::tests::rollout_set_round_trips` | L1 |
| `MEDULLA-DECISION-ENGINE-R654.1` | `crates/medulla-h-decide/src/decide/stat_tests/skill_evolution_tests.rs::paired_rollouts_promote_a_skill_revision` | L3 |
| `MEDULLA-DECISION-ENGINE-R654.2` | `crates/medulla-decision-stats/src/decision/promotion.rs::tests::paired_bootstrap_is_deterministic_across_thread_counts` | L2 |
| `MEDULLA-DECISION-ENGINE-R654.2` | `crates/medulla-h-decide/src/decide/stat_tests/skill_evolution_tests.rs::paired_rollouts_promote_a_skill_revision` | L3 |
| `MEDULLA-DECISION-ENGINE-R654.3` | `crates/medulla-h-decide/src/decide/stat_tests/skill_evolution_tests.rs::paired_rollouts_promote_a_skill_revision` | L3 |
| `MEDULLA-DECISION-ENGINE-R654.4` | `crates/medulla-h-decide/src/decide/stat_tests/skill_evolution_tests.rs::rollout_and_spec_refusals` | L3 |
| `MEDULLA-DECISION-ENGINE-R654.5` | `crates/medulla-h-decide/src/decide/stat_tests/skill_evolution_tests.rs::rollout_evidence_reads_back_field_for_field` | L3 |
| `MEDULLA-DECISION-ENGINE-R654.6` | `crates/medulla-h-decide/src/decide/stat_tests/skill_evolution_tests.rs::served_skill_evolution_journey` | L3 |
| `MEDULLA-DECISION-ENGINE-R700` | `crates/medulla-h-decide/src/decide/stat_tests/belief_revision_tests.rs::belief_pass_proposes_and_never_retracts` | L3 |
| `MEDULLA-DECISION-ENGINE-R700.1` | `crates/medulla-types/src/belief_revision.rs::tests::belief_types_round_trip_with_golden_vector` | L1 |
| `MEDULLA-DECISION-ENGINE-R700.1` | `crates/medulla-h-decide/src/decide/stat_tests/belief_revision_tests.rs::belief_pass_proposes_and_never_retracts` | L3 |
| `MEDULLA-DECISION-ENGINE-R700.2` | `crates/medulla-epistemic/src/belief_revision.rs::tests::pass_cost_is_linear_in_changed_claims` | L2 |
| `MEDULLA-DECISION-ENGINE-R700.2` | `crates/medulla-h-decide/src/decide/stat_tests/belief_revision_tests.rs::belief_pass_proposes_and_never_retracts` | L3 |
| `MEDULLA-DECISION-ENGINE-R700.3` | `crates/medulla-h-decide/src/decide/stat_tests/belief_revision_tests.rs::belief_pass_reaches_the_handler` | L3 |
| `MEDULLA-DECISION-ENGINE-R700.3` | `tests/decide/test_belief_revision_served.py::test_pass_served` | L4 |
| `MEDULLA-DECISION-ENGINE-R700.4` | `crates/medulla-h-decide/src/decide/stat_tests/belief_revision_tests.rs::belief_refusals_leave_state_unchanged` | L3 |
| `MEDULLA-DECISION-ENGINE-R700.5` | `crates/medulla-h-decide/src/decide/stat_tests/belief_revision_tests.rs::stored_revision_replays_to_the_same_posterior` | L3 |
| `MEDULLA-DECISION-ENGINE-R700.6` | `crates/medulla-h-decide/src/decide/stat_tests/belief_revision_tests.rs::belief_pass_proposes_and_never_retracts` | L3 |
| `MEDULLA-DECISION-ENGINE-R700.6` | `tests/decide/test_belief_revision_served.py::test_pass_served` | L4 |
| `MEDULLA-DECISION-ENGINE-R701` | `crates/medulla-h-decide/src/decide/stat_tests/failure_patterns_tests.rs::failures_cluster_and_gap_is_idempotent` | L3 |
| `MEDULLA-DECISION-ENGINE-R701.1` | `crates/medulla-types/src/failure_patterns.rs::tests::failure_types_round_trip_with_golden_vector` | L1 |
| `MEDULLA-DECISION-ENGINE-R701.1` | `crates/medulla-h-decide/src/decide/stat_tests/failure_patterns_tests.rs::failures_cluster_and_gap_is_idempotent` | L3 |
| `MEDULLA-DECISION-ENGINE-R701.2` | `crates/medulla-epistemic/src/failure_patterns.rs::tests::cluster_is_linear_and_signatures_normalize` | L2 |
| `MEDULLA-DECISION-ENGINE-R701.2` | `crates/medulla-h-decide/src/decide/stat_tests/failure_patterns_tests.rs::failures_cluster_and_gap_is_idempotent` | L3 |
| `MEDULLA-DECISION-ENGINE-R701.3` | `crates/medulla-h-decide/src/decide/stat_tests/failure_patterns_tests.rs::ingest_files_one_gap_per_topic` | L3 |
| `MEDULLA-DECISION-ENGINE-R701.3` | `tests/decide/test_failure_patterns_served.py::test_ingest_served` | L4 |
| `MEDULLA-DECISION-ENGINE-R701.4` | `crates/medulla-h-decide/src/decide/stat_tests/failure_patterns_tests.rs::failure_refusals_change_no_state` | L3 |
| `MEDULLA-DECISION-ENGINE-R701.5` | `crates/medulla-h-decide/src/decide/stat_tests/failure_patterns_tests.rs::gap_links_back_to_its_member_runs` | L3 |
| `MEDULLA-DECISION-ENGINE-R701.6` | `crates/medulla-h-decide/src/decide/stat_tests/failure_patterns_tests.rs::failures_cluster_and_gap_is_idempotent` | L3 |
| `MEDULLA-DECISION-ENGINE-R701.6` | `tests/decide/test_failure_patterns_served.py::test_ingest_served` | L4 |
| `MEDULLA-DECISION-ENGINE-R702` | `crates/medulla-h-decide/src/decide/stat_tests/correction_tests.rs::correction_commits_three_effects_atomically` | L3 |
| `MEDULLA-DECISION-ENGINE-R702.1` | `crates/medulla-types/src/correction.rs::tests::correction_types_round_trip_with_golden_vector` | L1 |
| `MEDULLA-DECISION-ENGINE-R702.1` | `crates/medulla-h-decide/src/decide/stat_tests/correction_tests.rs::correction_commits_three_effects_atomically` | L3 |
| `MEDULLA-DECISION-ENGINE-R702.2` | `crates/medulla-decision-stats/src/decision/correction.rs::tests::batch_holds_all_three_effects_and_k_is_bounded` | L2 |
| `MEDULLA-DECISION-ENGINE-R702.2` | `crates/medulla-h-decide/src/decide/stat_tests/correction_tests.rs::correction_commits_three_effects_atomically` | L3 |
| `MEDULLA-DECISION-ENGINE-R702.3` | `crates/medulla-h-decide/src/decide/stat_tests/correction_tests.rs::correction_reaches_the_handler` | L3 |
| `MEDULLA-DECISION-ENGINE-R702.3` | `tests/decide/test_correction_served.py::test_correction_served` | L4 |
| `MEDULLA-DECISION-ENGINE-R702.4` | `crates/medulla-h-decide/src/decide/stat_tests/correction_tests.rs::correction_refusals_commit_nothing` | L3 |
| `MEDULLA-DECISION-ENGINE-R702.5` | `crates/medulla-h-decide/src/decide/stat_tests/correction_tests.rs::correction_reads_back_by_receipt` | L3 |
| `MEDULLA-DECISION-ENGINE-R702.6` | `crates/medulla-h-decide/src/decide/stat_tests/correction_tests.rs::correction_commits_three_effects_atomically` | L3 |
| `MEDULLA-DECISION-ENGINE-R702.6` | `tests/decide/test_correction_served.py::test_correction_served` | L4 |
| `MEDULLA-DECISION-ENGINE-R703` | `crates/medulla-h-decide/src/decide/stat_tests/skill_evolution_tests.rs::candidates_are_proposals_only` | L3 |
| `MEDULLA-DECISION-ENGINE-R703.1` | `crates/medulla-decision-types/src/decision/skill_evolution.rs::tests::skill_candidates_round_trip_with_golden_vector` | L1 |
| `MEDULLA-DECISION-ENGINE-R703.1` | `crates/medulla-h-decide/src/decide/stat_tests/skill_evolution_tests.rs::candidates_are_proposals_only` | L3 |
| `MEDULLA-DECISION-ENGINE-R703.2` | `crates/medulla-decision-stats/src/decision/skill_evolution.rs::tests::blocking_bounds_pair_comparisons` | L2 |
| `MEDULLA-DECISION-ENGINE-R703.2` | `crates/medulla-h-decide/src/decide/stat_tests/skill_evolution_tests.rs::candidates_are_proposals_only` | L3 |
| `MEDULLA-DECISION-ENGINE-R703.3` | `crates/medulla-h-decide/src/decide/stat_tests/skill_evolution_tests.rs::decide_routes_skill_evolution` | L3 |
| `MEDULLA-DECISION-ENGINE-R703.4` | `crates/medulla-h-decide/src/decide/stat_tests/skill_evolution_tests.rs::merge_pair_refusals` | L3 |
| `MEDULLA-DECISION-ENGINE-R703.5` | `crates/medulla-h-decide/src/decide/stat_tests/skill_evolution_tests.rs::candidate_cites_traces_and_library_rows` | L3 |
| `MEDULLA-DECISION-ENGINE-R703.6` | `crates/medulla-h-decide/src/decide/stat_tests/skill_evolution_tests.rs::candidates_are_proposals_only` | L3 |
| `MEDULLA-DECISION-ENGINE-R704` | `crates/medulla-h-decide/src/decide/stat_tests/playbook_triage_tests.rs::flapping_service_proposes_without_acting` | L3 |
| `MEDULLA-DECISION-ENGINE-R704.1` | `crates/medulla-decision-types/src/decision/playbook.rs::tests::playbook_types_round_trip_with_golden_vector` | L1 |
| `MEDULLA-DECISION-ENGINE-R704.1` | `crates/medulla-h-decide/src/decide/stat_tests/playbook_triage_tests.rs::flapping_service_proposes_without_acting` | L3 |
| `MEDULLA-DECISION-ENGINE-R704.2` | `crates/medulla-decision-stats/src/decision/playbook.rs::tests::window_eviction_and_flap_detection` | L2 |
| `MEDULLA-DECISION-ENGINE-R704.2` | `crates/medulla-h-decide/src/decide/stat_tests/playbook_triage_tests.rs::flapping_service_proposes_without_acting` | L3 |
| `MEDULLA-DECISION-ENGINE-R704.3` | `crates/medulla-h-decide/src/decide/stat_tests/playbook_triage_tests.rs::decide_routes_playbook_triage` | L3 |
| `MEDULLA-DECISION-ENGINE-R704.4` | `crates/medulla-h-decide/src/decide/stat_tests/playbook_triage_tests.rs::playbook_refusals` | L3 |
| `MEDULLA-DECISION-ENGINE-R704.5` | `crates/medulla-h-decide/src/decide/stat_tests/playbook_triage_tests.rs::proposal_records_playbook_id_and_revision` | L3 |
| `MEDULLA-DECISION-ENGINE-R704.6` | `crates/medulla-h-decide/src/decide/stat_tests/playbook_triage_tests.rs::flapping_service_proposes_without_acting` | L3 |
| `MEDULLA-DECISION-ENGINE-R705` | `crates/medulla-h-decide/src/decide/stat_tests/research_voi_tests.rs::budget_stops_at_marginal_value` | L3 |
| `MEDULLA-DECISION-ENGINE-R705.1` | `crates/medulla-decision-types/src/decision/research_voi.rs::tests::research_types_round_trip_with_golden_vector` | L1 |
| `MEDULLA-DECISION-ENGINE-R705.1` | `crates/medulla-h-decide/src/decide/stat_tests/research_voi_tests.rs::budget_stops_at_marginal_value` | L3 |
| `MEDULLA-DECISION-ENGINE-R705.2` | `crates/medulla-decision-stats/src/decision/research_voi.rs::tests::heap_order_node_budget_and_stop_rule` | L2 |
| `MEDULLA-DECISION-ENGINE-R705.2` | `crates/medulla-h-decide/src/decide/stat_tests/research_voi_tests.rs::budget_stops_at_marginal_value` | L3 |
| `MEDULLA-DECISION-ENGINE-R705.3` | `crates/medulla-h-decide/src/decide/stat_tests/research_voi_tests.rs::decide_routes_research_next` | L3 |
| `MEDULLA-DECISION-ENGINE-R705.4` | `crates/medulla-h-decide/src/decide/stat_tests/research_voi_tests.rs::research_refusals` | L3 |
| `MEDULLA-DECISION-ENGINE-R705.5` | `crates/medulla-h-decide/src/decide/stat_tests/research_voi_tests.rs::spent_units_are_recorded_per_subtask` | L3 |
| `MEDULLA-DECISION-ENGINE-R705.6` | `crates/medulla-h-decide/src/decide/stat_tests/research_voi_tests.rs::budget_stops_at_marginal_value` | L3 |
| `MEDULLA-DECISION-ENGINE-R706` | `crates/medulla-h-decide/src/decide/stat_tests/er_proposals_tests.rs::cross_tenant_candidates_never_returned` | L3 |
| `MEDULLA-DECISION-ENGINE-R706.1` | `crates/medulla-decision-types/src/decision/entity_resolution.rs::tests::er_types_round_trip` | L1 |
| `MEDULLA-DECISION-ENGINE-R706.1` | `crates/medulla-h-decide/src/decide/stat_tests/er_proposals_tests.rs::cross_tenant_candidates_never_returned` | L3 |
| `MEDULLA-DECISION-ENGINE-R706.2` | `crates/medulla-decision-stats/src/decision/entity_resolution.rs::tests::unchanged_nodes_skip_embedding_and_drift_is_detected` | L2 |
| `MEDULLA-DECISION-ENGINE-R706.2` | `crates/medulla-h-decide/src/decide/stat_tests/er_proposals_tests.rs::cross_tenant_candidates_never_returned` | L3 |
| `MEDULLA-DECISION-ENGINE-R706.3` | `crates/medulla-h-decide/src/decide/stat_tests/er_proposals_tests.rs::resolve_entity_uses_tenant_scoped_candidates` | L3 |
| `MEDULLA-DECISION-ENGINE-R706.4` | `crates/medulla-h-decide/src/decide/stat_tests/er_proposals_tests.rs::er_refusals_and_hypothesis_marking` | L3 |
| `MEDULLA-DECISION-ENGINE-R706.5` | `crates/medulla-h-decide/src/decide/stat_tests/er_proposals_tests.rs::proposal_records_model_hash_and_tier` | L3 |
| `MEDULLA-DECISION-ENGINE-R706.6` | `crates/medulla-h-decide/src/decide/stat_tests/er_proposals_tests.rs::cross_tenant_candidates_never_returned` | L3 |
| `MEDULLA-DECISION-ENGINE-R707` | `crates/medulla-h-decide/src/decide/stat_tests/feature_match_tests.rs::closeout_requires_linked_change` | L3 |
| `MEDULLA-DECISION-ENGINE-R707.1` | `crates/medulla-decision-types/src/decision/feature_match.rs::tests::feature_types_round_trip` | L1 |
| `MEDULLA-DECISION-ENGINE-R707.1` | `crates/medulla-h-decide/src/decide/stat_tests/feature_match_tests.rs::closeout_requires_linked_change` | L3 |
| `MEDULLA-DECISION-ENGINE-R707.2` | `crates/medulla-decision-stats/src/decision/feature_match.rs::tests::blocked_scores_and_entropy_guard` | L2 |
| `MEDULLA-DECISION-ENGINE-R707.2` | `crates/medulla-decision-stats/src/decision/feature_match.rs::tests::status_changes_by_compare_and_set_on_revision` | L1 |
| `MEDULLA-DECISION-ENGINE-R707.2` | `crates/medulla-h-decide/src/decide/stat_tests/feature_match_tests.rs::closeout_requires_linked_change` | L3 |
| `MEDULLA-DECISION-ENGINE-R707.3` | `crates/medulla-h-decide/src/decide/stat_tests/feature_match_tests.rs::match_and_ledger_reach_their_handlers` | L3 |
| `MEDULLA-DECISION-ENGINE-R707.3` | `tests/decide/test_feature_ledger_served.py::test_ledger_served` | L4 |
| `MEDULLA-DECISION-ENGINE-R707.4` | `crates/medulla-h-decide/src/decide/stat_tests/feature_match_tests.rs::ledger_refusals_change_no_state` | L3 |
| `MEDULLA-DECISION-ENGINE-R707.5` | `crates/medulla-h-decide/src/decide/stat_tests/feature_match_tests.rs::ledger_rows_cite_matches_and_plan` | L3 |
| `MEDULLA-DECISION-ENGINE-R707.6` | `crates/medulla-h-decide/src/decide/stat_tests/feature_match_tests.rs::closeout_requires_linked_change` | L3 |
| `MEDULLA-DECISION-ENGINE-R707.6` | `tests/decide/test_feature_ledger_served.py::test_ledger_served` | L4 |
| `MEDULLA-DECISION-ENGINE-R046.751` | `benches/decide_budgets.rs::decide_topology_admit` | L6 |
## Planned bindings, range 8

Every row below has a planned test at its exact post-split path; the row text names the production caller it drives.

| Row | Planned test |
|---|---|
| `MEDULLA-DECISION-ENGINE-R800.1` | `crates/medulla-program/tests/optimize_front.rs::dominated_candidate_is_dropped` |
| `MEDULLA-DECISION-ENGINE-R800.2` | `crates/medulla-program/tests/optimize_front.rs::same_seed_reproduces_front_digest`; `crates/medulla-h-jobs/src/jobs/program_tests.rs::reflective_job_reports_front_and_next_parent` |
| `MEDULLA-DECISION-ENGINE-R800.3` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::reflective_job_reports_front_and_next_parent` |
| `MEDULLA-DECISION-ENGINE-R800.4` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::front_inputs_refuse_with_stable_codes` |
| `MEDULLA-DECISION-ENGINE-R800.5` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::front_digest_replays_under_one_seed` |
| `MEDULLA-DECISION-ENGINE-R800.6` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::served_reflective_job_journey` |
| `MEDULLA-DECISION-ENGINE-R801.1` | `crates/medulla-types/src/usage.rs::tests::record_round_trips_digest_only`; `tests/usage/test_usage_ledger_served.py::test_append_then_read_cost_end_to_end` |
| `MEDULLA-DECISION-ENGINE-R801.2` | `crates/medulla-persistence-records/src/usage_ledger/tests.rs::cost_uses_price_valid_at_record_time`; `crates/medulla-persistence-records/src/usage_ledger/tests.rs::alias_table_normalizes_model_keys`; `crates/medulla-persistence-records/src/usage_ledger/tests.rs::day_rollup_reads_buckets_not_records`; `tests/usage/test_usage_ledger_served.py::test_append_then_read_cost_end_to_end` |
| `MEDULLA-DECISION-ENGINE-R801.3` | `crates/medulla-capabilities/src/scopes/tests.rs::usage_domain_registers_read_and_service_only_write_scopes` |
| `MEDULLA-DECISION-ENGINE-R801.4` | `crates/medulla-persistence-records/src/usage_ledger/tests.rs::each_refusal_writes_nothing`; `crates/medulla-h-decide/src/usage/tests.rs::cross_tenant_read_is_empty` |
| `MEDULLA-DECISION-ENGINE-R801.5` | `crates/medulla-persistence-records/src/usage_ledger/tests.rs::append_writes_one_audit_line`; `crates/medulla-persistence-records/src/usage_ledger/tests.rs::golden_cost_rollup_is_byte_identical` |
| `MEDULLA-DECISION-ENGINE-R801.6` | `tests/usage/test_usage_ledger_served.py::test_append_then_read_cost_end_to_end` |
| `MEDULLA-DECISION-ENGINE-R801.7` | `crates/medulla-h-decide/src/usage/tests.rs::read_is_tenant_scoped`; `tests/usage/test_usage_ledger_served.py::test_usage_read_is_tenant_scoped` |
| `MEDULLA-DECISION-ENGINE-R801.8` | `crates/medulla-h-decide/src/usage/tests.rs::service_append_succeeds_and_human_is_refused`; `tests/usage/test_usage_ledger_served.py::test_service_append_and_price_put` |
| `MEDULLA-DECISION-ENGINE-R810.7` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::evaluated_revision_carries_held_out_evidence` |
| `MEDULLA-DECISION-ENGINE-R811.7` | `crates/medulla-h-decide/src/decide/stat_tests/policy_evolution_tests.rs::knob_set_evaluation_names_its_policy_revision` |
| `MEDULLA-DECISION-ENGINE-R811.8` | `crates/medulla-decision-outcome-types/src/decision/promotion.rs::tests::policy_evaluation_maps_to_held_out_evidence`; `crates/medulla-h-decide/src/decide/stat_tests/policy_evolution_tests.rs::capture_train_evaluate_and_promote` |
| `MEDULLA-DECISION-ENGINE-R036.103` | `crates/medulla-isolation/src/rbac_persist.rs::tests::reservation_blocks_writes_and_survives_reopen`; `crates/medulla-h-decide/src/decide/stat_tests/commit_admission_tests.rs::cross_owner_admission_journey` |
| `MEDULLA-DECISION-ENGINE-R036.104` | `crates/medulla-persistence-records/src/agent_library/admission_tests.rs::admission_participant_fences_and_replays`; `crates/medulla-h-decide/src/decide/stat_tests/commit_admission_tests.rs::cross_owner_admission_journey` |
| `MEDULLA-DECISION-ENGINE-R036.105` | `crates/medulla-raft-xshard/src/cross_shard_txn/certificate.rs::tests::certificate_binds_request_and_expires`; `commit_admission_tests.rs::cross_owner_admission_journey` |
| `MEDULLA-DECISION-ENGINE-R036.105.1` | `crates/medulla-persistence/src/redb_backend/xshard_certificate.rs::tests::certificate_seals_verifies_and_rejects_tamper` (L1); `crates/medulla-storage/src/owner/lineage_tests.rs::graph_shard_before_decision_certificates_is_a_frozen_predecessor` (L1); `crates/medulla-storage/src/owner/offline_upgrade/tests.rs::graph_shard_upgrade_creates_empty_decision_certificate_table` (L1); `crates/medulla-persistence-tests/src/backup_tests.rs::backup_census_includes_decision_certificates` (L1); `crates/medulla-persistence-tests/src/shard_migrate_tests.rs::shard_migration_carries_decision_certificates` (L1) |
| `MEDULLA-DECISION-ENGINE-R036.105.2` | `crates/medulla-raft-xshard/src/cross_shard_txn/certificate.rs::tests::certificate_binds_request_and_expires` (L1); `::tests::crash_between_decision_and_resolve_is_finished_on_restart` (L2); `crates/medulla-server-api/src/knob_config.rs::tests::decision_deadline_knob_bounds` (L1); `crates/medulla-h-decide/src/decide/stat_tests/commit_admission_tests.rs::cross_owner_admission_journey` (L3) |
| `MEDULLA-DECISION-ENGINE-R113.2.5` | `crates/medulla-persistence-records/src/decision_record/tests.rs::shared_commit_writes_record_and_evidence_in_one_transaction`; `::shared_commit_is_blocked_by_an_admission_fence`; `crates/medulla-h-decide/src/decide/stat_tests/commit_admission_tests.rs::topology_shared_commit_journey` (L3) |
| `MEDULLA-DECISION-ENGINE-R113.3.1.1` | `crates/medulla-persistence-records/src/decision_record/tests.rs::retry_allowance_table_issues_once_and_consumes_once`; `crates/medulla-storage/src/owner/lineage_tests.rs::agent_library_before_retry_allowances_is_a_frozen_predecessor`; `crates/medulla-storage/src/owner/offline_upgrade/tests.rs::agent_library_upgrade_creates_empty_retry_allowance_table` (all L1) |
| `MEDULLA-DECISION-ENGINE-R036.106` | `crates/medulla-isolation/src/rbac_persist.rs::tests::upgrade_preserves_rows_and_creates_empty_grant_table`; `commit_admission_tests.rs::cross_owner_admission_journey` |
| `MEDULLA-DECISION-ENGINE-R038.2` | `crates/medulla-h-decide/src/decide/stat_tests/tool_action_admission_tests.rs::untrusted_follow_up_is_denied`; `EvidenceClass::Observation` |
| `MEDULLA-DECISION-ENGINE-R038.3` | `crates/medulla-h-decide/src/decide/stat_tests/held_item_admission_tests.rs::program_needs_independent_verifier_and_passes` |
| `MEDULLA-DECISION-ENGINE-R039.3` | `crates/medulla-h-decide/src/decide/stat_tests/route_tests.rs::legal_preference_wins_and_illegal_is_recorded` |
| `MEDULLA-DECISION-ENGINE-R805.8` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::job_publishes_a_revision_without_moving_the_champion` |
| `MEDULLA-DECISION-ENGINE-R805.9` | `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::every_receipt_kind_resolves_to_held_out_evidence` |
| `MEDULLA-DECISION-ENGINE-R134.111.1` | `crates/medulla-decision-types/src/decision/task_plan/tests.rs::task_plan_round_trips`; `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::decide_routes_task_plan` |
| `MEDULLA-DECISION-ENGINE-R134.111.2` | `crates/medulla-decision-types/src/decision/task_plan/tests.rs::plan_respects_dependencies`; `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::decide_routes_task_plan` |
| `MEDULLA-DECISION-ENGINE-R134.111.3` | `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::decide_routes_task_plan`; `tests/decide/test_task_plan_served.py::test_explicit_plan_end_to_end` |
| `MEDULLA-DECISION-ENGINE-R134.111.4` | `crates/medulla-decision-types/src/decision/task_plan/tests.rs::invalid_graphs_refuse`; `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::invalid_task_requests_refuse` |
| `MEDULLA-DECISION-ENGINE-R134.111.5` | `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::task_plan_cannot_use_score_replay`; `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::decide_routes_task_plan` |
| `MEDULLA-DECISION-ENGINE-R134.111.6` | `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::maximum_bounded_plan_is_accepted` |
| `MEDULLA-DECISION-ENGINE-R134.111.7` | `crates/medulla-decision-types/src/decision/task_plan/tests.rs::the_canonical_task_plan_feature_schema_is_valid_and_stable`; `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::canonical_task_plan_schema_publishes_once_and_routes` |
| `MEDULLA-DECISION-ENGINE-R158.7` | `crates/medulla-h-decide/src/decide/stat_tests/run_adjustment_tests.rs::windows_drive_model_and_width_and_refuse_mid_run_widening` |
| `MEDULLA-DECISION-ENGINE-R179.7` | `crates/medulla-h-decide/src/decide/stat_tests/harness_contrast_tests.rs::contrast_reads_the_pinned_harness_evolution_knobs` |
| `MEDULLA-DECISION-ENGINE-R803.7` | `crates/medulla-h-decide/src/decide/stat_tests/harness_contrast_tests.rs::contrast_is_measured_from_stored_traces` |
| `MEDULLA-DECISION-ENGINE-R803.8` | `crates/medulla-h-decide/src/decide/stat_tests/harness_adoption_tests.rs::adoption_contrast_and_lesson_commit_together`; `tests/decide/test_harness_adoption_served.py::test_adoption_logged_and_lesson_retrieved` |
| `MEDULLA-DECISION-ENGINE-R805.10` | `crates/medulla-decision-outcome-types/src/decision/promotion.rs::tests::added_subject_kinds_round_trip_with_pointer_keys`; `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::promote_then_roll_back_every_subject_kind` |
| `MEDULLA-DECISION-ENGINE-R805.11` | `crates/medulla-decision-stats/src/decision/promotion.rs::tests::gate_rules_per_added_kind`; `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::promote_then_roll_back_every_subject_kind` |
| `MEDULLA-DECISION-ENGINE-R805.12` | `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::harness_definitions_promote_and_roll_back` |
| `MEDULLA-DECISION-ENGINE-R805.13` | `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::pointer_read_returns_champion_and_refuses_unknown_key` |
| `MEDULLA-DECISION-ENGINE-R815` | `crates/medulla-h-decide/src/decide/stat_tests/assemble_tests.rs::model_media_capability_and_unknown_code` |
| `MEDULLA-DECISION-ENGINE-R816` | `crates/medulla-h-graph/src/graph_ops/observation_state_tests.rs::state_is_run_bound` |
| `MEDULLA-DECISION-ENGINE-R817` | `tests/decide/test_revision_proposals_served.py::test_list_is_tenant_scoped_and_paged` |
| `MEDULLA-DECISION-ENGINE-R818` | `crates/medulla-h-decide/src/decide/stat_tests/probe_selection_tests.rs::selects_declared_entries_and_refuses_empty` |
| `MEDULLA-DECISION-ENGINE-R819` | `crates/medulla-h-decide/src/decide/stat_tests/serving_technique_tests.rs::verdict_follows_receipts_and_knobs` |
| `MEDULLA-DECISION-ENGINE-R820` | `crates/medulla-h-decide/src/decide/stat_tests/process_remediation_tests.rs::verdicts_follow_thresholds` |
| `MEDULLA-DECISION-ENGINE-R821` | `crates/medulla-h-decide/src/decide/stat_tests/retrieval_adoption_tests.rs::paired_run_adopts_and_refuses` |
| `MEDULLA-DECISION-ENGINE-R822` | `crates/medulla-h-decide/src/decide/stat_tests/calibration_tests.rs::isotonic_fit_is_monotone_and_promoted` |
| `MEDULLA-DECISION-ENGINE-R823` | `crates/medulla-h-decide/src/decide/stat_tests/reasoning_paradigm_tests.rs::legal_set_then_rank_and_abstain` |
| `MEDULLA-DECISION-ENGINE-R824` | `crates/medulla-h-decide/src/decide/stat_tests/capacity_tests.rs::capacity_proposal_verdicts` |
| `MEDULLA-DECISION-ENGINE-R825` | `crates/medulla-h-decide/src/decide/stat_tests/model_capacity_tests.rs::limit_narrows_on_latency_and_never_exceeds_ceiling` |
| `MEDULLA-DECISION-ENGINE-R826` | `crates/medulla-h-decide/src/decide/stat_tests/ingest_shortfall_tests.rs::allocation_ties_and_purge_ineligibility` |
| `MEDULLA-DECISION-ENGINE-R827` | `crates/medulla-h-decide/src/decide/stat_tests/revert_family_tests.rs::dependents_forbid_revert` |
| `MEDULLA-DECISION-ENGINE-R828` | `crates/medulla-h-decide/src/decide/stat_tests/resolve_conflict_tests.rs::winner_by_margin_tie_and_abstain_codes` |
| `MEDULLA-DECISION-ENGINE-R802.1` | `web_fetch_selection.rs::tests::question_round_trips` |
| `MEDULLA-DECISION-ENGINE-R810` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::program_optimize_job_end_to_end` |
| `MEDULLA-DECISION-ENGINE-R810.2` | `crates/medulla-program/tests/native_program.rs::cancel_stops_within_one_candidate` |
| `MEDULLA-DECISION-ENGINE-R810.4` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::program_job_refusals` |
| `MEDULLA-DECISION-ENGINE-R810.5` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::rerun_with_one_seed_gives_one_revision_digest` |
| `MEDULLA-DECISION-ENGINE-R810.6` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::served_program_job_journey` |
| `MEDULLA-DECISION-ENGINE-R811.2` | `crates/medulla-server-graph/src/dispatch/policy_evolution/gate/tests.rs::evaluation_overlapping_training_refuses` |
| `MEDULLA-DECISION-ENGINE-R811.4` | `crates/medulla-h-decide/src/decide/stat_tests/policy_evolution_tests.rs::policy_evolution_refusals` |
| `MEDULLA-DECISION-ENGINE-R811.5` | `crates/medulla-h-decide/src/decide/stat_tests/policy_evolution_tests.rs::receipts_read_back_field_for_field` |
| `MEDULLA-DECISION-ENGINE-R811.6` | `crates/medulla-h-decide/src/decide/stat_tests/policy_evolution_tests.rs::served_policy_evolution_journey` |
| `MEDULLA-DECISION-ENGINE-R812.2` | `crates/medulla-decision-stats/src/decision/promotion.rs::tests::challenger_win_loss_tie_and_judge_refusal` |
| `MEDULLA-DECISION-ENGINE-R813` | `crates/medulla-h-decide/src/decide/stat_tests/score_corpus_tests.rs::scores_build_a_corpus_and_compile` |
| `MEDULLA-DECISION-ENGINE-R813.1` | `crates/medulla-program/tests/native_program.rs::example_round_trips_per_grader_kind` |
| `MEDULLA-DECISION-ENGINE-R813.2` | `crates/medulla-program/src/corpus.rs::tests::each_grader_kind_and_the_skip` |
| `MEDULLA-DECISION-ENGINE-R813.4` | `crates/medulla-h-decide/src/decide/stat_tests/score_corpus_tests.rs::score_refusals` |
| `MEDULLA-DECISION-ENGINE-R813.5` | `crates/medulla-h-decide/src/decide/stat_tests/score_corpus_tests.rs::example_names_score_grader_author_and_run` |
| `MEDULLA-DECISION-ENGINE-R813.6` | `crates/medulla-h-decide/src/decide/stat_tests/score_corpus_tests.rs::served_scores_to_program_journey` |
| `MEDULLA-DECISION-ENGINE-R036.107` | `crates/medulla-h-decide/src/decide/stat_tests/assemble_tests.rs::allowlist_removes_tools_and_admission_still_runs` |
| `MEDULLA-DECISION-ENGINE-R804` | `crates/medulla-h-jobs/src/jobs/program_tests.rs::judge_only_test_split_refuses` |
| `MEDULLA-DECISION-ENGINE-R805.2` | `crates/medulla-decision-stats/src/decision/promotion.rs::tests::regression_triggers_rollback` |
| `MEDULLA-DECISION-ENGINE-R805.3` | `crates/medulla-h-decide/src/decide/stat_promotion.rs::qualified_champion` |
| `MEDULLA-DECISION-ENGINE-R805.4` | `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::promotion_refusals` |
| `MEDULLA-DECISION-ENGINE-R805.5` | `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::pointer_history_replays` |
| `MEDULLA-DECISION-ENGINE-R805.6` | `crates/medulla-h-decide/src/decide/stat_tests/promotion_tests.rs::served_promotion_journey` |
| `MEDULLA-DECISION-ENGINE-R805.7` | `crates/medulla-decision-outcome-types/src/decision/promotion.rs::tests::lifecycle_types_round_trip` |
| `MEDULLA-DECISION-ENGINE-R651.7` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::episodes_promote_to_a_linked_semantic_fact` |
| `MEDULLA-DECISION-ENGINE-R651.7` | `tests/memory/test_consolidate_served.py::test_episodes_promote_to_semantic_fact` |
| `MEDULLA-DECISION-ENGINE-R651.8` | `crates/medulla-graph-core/src/graph/txn_maintenance.rs::tests::reinforce_and_decay_share_importance_update` |
| `MEDULLA-DECISION-ENGINE-R651.8` | `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs::reinforce_then_decay_matches_the_half_life` |
| `MEDULLA-DECISION-ENGINE-R651.8` | `tests/memory/test_consolidate_served.py::test_reinforce_and_decay_share_importance` |
| `MEDULLA-DECISION-ENGINE-R651.9` | `crates/medulla-types/src/memory_scope.rs::tests::scope_never_exceeds_visible_sessions` |
| `MEDULLA-DECISION-ENGINE-R651.9` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::scope_narrows_recall_and_acl_still_applies` |
| `MEDULLA-DECISION-ENGINE-R651.10` | `crates/medulla-decision-stats/src/decision/participant_derive.rs::tests::targets_follow_flags_and_never_repeat` |
| `MEDULLA-DECISION-ENGINE-R651.10` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::observation_flags_decide_which_perspectives_exist` |
| `MEDULLA-DECISION-ENGINE-R651.11` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::turn_append_enqueues_one_ordered_job_per_key` |
| `MEDULLA-DECISION-ENGINE-R651.12` | `crates/medulla-h-graph/src/graph_ops/participant_memory_tests.rs::derive_job_writes_claims_with_turn_evidence` |
| `MEDULLA-DECISION-ENGINE-R651.12` | `tests/memory/test_participant_memory_served.py::test_turns_become_perspective_claims` |
| `MEDULLA-DECISION-ENGINE-R700.7` | `crates/medulla-h-decide/src/decide/stat_tests/belief_revision_tests.rs::contradiction_is_tracked_and_status_is_ordered` |
| `MEDULLA-DECISION-ENGINE-R134.111.11` | `tests/test_task_plan_receipt_validation.py::test_receipt_refuses_a_different_call` |
| `MEDULLA-DECISION-ENGINE-R134.111.13` | `tests/decide/test_task_plan_served.py::test_tool_step_bindings_end_to_end` |
| `MEDULLA-DECISION-ENGINE-R179.8` | `crates/medulla-decision-types/src/decision/knob_families/participant.rs::tests::knob_defaults_are_inside_bounds` |
| `MEDULLA-DECISION-ENGINE-R179.8` | `crates/medulla-h-decide/src/decide/stat_tests/knob_tests.rs::participant_knobs_are_registered_and_tunable` |
| `MEDULLA-DECISION-ENGINE-R803.8` | `stat_log.rs::dispatch` |
| `MEDULLA-DECISION-ENGINE-R814` | `tests/decide/test_prompt_candidates_served.py::test_dynamic_agent_gets_the_matching_connector_system_prompt` |
| `MEDULLA-DECISION-ENGINE-R814.1.2` | `crates/medulla-types/tests/prompt_candidate.rs::candidate_round_trips_and_kind_follows_role` |
| `MEDULLA-DECISION-ENGINE-R814.2` | `crates/medulla-compute/src/assemble/tests/prompt.rs::selection_scores_by_match_and_confidence_and_abstains` |
| `MEDULLA-DECISION-ENGINE-R814.2` | `crates/medulla-h-decide/src/decide/stat_tests/prompt_candidate_tests.rs::prompt_candidate_journey` |
| `MEDULLA-DECISION-ENGINE-R814.3` | `tests/decide/test_prompt_candidates_served.py::test_assemble_reads_candidates_from_imported_pack` |
| `MEDULLA-DECISION-ENGINE-R814.4` | `crates/medulla-h-decide/src/decide/stat_tests/prompt_candidate_tests.rs::unbound_prompt_refuses_and_rejected_connector_is_excluded` |
| `MEDULLA-DECISION-ENGINE-R814.5` | `tests/decide/test_prompt_candidates_served.py::test_record_names_prompt_connector_digest_and_capability` |
| `MEDULLA-DECISION-ENGINE-R814.7` | `tests/decide/test_prompt_candidates_served.py::test_system_prompt_query_lists_candidates_for_a_task` |
| `MEDULLA-DECISION-ENGINE-R814.8` | `tests/decide/test_prompt_candidates_served.py::test_non_canonical_prompt_is_refused_with_field_path` |
| `MEDULLA-DECISION-ENGINE-R822` | `crates/medulla-h-decide/src/decide/stat_calibration.rs::fit_isotonic` |
| `MEDULLA-DECISION-ENGINE-R828` | `crates/medulla-server-api/src/api/conflict_decider.rs::branch_merge_conflicts_route_through_the_conflict_decider_seam` |
| `MEDULLA-DECISION-ENGINE-R829` | `crates/medulla-h-decide/src/decide/stat_tests/connector_toolchoice_tests.rs::catalog_serves_connector_points_and_unknown_id_abstains` |
| `MEDULLA-DECISION-ENGINE-R829.1` | `crates/medulla-h-decide/src/decide/stat_tests/connector_toolchoice_tests.rs::catalog_serves_the_five_rank_points_and_unknown_id_abstains` and `crates/medulla-h-decide/src/decide/stat_tests/connector_toolchoice_tests.rs::more_than_two_thousand_candidates_are_refused` (L3). |
| `MEDULLA-DECISION-ENGINE-R829.2` | `crates/medulla-h-decide/src/decide/features/capability_text.rs::tests::name_match_counts_double_and_rare_terms_weigh_more` and `crates/medulla-h-decide/src/decide/features/capability_text.rs::tests::semantic_absent_when_embedding_model_not_ready` (L2). |
| `MEDULLA-DECISION-ENGINE-R829.3` | `tests/decide/test_capability_rank_served.py::test_named_system_and_object_rank_first_in_a_large_catalog` and `tests/decide/test_capability_rank_served.py::test_p99_ceiling_holds_on_a_warm_slot` (L4). |
| `MEDULLA-DECISION-ENGINE-R134.111.14` | `crates/medulla-decision-types/src/decision/task_plan/tests.rs::control_steps_round_trip_and_refuse_two_kinds` |
| `MEDULLA-DECISION-ENGINE-R134.111.15` | `crates/medulla-decision-types/src/decision/task_plan/tests.rs::control_checks_refuse_planted_faults` |
| `MEDULLA-DECISION-ENGINE-R134.111.16` | `crates/medulla-h-decide/src/decide/stat_tests/task_plan_tests.rs::control_refusals_name_the_task` |
| `MEDULLA-DECISION-ENGINE-R134.111.17` | `crates/medulla-decision-types/src/decision/task_plan/tests.rs::control_fields_change_the_digest` |
| `MEDULLA-DECISION-ENGINE-R134.111.18` | `tests/decide/test_task_plan_served.py::test_control_steps_served` |
| `MEDULLA-DECISION-ENGINE-R180` | `crates/medulla-h-decide/src/decide/stat_tests/process_decision_tests.rs::decide_routes_table_and_branch` |
| `MEDULLA-DECISION-ENGINE-R180.1` | `crates/medulla-decision-types/src/decision/process_decision.rs::tests::requests_round_trip` |
| `MEDULLA-DECISION-ENGINE-R180.2` | `crates/medulla-decision-stats/src/decision/dmn_eval.rs::tests::every_hit_policy_matches_its_fixture` |
| `MEDULLA-DECISION-ENGINE-R180.3` | `crates/medulla-decision-stats/src/decision/branch_select.rs::tests::branch_kinds_take_the_right_arms` |
| `MEDULLA-DECISION-ENGINE-R180.4` | `crates/medulla-h-decide/src/decide/stat_tests/process_decision_tests.rs::decide_routes_table_and_branch` |
| `MEDULLA-DECISION-ENGINE-R180.5` | `crates/medulla-h-decide/src/decide/stat_tests/process_decision_tests.rs::decision_refusals_name_the_rule` |
| `MEDULLA-DECISION-ENGINE-R180.6` | `crates/medulla-h-decide/src/decide/stat_tests/process_decision_tests.rs::premises_cite_rules_and_inputs` |
| `MEDULLA-DECISION-ENGINE-R180.7` | `tests/decide/test_process_decision_served.py::test_table_and_branch_served` |

## Explicit state machine rows (Article XXXI)

- `MEDULLA-DECISION-ENGINE-R830`: see each child (rollup).
- `MEDULLA-DECISION-ENGINE-R830.1`: `crates/medulla-decision-types/src/solve/solve_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-decision-types/src/solve/solve_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-decision-types/src/solve/solve_machine.rs::tests::no_writer_sets_status_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::solve_machine_matches_edge_table` (planned, L1): proves: Certificate/SolveStatus state machine: route 1 census findings through SolveStatus::apply.
- `MEDULLA-DECISION-ENGINE-R830.2`: `crates/medulla-decision-types/src/decision/comparative_analysis_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-decision-types/src/decision/comparative_analysis_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-decision-types/src/decision/comparative_analysis_machine.rs::tests::no_writer_sets_stage_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::comparative_analysis_machine_matches_edge_table` (planned, L1): proves: ComparativeStageBudget/ComparativeAnalysisStage state machine: route 1 census findings through ComparativeAnalysisStage::apply.
- `MEDULLA-DECISION-ENGINE-R830.3`: `crates/medulla-decision-types/src/decision/decision_job_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-decision-types/src/decision/decision_job_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-decision-types/src/decision/decision_job_machine.rs::tests::no_writer_sets_state_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::decision_job_machine_matches_edge_table` (planned, L1): proves: DecisionJobRecord/DecisionJobState state machine: route 1 census findings through DecisionJobState::apply.
- `MEDULLA-DECISION-ENGINE-R830.4`: `crates/medulla-decision-outcome-types/src/decision/edit_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-decision-outcome-types/src/decision/edit_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-decision-outcome-types/src/decision/edit_machine.rs::tests::no_writer_sets_status_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::edit_machine_matches_edge_table` (planned, L1): proves: EditFact/EditStatus state machine: route 1 census findings through EditStatus::apply.
- `MEDULLA-DECISION-ENGINE-R830.5`: `crates/medulla-types/src/gap_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-types/src/gap_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-types/src/gap_machine.rs::tests::no_writer_sets_state_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::gap_machine_matches_edge_table` (planned, L1): proves: Gap/GapState state machine: route 1 census findings through GapState::apply.
- `MEDULLA-DECISION-ENGINE-R830.6`: `crates/medulla-h-decide/src/decide/loop_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-h-decide/src/decide/loop_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-h-decide/src/decide/loop_machine.rs::tests::no_writer_sets_state_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::loop_machine_matches_edge_table` (planned, L1): proves: LoopInstance/LoopState state machine: route 13 census findings through LoopState::apply.
- `MEDULLA-DECISION-ENGINE-R830.7`: `crates/medulla-decision-outcome-types/src/plan_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-decision-outcome-types/src/plan_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-decision-outcome-types/src/plan_machine.rs::tests::no_writer_sets_status_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::plan_machine_matches_edge_table` (planned, L1): proves: PlanItem/PlanStatus state machine: route 1 census findings through PlanStatus::apply.
- `MEDULLA-DECISION-ENGINE-R830.8`: `crates/medulla-decision-types/src/decision/source_reliability_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-decision-types/src/decision/source_reliability_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-decision-types/src/decision/source_reliability_machine.rs::tests::no_writer_sets_state_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::source_reliability_machine_matches_edge_table` (planned, L1): proves: SourceReliabilityWire/SourceReliabilityState state machine: route 1 census findings through SourceReliabilityState::apply.
- `MEDULLA-DECISION-ENGINE-R830.9`: `crates/medulla-types/src/loop_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-types/src/loop_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-types/src/loop_machine.rs::tests::no_writer_sets_stage_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::loop_machine_matches_edge_table` (planned, L1): proves: StageBeacon/LoopStage state machine: route 1 census findings through LoopStage::apply.
- `MEDULLA-DECISION-ENGINE-R830.10`: `crates/medulla-decision-outcome-types/src/decision/evolution_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-decision-outcome-types/src/decision/evolution_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-decision-outcome-types/src/decision/evolution_machine.rs::tests::no_writer_sets_stage_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::evolution_machine_matches_edge_table` (planned, L1): proves: StageInput/EvolutionStage state machine: route 2 census findings through EvolutionStage::apply.
- `MEDULLA-DECISION-ENGINE-R830.11`: `crates/medulla-decision-outcome-types/src/policy_evolution/training_run_machine.rs::tests::every_legal_transition_applies`, `crates/medulla-decision-outcome-types/src/policy_evolution/training_run_machine.rs::tests::every_illegal_pair_is_refused_with_transition_refused`, `crates/medulla-decision-outcome-types/src/policy_evolution/training_run_machine.rs::tests::no_writer_sets_status_outside_apply`, `crates/medulla-rdf/tests/state_machine_vocabulary.rs::training_run_machine_matches_edge_table` (planned, L1): proves: TrainingRun/TrainingRunStatus state machine: route 1 census findings through TrainingRunStatus::apply.
