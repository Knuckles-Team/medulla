# MEDULLA-DECISION-ENGINE — Decision engine

Component: Decision engine

## Outcome

Medulla evaluates typed decisions from caller-visible facts. It records how each conclusion was reached and can independently verify a committed result. Connector pack annotations enter that decision pipeline as **claims**, not proof of a capability. Consumers can assemble an agent graph, ask a statistical question, and inspect a certificate and evidence. They do not rely on a private documentation service.

This specification owns Medulla's decision contracts, solver, evidence record and jobs. It defines the decision-facing semantics of published pack facts. Agent execution, connector discovery and ingestion orchestration are consumers. Their adapters may call these methods but cannot become a second authority for decision records or policy.

## Scope

Owner: `medulla` (native Rust). The decision crates are `crates/medulla-decision-types`, `crates/medulla-decision-outcome-types`, `crates/medulla-decision-policy`, `crates/medulla-decision-stats`, `crates/medulla-compute` and the handler crate `crates/medulla-h-decide`.

Non-goals: the engine never generates text, never holds model weights and never executes an action. Arbora owns admission and the served door. Meristem owns agent execution. Connector repositories own source calls.

Why a new spec: decisions, records and fitted heads form one owned capability. No other spec owns the ladder, the solver, the record or the head lifecycle.

## User Scenarios & Testing

### User Story 1 - Ask a typed question and get a proof-carrying answer (Priority: P1)

The user is a caller or operator of the decision engine. The outcome is a governed answer that the caller can audit.

**Acceptance scenarios**

1. **Given** an operator submits a typed question through `Decide` with tenant-visible candidates, **When** the ladder filters by tenant, applies policy and constraints, derives entailments, solves exactly and ranks statistically, **Then** the answer carries one resolution kind and one evidence class, or a typed abstention.

Independent test: the bound tests of `MEDULLA-DECISION-ENGINE-R001.1` run this story through a production entry point.

### User Story 2 - Assemble an agent graph by exact solve and commit it (Priority: P1)

The user is a caller or operator of the decision engine. The outcome is a governed answer that the caller can audit.

**Acceptance scenarios**

1. **Given** a caller asks `AgentAssemble` for a graph that covers required capabilities under a budget, **When** the engine solves a bounded 0-1 model and `DecisionCommit` re-derives and verifies the result, **Then** the committed record holds a certificate that a separate verifier reproduces byte for byte.

Independent test: the bound tests of `MEDULLA-DECISION-ENGINE-R004` run this story through a production entry point.

### User Story 3 - Rank options statistically and abstain when unsure (Priority: P1)

The user is a caller or operator of the decision engine. The outcome is a governed answer that the caller can audit.

**Acceptance scenarios**

1. **Given** a question has legal options and fitted heads, **When** `Decide` ranks only legal options with calibrated, conformal and drift-checked scores, **Then** the answer abstains when coverage, drift or sample gates fail and never widens the legal set.

Independent test: the bound tests of `MEDULLA-DECISION-ENGINE-R005` run this story through a production entry point.

### User Story 4 - Fit, evaluate, promote and roll back decision heads (Priority: P2)

The user is a caller or operator of the decision engine. The outcome is a governed answer that the caller can audit.

**Acceptance scenarios**

1. **Given** an evolution job fits a head from independently evaluated outcomes, **When** `DecisionEval` seals a held-out receipt and the guarded publish path promotes the head, **Then** one compare-and-set on the champion pointer promotes it and one rolls it back.

Independent test: the bound tests of `MEDULLA-DECISION-ENGINE-R040` run this story through a production entry point.

### User Story 5 - Record, retain and replay decisions (Priority: P2)

The user is a caller or operator of the decision engine. The outcome is a governed answer that the caller can audit.

**Acceptance scenarios**

1. **Given** a committed decision record exists, **When** an auditor reads, replays or logs it on any supported target, **Then** the replay is byte-identical and every record version stays decodable.

Independent test: the bound tests of `MEDULLA-DECISION-ENGINE-R007` run this story through a production entry point.

### User Story 6 - Reuse the ladder for retrieval, ingestion, routing and risk questions (Priority: P2)

The user is a caller or operator of the decision engine. The outcome is a governed answer that the caller can audit.

**Acceptance scenarios**

1. **Given** a consumer asks for a retrieval plan, an ingestion lane, a route or a risk score, **When** the matching question family runs the same ladder, **Then** the answer is an advisory proposal with evidence and never an unauthorized write.

Independent test: the bound tests of `MEDULLA-DECISION-ENGINE-R030` run this story through a production entry point.

### User Story 7 - Plan tasks, choose tools and bound context (Priority: P2)

The user is a caller or operator of the decision engine. The outcome is a governed answer that the caller can audit.

**Acceptance scenarios**

1. **Given** a planner asks how to execute a task, **When** the engine returns coverage, guardrails, tool subsets and certified context packages, **Then** the plan stays within the context budget and cites its sources.

Independent test: the bound tests of `MEDULLA-DECISION-ENGINE-R126.1` run this story through a production entry point.

### User Story 8 - Designate capabilities and select connector prompts (Priority: P3)

The user is a caller or operator of the decision engine. The outcome is a governed answer that the caller can audit.

**Acceptance scenarios**

1. **Given** a connector pack is published and a task needs a capability, **When** the capability index and prompt-candidate rows select a component by evidence, **Then** the designation is request-scoped, tenant-visible and explains each exclusion.

Independent test: the bound tests of `MEDULLA-DECISION-ENGINE-R172.1` run this story through a production entry point.

### User Story 9 - Meter usage and tune decision knobs (Priority: P3)

The user is a caller or operator of the decision engine. The outcome is a governed answer that the caller can audit.

**Acceptance scenarios**

1. **Given** an operator reviews cost, quotas and tunable values, **When** the usage ledger, price catalog and knob registry apply, **Then** each knob has a default, bounds and unit and a revision promotes only through the champion lifecycle.

Independent test: the bound tests of `MEDULLA-DECISION-ENGINE-R133.1` run this story through a production entry point.

### Edge Cases

- A candidate outside the caller tenant never reaches feature extraction or counts.
- An unknown cost, latency or capability fact stays unknown and never becomes zero or true.
- A stale catalog digest makes `DecisionCommit` fail closed.
- A record version the decoder does not know fails closed.
- A solver node budget that runs out returns a typed abstention with the best bound.
- Exploration is refused for policy, security, write-back and irreversible questions.
- A fitted head whose drift exceeds its bound abstains instead of ranking.

## Requirements

- **FR-001**: Resolution kinds and evidence classes on every answer. Rows: `MEDULLA-DECISION-ENGINE-R001`, `MEDULLA-DECISION-ENGINE-R002`, `MEDULLA-DECISION-ENGINE-R003`.
- **FR-002**: Exact 0-1 solver with an independently verifiable certificate. Rows: `MEDULLA-DECISION-ENGINE-R004`, `MEDULLA-DECISION-ENGINE-R049`, `MEDULLA-DECISION-ENGINE-R070`.
- **FR-003**: Statistical guarantees: conformal, calibration, off-policy gates. Rows: `MEDULLA-DECISION-ENGINE-R005`, `MEDULLA-DECISION-ENGINE-R006`, `MEDULLA-DECISION-ENGINE-R023`.
- **FR-004**: Tenant visibility before feature computation. Rows: `MEDULLA-DECISION-ENGINE-R013`, `MEDULLA-DECISION-ENGINE-R085`, `MEDULLA-DECISION-ENGINE-R087`.
- **FR-005**: Decision commit re-derives and verifies before writing. Rows: `MEDULLA-DECISION-ENGINE-R019`, `MEDULLA-DECISION-ENGINE-R051`, `MEDULLA-DECISION-ENGINE-R069`.
- **FR-006**: Record versioning, limits and retention. Rows: `MEDULLA-DECISION-ENGINE-R065`, `MEDULLA-DECISION-ENGINE-R066`, `MEDULLA-DECISION-ENGINE-R067`.
- **FR-007**: Deterministic replay across targets. Rows: `MEDULLA-DECISION-ENGINE-R007`, `MEDULLA-DECISION-ENGINE-R091`, `MEDULLA-DECISION-ENGINE-R119`.
- **FR-008**: Fitted-head lifecycle: fit, evaluate, promote, roll back. Rows: `MEDULLA-DECISION-ENGINE-R040`, `MEDULLA-DECISION-ENGINE-R060`, `MEDULLA-DECISION-ENGINE-R805`.
- **FR-009**: Question families reuse one ladder. Rows: `MEDULLA-DECISION-ENGINE-R029`, `MEDULLA-DECISION-ENGINE-R036`, `MEDULLA-DECISION-ENGINE-R046`.
- **FR-010**: One refusal taxonomy for decision outcomes. Rows: `MEDULLA-DECISION-ENGINE-R010`, `MEDULLA-DECISION-ENGINE-R010.100`, `MEDULLA-DECISION-ENGINE-R001.101`.
- **FR-011**: Task planning, guardrails and coverage facts. Rows: `MEDULLA-DECISION-ENGINE-R126`, `MEDULLA-DECISION-ENGINE-R127`, `MEDULLA-DECISION-ENGINE-R134`.
- **FR-012**: Context budget and certified package selection. Rows: `MEDULLA-DECISION-ENGINE-R137`, `MEDULLA-DECISION-ENGINE-R138`, `MEDULLA-DECISION-ENGINE-R139`.
- **FR-013**: Capability designation index. Rows: `MEDULLA-DECISION-ENGINE-R172`, `MEDULLA-DECISION-ENGINE-R650`, `MEDULLA-DECISION-ENGINE-R176`.
- **FR-014**: Connector prompts as system-prompt candidates. Rows: `MEDULLA-DECISION-ENGINE-R814`, `MEDULLA-DECISION-ENGINE-R177`, `MEDULLA-DECISION-ENGINE-R815`.
- **FR-015**: Usage ledger and price catalog. Rows: `MEDULLA-DECISION-ENGINE-R801`, `MEDULLA-DECISION-ENGINE-R802`, `MEDULLA-DECISION-ENGINE-R803`.
- **FR-016**: Evolution-tuned decision knobs. Rows: `MEDULLA-DECISION-ENGINE-R179`, `MEDULLA-DECISION-ENGINE-R178`, `MEDULLA-DECISION-ENGINE-R812`.

## Success Criteria

- **SC-001**: Every served answer names its resolution kind and evidence class. Rows: `MEDULLA-DECISION-ENGINE-R001`, `MEDULLA-DECISION-ENGINE-R002`.
- **SC-002**: A verifier recomputes every solver certificate with no difference. Rows: `MEDULLA-DECISION-ENGINE-R004`, `MEDULLA-DECISION-ENGINE-R049`.
- **SC-003**: A replay of a committed record yields identical bytes on each supported target. Rows: `MEDULLA-DECISION-ENGINE-R007`, `MEDULLA-DECISION-ENGINE-R091`.
- **SC-004**: No decision path reads a candidate outside the caller tenant. Rows: `MEDULLA-DECISION-ENGINE-R013`.
- **SC-005**: A promoted head always has a held-out receipt bound to its digest. Rows: `MEDULLA-DECISION-ENGINE-R040`, `MEDULLA-DECISION-ENGINE-R060`.
- **SC-006**: A rollback of a champion pointer is one compare-and-set. Rows: `MEDULLA-DECISION-ENGINE-R805`.
- **SC-007**: Decide meets its latency budget at 64 candidates and 32 capabilities. Rows: `MEDULLA-DECISION-ENGINE-R178`.
- **SC-008**: Each refusal code is unique and registered once. Rows: `MEDULLA-DECISION-ENGINE-R010.100`.

## Assumptions

- Arbora supplies a verified principal and tenant on every served request.
- The reference host runs the benchmark suite that measures each service-level budget.
- Pack-provided facts enter as claims until an independent evaluation raises them.
- Enforcement is server-side only: arbora admission and medulla policy. No client-side call-path gate exists (`MEDULLA-DECISION-ENGINE-R116.2`).

## Behavior detail

### Required behavior

1. **Decision ladder.** Filter by tenant and row visibility before feature extraction or counting. Apply policy, pins, denials and hard compatibility constraints. Derive native ontology entailments. Solve the legal set by bounded exact integer optimization. Use fitted statistics only to rank legal options or abstain. Every answer exposes `resolution_kind` (`constraint`, `entailment`, `optimization`, `statistical`, `abstention`) and `evidence_class` (`proof`, `observation`, `claim`). The conclusion takes the weakest premise class. The derivation itself can be proof even when a pack-provided capability is a claim. Statistics can never authorize a denied option. (MEDULLA-DECISION-ENGINE-R001–003, MEDULLA-DECISION-ENGINE-R009, MEDULLA-DECISION-ENGINE-R013, MEDULLA-DECISION-ENGINE-R015–020, MEDULLA-DECISION-ENGINE-R048)
2. **Assembly and solver.** `AgentAssemble` accepts typed tasks/capabilities and constraints. Medulla derives capability closure and returns one legal `AgentGraphDraft` or a typed `AbstainReason`. The 0–1 model covers capability coverage, implications, exactly-one slots, compatibility, knapsack budgets, optional template shape and cardinality. Optimize lexicographically with checked integer arithmetic, deterministic branching and a work-node budget. There is no arbitrary answer-count cap. A certificate names feasibility, objective, lower bound, gap, solver version and input digest; a separate verifier recomputes them. Unknown cost, latency or capability facts cannot silently become zero or true. Bounded why-not re-solves explain excluded visible options. (MEDULLA-DECISION-ENGINE-R004, MEDULLA-DECISION-ENGINE-R008, MEDULLA-DECISION-ENGINE-R010–014, MEDULLA-DECISION-ENGINE-R018–019, MEDULLA-DECISION-ENGINE-R036, MEDULLA-DECISION-ENGINE-R047–054, MEDULLA-DECISION-ENGINE-R065, MEDULLA-DECISION-ENGINE-R070)
3. **Decision record and commit.** Records pin the request, visible candidate facts and provenance, ontology/policy/catalog/head digests, quantized feature matrix when used, solver identity, eliminated constraints, derivations, result and digest. `DecisionCommit` re-derives the result, verifies the certificate, checks every pinned component revision and compares the candidate-universe catalog digest within one Agent Library transaction. It stores a `DecisionRecord` component under `decision:<digest>`; a byte-identical retry is idempotent, while stale catalogs or fabricated inputs fail closed. Agent graph synthesis evidence references the committed record. Record versions remain decodable; unknown versions fail closed. Library-sourced records persist; graph-sourced statistical summaries use RLS-routed graph writes and policy-set retention. Maximum record size is 64 KiB, 64 candidates and 32 required capabilities, with typed size abstention. (MEDULLA-DECISION-ENGINE-R007, MEDULLA-DECISION-ENGINE-R012, MEDULLA-DECISION-ENGINE-R017, MEDULLA-DECISION-ENGINE-R019, MEDULLA-DECISION-ENGINE-R047, MEDULLA-DECISION-ENGINE-R051, MEDULLA-DECISION-ENGINE-R058, MEDULLA-DECISION-ENGINE-R065–071, MEDULLA-DECISION-ENGINE-R095)
4. **Statistics and evaluation.** `Decide` shortlists only caller-visible candidates, computes versioned features, separates full-label from bandit-label data, and records exact propensities. Calibration, conformal coverage, drift and selective-risk gates can abstain. Off-policy estimates report effective sample size and refuse promotion below policy thresholds. Training uses only independently evaluated, sufficiently traced outcomes from approved commit principals; self-reported outcomes are claims. Exploration requires an explicit budget and server HMAC seed, and is unavailable for policy, security, write-back and irreversible actions. `DecisionFit` emits a draft blob; `DecisionEval` emits a digest-bound receipt; only the guarded component publish path promotes a fitted head. Fixed-point/soft-float replay must produce identical bytes on supported targets. (MEDULLA-DECISION-ENGINE-R005–007, MEDULLA-DECISION-ENGINE-R016, MEDULLA-DECISION-ENGINE-R020–028, MEDULLA-DECISION-ENGINE-R040, MEDULLA-DECISION-ENGINE-R053–062, MEDULLA-DECISION-ENGINE-R066–074, MEDULLA-DECISION-ENGINE-R082 et seq. (see `requirements.md`), MEDULLA-DECISION-ENGINE-R100, MEDULLA-DECISION-ENGINE-R103, MEDULLA-DECISION-ENGINE-R118, MEDULLA-DECISION-ENGINE-R119)
5. **Question families.** Reuse the same ladder for retrieval-plan choice, ingestion lane, enrichment scheduling, entity-resolution proposals, schema mapping, contradiction proposals, routing, risk, budget-aware option selection, connector event and tool proposals, A2A routing, context-bounded tool subsets, topology and finance recommendations. An advisory question returns a proposal and evidence only; execution and irreversible write-back still require their own deterministic authorization. `DecideText` has its own quote-aware, typed-parameter parser. (MEDULLA-DECISION-ENGINE-R029–045, MEDULLA-DECISION-ENGINE-R046, MEDULLA-DECISION-ENGINE-R062, MEDULLA-DECISION-ENGINE-R071, MEDULLA-DECISION-ENGINE-R100, MEDULLA-DECISION-ENGINE-R102, MEDULLA-DECISION-ENGINE-R103, MEDULLA-DECISION-ENGINE-R104 et seq. (see `requirements.md`), MEDULLA-FINANCE-PRIMITIVES-R009)
6. **Pack fact admission.** Decision candidates come only from published, tenant-visible component revisions. Pack-provided capability, modality, schema digest, price, latency, model and `contract_pin` facts carry claim provenance. Unknown foreign capability IRIs remain exact claim terms; malformed native capability IRIs are refused upstream. Withdrawn components are ineligible. A stale ontology/shapes projection cannot supply current facts. Packs cannot publish `DecisionRecord`, `DecisionHead` or `DecisionPolicy` kinds. Re-import of identical content cannot change the decision candidate digest. (MEDULLA-DECISION-ENGINE-R050, MEDULLA-DECISION-ENGINE-R061, MEDULLA-DECISION-ENGINE-R075, MEDULLA-TYPED-PACKS-R032, MEDULLA-TYPED-PACKS-R042 et seq. (see `typed-packs-and-catalog/requirements.md`), MEDULLA-TYPED-PACKS-R066)

## Learned source reputation and replay (MEDULLA-DECISION-ENGINE-R118, MEDULLA-DECISION-ENGINE-R119)

**MEDULLA-DECISION-ENGINE-R118.** Medulla derives a read-only, tenant-visible `reputation` relation from independently evaluated outcomes linked to approved decision commits. Use hierarchical Beta–Binomial option pools and proper-score forecaster records (Brier, log score, Murphy decomposition and calibration error), with a declared time-discount rule and minimum-sample gate. A sparse or unverified pool abstains; it cannot receive a fabricated neutral reliability. `AuthorityPolicy.source_reliability`, retrieval candidate reliability and UQL `SOURCE RELIABILITY` read the same versioned materialization. Visibility is applied before pooling, and an outcome supplied only by the candidate or its publisher is a claim, not an independent label. Evidence fusion belongs to `medulla-epistemic`; the finance signal method remains a typed adapter to that one kernel. A request-carried reliability weight cannot stand in for the governed materialization.

**MEDULLA-DECISION-ENGINE-R119.** `DecisionEval` replay takes `WalkForward(train,test,step,purge,embargo)`, `SharedCap(cap,Proportional)` and a policy-independent environment. Every fold reads a bitemporal `AS OF` snapshot, refits on training history only, and records input/head/policy digests, option contributions and a resumable checkpoint. The same sealed, content-addressed `EvaluationRun` records the exact trial count used by deflated Sharpe, split-derived PBO and Diebold–Mariano. It compares those metrics against a pinned incumbent. A superseding run references the prior digest without rewriting history. Finance `BacktestRun` is a specialization of the shared seal, verification and no-look-ahead rules. A policy-dependent environment must refuse replay and use the separately identified off-policy evaluation path. `DecideText` parses `REPLAY WALK FORWARD` through its typed lexer; parser success alone cannot authorize a job or publish a fitted head.

## Task-plan facts (MEDULLA-DECISION-ENGINE-R126, MEDULLA-DECISION-ENGINE-R127)

An Meristem task planner answers "How can I execute task X?" as one plan. Medulla supplies the facts behind that plan; the planner only composes them. `AgentAssemble` (MEDULLA-DECISION-ENGINE-R036) remains the authority for agent composition. The coverage query (MEDULLA-DECISION-ENGINE-R126) names Agent Library templates and A2A agents that already serve each capability. The guardrail query (MEDULLA-DECISION-ENGINE-R127) names the rules each task class must follow. Both answers carry premises, so the planner cites ontology facts for every plan element. Neither answer authorizes execution. The consumer contract is MERISTEM-CONTROL-R027 in [Meristem](https://github.com/Knuckles-Team/agent-utilities/tree/main/specs/agent-control-plane).

## Certified context packages (R137.7–R137.12)

The engine owns certified multi-resolution context-package selection under the tightest applicable window, reserved-output, caller-token, monetary and latency constraints. It preserves exact tokenizer weights, net-value optimization, fewer-token ties and cited source/range/revision identity. MERISTEM-CONTEXT-R004 consumes the typed selection without reranking. Full-prompt fit and the existing recursive decomposition and parallel-map obligations remain separate and unchanged. These additive definitions do not claim a served package handler or runtime verification.

## Comparative analysis plan (MEDULLA-DECISION-ENGINE-R139)

Medulla produces a read-only, revision-pinned `ComparativeAnalysisPlan` through public `Method::Decide`. It reuses caller-supplied query-vector and completed `UnifiedQuery` results for zero-model vector discovery. It selects normalized-importance `>= 0.50` candidates for one or two Meristem synthesis calls. It selects only `>= 0.80` candidates for budgeted deep extraction. The versioned thresholds, candidate order/universe, query and space revisions, and each stage and total call ceiling are bound into the plan. The existing 64-candidate decision limit applies.

Medulla returns a typed refusal before exposing a plan for stale or malformed inputs or impossible budgets. Medulla never generates text or invokes a model. Meristem consumes the plan and calls its configured service only afterward at `agent_utilities/knowledge_graph/retrieval/context_compiler_serving.py::compiled_chat_completion`. Meristem owns a separate invocation receipt bound to the plan digest. `MERISTEM-HARNESS-R024.3` trace digests are not a comparative-analysis consumer contract. This Medulla-only slice does not add a Meristem requirement or runtime receipt; the served Meristem consumer remains a dependency hold. R080.1 and R136.3 are adjacent mechanisms only. These definitions remain SPECIFIED until both Medulla and Meristem served receipts exist.
