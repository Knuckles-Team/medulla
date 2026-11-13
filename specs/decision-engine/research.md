# decision-engine — Research

## Decision: Medulla owns the embedding set in prepare

**Decision.** Medulla owns the embedding set built in source-ingest `prepare` (`MEDULLA-DECISION-ENGINE-R079.6`) and builds it now; the connector column-profile row is filed in the connector SDK repository.



## Improvement loops (R653)

- **One loop is two instances.** The loop state is an instance of the existing loop chart. The lease, gaps,
  offers, beacons, ledger, patterns, ratchet comparisons and transition history are one `LoopView` held by
  an instance of the one-state `loop_record` chart, written by one `write` self-loop. The loop chart is not
  widened, so its own tests and its reuse stay valid. Every op answers the `LoopView`.
- **Claim creates.** `LoopOp` has no create op, so the first `Claim` of an unknown loop creates it from the
  `objective_digest` and `priority` it carries. Later claims ignore both.
- **Release keeps the epoch.** A release fires `lease_lost` (running to orphaned) and ends the lease at the
  release time. The epoch stays so the next claim raises it. A holder acts only while the loop is in an active
  state, so a released epoch is fenced even though it still matches.
- **Expiry reclaim** fires `lease_lost` then `claim`, so an expired in-flight loop is taken over through the
  chart and the transition history shows both events.
- **Mine runs inline.** The miner is a pure computation bounded by three knobs; a separate job adds a state
  machine and a store for no gain.
- **Extra refusals.** `LOOP_NOT_CLAIMABLE` (finished or paused loop) and `GAP_NOT_OFFERED` (settle without an
  offer) close the cases the seven listed codes leave open.
- **Queue order has no served consumer yet.** `LoopQueue` fixes the total scheduling order; a claim-next op
  that enumerates loops needs its own row.

## Semantic enrichment (R614.2)

- The behaviours are pure functions in the enrichment crate with the three knobs as range-checked
  parameters; the served `SemanticEnrich` method (R614.3) is their only caller.
- A card node id derives from the subject and the prompt digest, so replacement is a plain write to the same id.
- Classification refuses a vector whose dimension differs from a centroid (`VECTOR_DIMENSION_MISMATCH`)
  rather than comparing a prefix.

## Family answers and utterance binding (R001.101.3, R001.101.5)

Decision: a family's typed answer is one closed `FamilyAnswer` enum carried as
the record's single `answer` field, so new families add a variant and never a
record field; the task plan moved into it (wire field `task_plan` -> `answer`).
Per-kind behaviour that a stage needs is a `DecisionFamily` hook
(`binds_utterance`), never a `QuestionKind` comparison in a stage file.

## Primitive contract source and graph belief slices (R036.100.1, R088.1)

- A primitive contract is read from component attributes, not a new entry field: the entry already carries a closed attribute map, versioning and a definition digest, so the contract is bound by the same digest and a component that declares none stays legal (`primitive: None` skips the checks).
- A graph belief slice appends `BELIEF AS OF t` to the candidate plan instead of leading with it: over empty input the operator yields every live row, which could add options; appended, it only narrows the candidate rows. A slice with any candidate absent at `t` is an unknown fact, never a smaller option set.

## Topology admission (R046.750, R046.751)

The plan carries no depth, so `admit_topology` takes the template's depth beside the plan. The cap knobs
(`topology.max_width`, `topology.max_depth`, `topology.max_rounds`) live in the decision knob registry
(`knob_families/topology.rs`) and only tighten the topology policy's and the request's caps; the solver and the
admission therefore hold the plan to one set of caps. A breach after solving abstains the record with the refusal
code, never a plan. The hot-path budget rides the existing `cargo xtask decide-budgets` check through
`group_budgets`, not a second script; the admit group is `decide/topology_admit` so the served family group of
R111.1 keeps the name `decide/topology`.

## Standalone topology question (R111.1)

`Decide` of kind `Topology` is the assembly's topology question asked of one template with no agent variables.
`medulla_compute::assemble::decide_topology` shares the posing rule (`pose_for`), the rows (`Question::extend`), the
solver and the cap admission with `AgentAssemble`, so the two cannot disagree on a plan. Decisions:

- The question is one typed parameter, `topology`, naming one template. Choosing among templates stays with
  `AgentAssemble`; a flat choice would otherwise duplicate its enumeration.
- Schema entailments and capacity headroom are observations of the engine, not caller claims: the server fills
  `TopologyQuestion::read` before deciding and refuses a caller-supplied value. The filled parameter is part of the
  sealed inputs, so a replay reads the same facts.
- The record is a claim, not a proof: the weakest premise is the publisher's topology claim (and the headroom is an
  observation), exactly as for `AgentAssemble` (`assemble/tests/topology.rs`). The solver certificate is what makes the
  resolution kind `Optimization`.
- An unreadable schema or graph is `TOPOLOGY_SCHEMA_UNAVAILABLE`; a template that cannot answer is an abstention
  with `AbstainReason::Infeasible` or `BudgetExhausted`.

## Run credit (R100.102)

`DecisionLog` `Learn` with a `RunOutcome` credits a delegated run. Decisions:

- Cited rows count as positives and uncited returned rows as hard negatives only when the run succeeded. A failed
  run's citations are not evidence of relevance (the same rule as the retrieval verdict: nothing is learned from a
  run that was not a success), but its steps are still credited, because the reward is the caller's own judgement of
  each step.
- A step's `baseline_revision` is the revision of the baseline its advantage was measured against, read before the
  update; a capability with no stored baseline reads revision 0.
- Baseline revisions are stored with an inverted revision (`u32::MAX - revision`) in the key, so the head is one page
  read however many revisions exist. The receipt and every revision commit in one atomic batch; a writer that lost a
  concurrent revision retries once against the new head.
- The receipt carries `outcome_digest` so a replay compares content without a second row, and its evidence premises.
- Feedback events for the document-boost store (federated-query R705.3) are derived from the stored run receipts by
  that store, not appended by `record_run`: the run write stays one atomic batch inside the decision artifact store
  and the decision-engine handler takes no dependency on the retrieval crate.
- Profile-use grants (R036.106) are rows in their own owner table, not fields of the authorization image, so a grant
  never changes the policy or identity digests a reservation records. A grant has a lifecycle (active, revoked), so it
  is the explicit `ProfileGrantMachine`: one persisted `state` field, one event `Revoke`, a second revoke refused with a
  typed `TRANSITION_REFUSED`; the change time is `state_changed_at_ms`, never a nullable `revoked_at_ms`. A new grant is
  refused with `RBAC_RESERVED` while a reservation holds the image, because granting changes who may use a profile.
- The RBAC layout upgrade is one row of the shared offline upgrade registry (`RBAC_BEFORE_PROFILE_GRANTS`) rather than a
  second private migration path, so the operator `store-upgrade` command applies it with the same inspection and
  byte-fingerprint rechecks as every other owner file. An ordinary `RbacStore::open` of a predecessor refuses with
  `RBAC_LAYOUT_OUTDATED` and writes nothing.

## Coordinator decision certificate and the shared commit path (R036.105.1, R036.105.2, R113.2.5, R113.3.1.1)

- **Own table, not a reserved key.** The certificate is stored in `xshard_decision_certificate`, a table of its own with its own storage lineage row and an offline-upgrade registry entry. A reserved key under `XSHARD_PREPARE` would leak into the prepare scan, the backup census and shard migration, which all treat that table's keys as `(txn_id, group_id)` prepares. The new table is added to the backup census and the migration copy explicitly instead.
- **Legacy rows never count.** A terminal `xshard_decision` value without a certificate is legacy evidence. Recovery resolves it as abort and refuses with `CERTIFICATE_LEGACY_ROW`; the upgrade does not invent certificates.
- **The deadline is a registry knob.** `xshard.decision_deadline_ms` (default 30 s, range 1 s to 10 min, tunable by the operator only through the startup knob loader) bounds how long a prepared reservation may wait for its COMMIT. An expired COMMIT records ABORT with the certificate; an ABORT never expires. The storage writer thread sets `decided_ms` so the clock that decides expiry is the one that serialises the write.
- **One coordinator decision for every owner.** The Agent Library, the capacity ledger and RBAC are separate owner files, so none can commit the others. The durable coordinator decision is the single commit point; each owner resolves from it, and recovery finishes any owner that stopped between the decision and its own write.
- **Evidence and record share an owner.** The decision record and the graph evidence are both Agent Library rows, so they commit in one owner transaction; only the capacity lease is a second participant. This keeps the participant count at the minimum and avoids a second capacity authority.
- **Retry allowance is a table, not a counter.** The tombstone row is retained so a restart never restores the allowance; it is an allowance row, not a decision record, which keeps R113.2.3's rule that a denial leaves no decision record.

## Thought search wiring (R750.750 to R750.754)

- **Knobs are decision knobs.** The five `thought_search.*` knobs are `KnobSpec` rows in the decision knob registry, read from the pinned policy with `DecisionPolicy::knob` and recorded as `knob:` premises, like the harness-contrast knobs. They are not server startup knobs: the server loader's own rule is that decision knobs live only in the decision registry, and evolution tunes them through knob-set revisions.
- **Candidates are the leaf ids.** `Decide` needs a candidate source; the family declares the frontier leaves as options so the record's candidate digest covers exactly what was ranked. An empty frontier is refused, because a declared source names at least one option.
- **A budget stop abstains.** The answer still carries `Stop{certified_gap}`; the outcome is `Abstained` with `BudgetExhausted` naming the best node, resolution `Optimization`, evidence `Claim` (scores are caller claims).
- **A missing strategy is unknown.** A task class absent from `DecisionPolicy.reasoning` refuses `THOUGHT_STRATEGY_UNKNOWN`; there is no silent default strategy.
## Drift quality, staleness and region selection (R174.8.2.1)

## Drift quality, staleness and region selection (R174.8.2.1)

- **Quality decay is a rolling MRR against an activation baseline.** The monitor keeps the last `drift.quality_window` reciprocal ranks of cited outcomes and compares their mean with the MRR measured at activation. A decay above `drift.quality_decay_pct` percent fires after two consecutive evaluations, like every other signal. With no baseline or no outcome the signal is simply absent.
- **Staleness is the `stale_index` rule, declared once.** The lag of the graph version over the version the index is embedded through is compared with `discover.max_version_lag`. The knob is declared beside the monitor; the discover quality verdict (R089.2) reads the same knob rather than declaring a second one.
- **The changed-node bitmap is run-length compressed over revisions.** Marks keep sorted, disjoint, non-adjacent inclusive runs, so memory grows with the number of contiguous ranges, not with writes. A plan names only firing shards; a shard with changes but no firing signal contributes nothing. Beyond 4096 regions the smallest gaps are joined first, so the plan stays a superset of every change and never drops one.
## Trajectory decisions (R140.100 to R140.103)

- **Pure decisions, caller-supplied facts.** Each decision is a pure function in `decision/task_plan/`: knobs, the clock, the stored outcome records and the caller tenant are arguments. The served route (R140.104) reads the policy knobs, the clock and `LogReader` and passes them in, so the unit proofs need no store.
- **Knobs are policy knobs.** `trajectory.*`, `breadth.*` and `pairwise.*` are declared in `knob_families/trajectory.rs` and registered in the decision registry; a value outside the declared range is held to the nearest bound when read.
- **The initial breadth is clamped like every result.** `breadth.initial` has range [1, 16] and `breadth.max` may be set lower; clamping to `[1, breadth.max]` keeps the invariant that no decision exceeds the maximum.
- **The previous breadth is part of a present history.** The decision digest covers it, so the request carries it beside the cited attempts, and the engine never infers it from a stored outcome.
- **A refusal never degrades to a default.** A missing, foreign or moved outcome refuses; it is never read as a zero outcome. Duplicate pairs in a pairwise ranking are accepted only when they are the same record.

## Where `SemanticEnrich` runs and what counts as a schedule (R614.3)

- The handler is a graph-scoped route in `medulla-server-graph` (`graph_pipeline/semantic_enrichment.rs`), reached from `route_web_ingest` beside `EnrichDocument`. It is not in `medulla-h-retrieve`: commit authority (the placement fence and the ChangeEnvelope applier) lives in server-graph, and h-retrieve depends on neither ingest nor enrichment. One path commits.
- A schedule decision counts when it is a committed `DecisionLogEntry` of question kind `EnrichmentSchedule`, visible to the caller (tenant-wide, or committed by the caller), whose executed option (acted or explored) is exactly `semantic:<op>:<subject>`. An advisory or abstained decision executes nothing and schedules nothing. The check runs first, so an unscheduled call reads no graph data.
- Classify reads the Document vector from the semantic store, or embeds the Document text off the request lock; topic centroids are the stored vectors of `Topic` nodes of the requested taxonomy. Project reads the visible nodes of the label in id order, at most `enrichment.projection_batch`, and reports `more`.
- **Context-budget relevance and token cost (R029.2.2).** A source's relevance is the pinned head's score (the softmax probability of a listwise head, else the logit) read through a `refine` family hook after the ladder; `context.tokens` is a whole token count carried on Q32. The partition replaces the ladder outcome with advisory scores of the selected sources and seals one `context_source:<id>=<selected or reason>` claim premise per source. A named source that is not visible is `unauthorized` under the caller's own string; with no head reading the ladder abstention stands.

## Family premises computed at conclude time (R136.5, R136.6, R137.4, R137.5)
A family whose premises need what only its conclude stage reads (a model-profile lookup, the tallies of a round, the cost of an allocation) returns them on `Executed.family_premises` as `FamilyPremise` values; its `premises` hook seals them. A premise sourced from the policy is bound to the digest of the policy in force at seal time, so a family never needs the digest while it concludes. One mechanism serves every such family; no family reads storage a second time to rebuild its premises.

Over-window refusals use the registered code `CONTEXT_BUDGET_TOO_MANY_ITEMS` (the row text previously named an unregistered `CONTEXT_TOO_MANY_ITEMS`). A round with no declared fixes and no regressions abstains through `AbstainReason::Family` with `ROUND_NO_PREDICTION`; allocation refusals keep the `PARAMETER_INVALID` wire error naming the registered `ALLOCATION_*` code and its detail. All three codes are registered in `codes.rs` family tables for their question kinds.

## One `refine` hook, one `premises` signature, and two context questions (DE-REFINE-HOOK)
`DecisionFamily::refine` takes the request (`None` when a logged record is replayed), the pinned head, the visible candidate views, the policy digest and the executed answer, and returns the refined answer. It runs after both a family's own `conclude` and the shared ladder, so the rank family (calibrated ordering) and the source-selection family (token-budget partition) share it. `premises` takes the request, the executed answer and the policy digest; a family seals either what it computed at conclude time (`Executed.family_premises`) or what its refinement left on the executed answer (`Executed.context_selection`) through that one hook. On replay the source-selection family keeps the stored answer, because a replay has neither the request nor the views.

Two features had claimed the `context_budget` kind with different questions: which sources fit a token budget (R029.2.2, candidates plus `context.token_budget`) and how to split a prompt longer than the model window (R137.3 to R137.6, `prompt_tokens` and one `plan` option). A hook that validates declared options has no request to read, so one kind cannot tell the two apart without guessing from option names. The kinds are therefore separate: `QuestionKind::ContextBudget` keeps source selection and `QuestionKind::OverWindowPlan` (`over_window_plan`) carries the over-window plan, with its own family file and test file. The R137.3 to R137.6 rows name those paths.

## Instruction disclosure reads versions from the named graph (R137.100)
Versions are tenant graph nodes `instruction-version:<set>:<version>` carrying the engine-owned marker `_instruction_version` and their own `tenant_id`; the family enumerates a set's nodes by that id prefix in the caller's row-level-filtered view (the `graph` param), strictly decodes them, and drops any node that is not a well-formed version of the caller's tenant. A set with no visible version refuses `DISCLOSURE_UNAUTHORIZED` whether it is missing or another tenant's, so the answer leaks no existence. The latest version is the highest `version` among the versions the phase allows; a pinned digest is checked against the version selected for its set, and a pin on a set that selected nothing is ignored because nothing is being disclosed. `occurred_triggers` on an `initial` question is a parameter error. `run_id` and `sequence` are recorded as a Claim premise; they do not change the selection. The one empty case (a runtime question whose triggers released nothing) answers an empty advisory, which is the withheld state. Premises sealed from the answer use the `Observation` provenance naming the version node.

## Scoped adapter scope membership and monitor trigger (R174.7.3)
Decision: a judged retrieval belongs to a corpus scope when its recorded plan template anchors on that corpus class, and to a capability scope when its template's task class is that capability; the graph scope covers all. A scope that no judged retrieval of the tenant belongs to is refused `ADAPTER_SCOPE_UNKNOWN`. The monitor is one governed learning write (`MonitorAdapter`) that reads the scope's active adapter and the outcomes judged in a window, and moves the pointer back itself when the Wilson upper bound of the win rate falls below one half, so rollback has one writer and one receipt. A refit's `parent_digest` is the scope's active adapter and its `revision` the parent's plus one.

## Index-side adapters (R174.7.3.2)
Decision: the transform `W = I + sum(g_i u_i u_i^T)` is symmetric, so `q . (W d) = (W q) . d`: an index-side adapter is the same fit as the query-side one, evaluated by the same held-out comparison, and its receipt records `side = index`. The side is a field of the adapter body (left out of the canonical bytes when it is `query`, so existing digests hold), because the stored adapter is keyed by body digest and two sides of equal directions would otherwise overwrite one receipt. An index adapter is materialized, never pointer-activated: the generation cycle applies it to the active generation's stored vectors, the existing generation receipt qualifies the shadow, and the generation pointer move is the activation. The cycle then queries the shadow with the base query. A node the active generation does not embed (or the caller cannot see) is left unembedded, which the receipt's embedded-count rule rejects if it leaves the shadow smaller than the active generation. Background capacity for such a cycle is CPU, not an embedding endpoint.

## Robust transfer across backbones (R059.1.1 to R059.1.3)

- **The skill identity rides on the binding.** `BackboneBinding` carries `skill_key` beside the backbone, task distribution, dataset and metric, so one evaluation names everything it was run under and the read groups by backbone within the requested skill. A malformed binding (empty id or revision, a digest that is not 64 lowercase hex characters) is refused when the evaluation is joined, never stored.
- **No layout change.** The `backbone` field is omitted from a stored evaluation when absent, so every existing stored evaluation keeps its bytes and its digest; the evaluation artifact is a self-describing row, not a table layout, so no offline upgrade step applies.
- **Reuse of the one label rule.** `robust_transfer` reads `label_use` from the aggregate (independent, uncensored, at or above the fidelity floor, not a claim); there is no second rule. Group rates use the same Beta-Binomial pooling as the aggregate, under one root of backbone leaves.
- **Refusals are registered codes.** `BACKBONE_MISSING`, `BACKBONE_DUPLICATE`, `TRANSFER_TOO_FEW_BACKBONES` and `TRANSFER_SAMPLES_BELOW_MIN` live in `transfer::REFUSALS` and join the code-uniqueness test; the served read renders them as `CODE: detail`.
- **Knobs are registry knobs.** `transfer.min_backbones` and `transfer.min_samples` are declared in `knob_config.rs` with `knob-table.md` rows; the handler reads the resolved values and passes them to the kernel as parameters.
- **Premises belong to the estimate.** The aggregate read has no decision record, so `TransferEstimate` carries `evidence_class` (the weakest label class) and its own `premises`: `transfer.robust_rate` and `transfer.min_backbone` at that class, plus one Definition premise per knob bound to a digest of the resolved knob values.
- **Skill candidates share the transfer read (R059.2).** `skill_candidates` is computed on every `DecisionLog` aggregate from the same joined rows and the one `label_use` rule: an option under one question qualifies at `transfer.skill_after_successes` independent successful labels from at least `transfer.min_rollouts` distinct records. Counting evaluations and records separately is what makes one record judged by several evaluators count once towards the rollout floor. The knobs sit beside the other `transfer.*` registry knobs, because the aggregate read takes no pinned policy; when DE-R059.1-KNOBS moves the `transfer.*` knobs into policy knob families, all four move together. Each row carries its own `premise` (the aggregate has no decision record).
- **Not built in this item.** R121.100 would register `QuestionKind::StepVerification` and a family at the paths the parked step-verification family already uses (spec/fcontinue-w1-decision-engine-5, R753.755), and R118.1.2.4 waits on the wire work of R118.1.2.2/.3.

## R044.2.2: where the served A2A routing family reads graphs and keeps its refusals

The family reads the request's `AgentLibrary` scope through `library_candidates`, so the tenant binding and visibility filter are the ones every library question uses; a graph published by another tenant is never a known id. The refusal table is declared in `medulla-decision-types` (`a2a_route.rs`) because `codes::FAMILY_TABLES` lives there and `medulla-decision-outcome-types` depends on it; the routing function `A2ARoutingResolution::route` stays in the outcome-types crate and the family calls it. The matched graph is sealed as a `Publisher` premise with the graph's content digest; an abstention carries `Claim` evidence and the default abstention resolution.
## Ordering usage guidance steps (R177.2)
Decision: `:precedes` is a hard constraint and a success rate only chooses among steps that are ready at the same time. Ready steps are scanned in tool-id order, so the pairwise rule (rate when both tools have `usage.min_trials` outcomes, otherwise tool id) is applied the same way for any input order. Why: a pairwise rule that mixes a rate with an id fallback is not transitive, so a fixed scan order is what keeps the answer reproducible.

## Research walk: what counts against the node budget and the value floor (R705.2)
Decision: the node budget counts sources read, so the result count never bounds the walk. The floor is tested on the best queued expected value before a read, so a source at or below the floor is never read and ends the walk. Links are queued once per source (a source reached by two paths keeps the shallower first reach), and a source whose depth would exceed `research.depth_limit` is skipped, not queued. Why: the walk is bounded by work done, not by what it returns, and the heap makes the best remaining value the only value the stop rule needs.

## Matching: what the entropy guard removes (R707.2)
Decision: a name at or above `match.entropy_mbit` contributes no token similarity, so two features that share only such a name score at most the label and embedding weights and fall below `match.score_min` unless the embeddings agree. Entropy is computed with an integer base-2 logarithm (Q20), so the guard is the same on every platform and needs no float transcendental. Blocks are filled in feature id order so the cap is reproducible; the count of features past the cap is returned, not hidden.
## PromQL series functions and the golden equivalence test (R117.1.1)

- PromQL series functions are named `medulla_<func>`, the same spelling the SQL window functions use, one per `SIGNATURES` entry; a PromQL name that merely matches a bare kernel name (`abs`, `lag`) never shadows a built-in PromQL function.
- A call takes its range vectors first (one or two, per the signature) and then numeric parameters; the planner compiles it to the shared `Program` and holds no evaluator of its own. Each series' points inside the range window are fed in timestamp order and the sample is the value at the last point (none while warming up). Two-series calls pair series whose labels agree ignoring the metric name, on the timestamps both carry. A window that holds the whole history therefore equals the UQL and SQL whole-history result.
- The reference vectors are not copied: the test reads the existing pandas reference fixture and a small manifest names the functions, parameters and reference case; `div` is checked against the `x / y` oracle (CONTRACT-R004, FIN-R013.2.6: no committed generated artifacts).
- `medulla-tsdb`'s `promql` feature now implies `derive`, because the series functions run the derive program.

## R118.1.2.3: how the Query readers read the governed reliability

- One resolver: `medulla_exec::exec::reliability::resolve` (graph propagated belief as prior, the caller's visible decision log as the estimate). The `SOURCE RELIABILITY` stage, `EpistemicStatus` and `RankByProvenance` all call it; none reads a number the caller supplies.
- `RetrievalCandidateWire` is request-only, so it carries the candidate's `source_id` and the engine resolves it. The typed `SourceReliability` rides the result (`RankedResultWire.source_reliability`, `AuthorityPolicyWire.source_reliability`). A request with the old scalar is refused at decode.
- An abstaining source (`InsufficientHistory`) feeds `None` into the epistemic crate's `RetrievalCandidate.source_reliability`; the reliability component then counts 0.0, the same honest "unknown" an absent calibration gets. No prior is substituted.
- `EpistemicStatus` applies only an `Estimated` mean as the policy's reliability multiplier; `Absent` keeps the default multiplier (the node's own propagated belief is already the prior) and `InsufficientHistory` applies nothing.
- `RankByProvenance` now reads a graph snapshot for priors, so it moved from the pure-compute exceptions to `RLS_ROUTED` and the handler `rls.filter_view`s the snapshot first.
- The decision log is bound from the verified carrier only (`served_reputation_source`, shared with the plan leg); without a carrier or decide handler the reader answers `Absent`.
- Generated Python models are not regenerated by hand (CONTRACT-R004: no committed generated artifacts).
## Capability designation index: design calls (R172.2 to R172.6)

- Question weights and knobs. A question carries its own `weights` (cosine, reward, prior). The knobs `designation.reward_weight` and `designation.prior_weight` are the ceilings a question's reward and prior weight is clamped to, so a caller can never weigh learned reward or the ontology prior above the registry bound. Both knobs default to 0.15.
- One writer per shard publishes a copy of the shard as the next epoch and swaps it atomically (`ArcSwap`); readers hold an `Arc` of one epoch and never wait for a writer. A change batch is applied all or nothing: a refusal discards the working copy, so no partial write is ever visible. The copy is the cost of one batch, not a rebuild: postings, arena and graph are not recomputed.
- A replaced record takes a new slot and tombstones the old one; tombstoned slots leave the postings at once and leave the arena and graph at compaction, which builds a new generation beside the live one.
- A drained shard (no live record) releases its embedding pin, and a record of another dimension then starts a fresh arena as a new generation.
- Reads cover the asking tenant's shard and the commons shard. A visible shard pinned to another embedding version or dimension refuses the whole read, naming the expected and offered value.
- Vectors are stored L2-normalized; the similarity is the inner product. Pools above `designation.flat_threshold` use the existing `HnswIndex::search_filtered` with the pool as the admit predicate; there is no second ANN.

## Task decomposition and skill planning: design calls (R134.3, R134.4)

- One canonicalizer. `decompose_task` (decision-types, `task_plan/decompose.rs`) and `plan_over_skills` (decision-stats, `skill_plan.rs`) both return a `TaskPlanProposal`; `stat_task_plan` hands it to `plan_explicit_tasks`. Neither adds a cycle check or a second plan type: a decomposition or skill query with a cycle gets the shipped `plan_cycle` refusal.
- Wire parameters. `decomposition` is `TypedValue::TaskDecomposition(DecomposeFacts)` and `skill_query` is `TypedValue::SkillPlanQuery(SkillPlanQuery)`; a `task_plan` question still takes exactly one parameter, and the name must match its type. The plan is resolved after the decision policy so the knobs come from the pinned policy; a request with no plan parameter reads no policy there.
- The output task is one of the subtasks (it lists what it combines in `depends_on`), so the proposal needs no synthetic sink. Independence is the empty transitive closure of `depends_on`; the output is excluded from the vote-or-concat decision, because it is the combiner, not a voter.
- Knobs are decision-policy knobs (DE-R059.1-KNOBS) in `knob_families/task_plan.rs`: `task_plan.specialist_min_score`, `skill_plan.bm25_saturation`, `skill_plan.relevance_floor`, `skill_plan.split_prompt_bytes`. Floors are compared as exact integers on Q32 (`score * 1e6 >= floor_micros << 32`); the only float is the BM25 score, quantised once by `quant::q32`.
- Skill text is the component summary plus its declared capabilities, joined by spaces, over `Published` skills the tenant can see (library search is tenant-scoped; another tenant's skill never changes a visible skill's score because BM25 statistics are candidate-local).
- Merge and split. Subtasks merge only within a group of equal chosen skill and equal `depends_on`, packed greedily in id order while the joined prompt stays within `split_prompt_bytes`; the merged task keeps the lowest id and every dependant (and the output id) is remapped. A prompt over the cap splits at line boundaries into `<id>.1`..`<id>.n`, a chain; dependants and the output id move to the last part. A single line longer than the cap is cut at the last character boundary that fits, so a part never exceeds the cap.
- Refusal codes `NO_SPECIALIST_FOR_SUBTASK`, `NO_SKILL_FOR_SUBTASK` and `EMPTY_SKILL_INDEX` are constants beside the existing task-plan codes and name the first uncovered subtask id in id order; the abstention table of R134.6 registers them.
## SQL column admission and the embedding set (R079.4, R079.6)
Decision: the connector sends one `SqlColumnProfile` per column on the `SourceRecord` itself (`column_profiles`, omitted from the canonical bytes when empty so no existing batch digest changes). Medulla alone decides admission in `admit_sql_column`; the column kind is a closed `SqlColumnType` (text, boolean, timestamp, numeric), an enumeration is text whose Q32 distinct ratio is below `embedding.column_min_distinct_ratio`, and free text needs `embedding.column_min_samples` values (default 100). Both are decision-policy knobs beside the other `embedding.*` knobs, read through `AdmissionKnobs::from_policy` as `EnrichDocument` does today (no per-tenant ingest policy exists yet; the engine default is the resolved policy). The embedding set is built in `prepare` from the records the batch actually admitted (a record governance refused contributes nothing) and rides the receipt as observation evidence (`column_exclusions`, `embedding_set`): stored in the receipt node, replayed from it, and outside the receipt digest like `drift`, so no stored layout or digest changes. The served proof lives in the h-admin served harness because only it can import the connector pack a `SourceIngest` needs. R079.6's own L1 test (`prepare_builds_the_embedding_set_from_admitted_columns`) is not written: the served R079.4 test covers `prepare`, and R079.6 stays open until its named test exists.

## Where the R079.6 proof runs
Decision: `prepare` needs an imported connector pack and a durable graph backend, which only the h-admin served harness can stand up, so its direct test lives in `medulla-h-admin` (`connector_pack/tests/embedding_set.rs`, dev-dependency on `medulla-h-ingest`) and calls `prepare` with a `PrepareContext` instead of a served `SourceIngest`. `PreparedSourceIngestion::embedding_set()` is the read accessor; no receipt is involved (DECISIONS FQ-R707.6: move the test to a crate that can host it).

## Retrieval feedback demotion (R079.5)
Decisions: the per-class usage is computed from stored facts only. Embedded units of a class are the chunks the stored `DocumentEnrichment` records currently list (class from the record's per-ordinal admissions); citations are the distinct cited span ids of retrieval outcomes recorded inside `embedding.demote_window`, joined to those chunks. The scan lives in `AgentLibraryStore::cited_span_ids_since` because server-graph cannot depend on the decide crate; the `retrieval:` key prefix moved there as one constant. A demoted class skips fresh spans as `Demoted`, but an exact repeat of an already-embedded span still reuses its vector (`SkippedExact`), since reuse makes no embedding call. The receipt's `demoted_classes` names the classes in effect for the call. Both knobs are decision-policy knobs (DE-R059.1-KNOBS).
## Where the R079.5 served proof gets its executed retrieval record
Decision: the L3 test drives the real served chain end to end. It builds the decision-plane `Harness` (public under the `test-support` feature of `medulla-h-decide`, already a dev-dependency of the root crate) around the served state's own Agent Library, publishes a feature schema into the test tenant, commits a retrieval-plan record through served `Decide` and `DecisionLog` Commit, rewrites that stored entry's outcome to an executed option (the outcome join reads the stored entry, it does not replay), and then records the citation through served `DecisionLog` Learn `RecordOutcome`. No release seam is added.

## Paper admission (R031.100)
- **Relevance is the calibrated probability of a pinned head over two features.** BM25 of `paper.summary` against the platform goals (corpus is the candidate set) and the cosine similarity to the nearest row of the named content graph. A head without a calibration table abstains every candidate with `UNCALIBRATED_CANDIDATE`; no caller or model score is relevance.
- **Nearest-neighbour similarity is a recorded question parameter.** The server reads `GraphVectors::probe` for each staged paper (excluding the paper itself) through the caller's graph ACL and records the result as `nearest_neighbour:<graph_param>` in the request, with the graph version it read at, so a replay of the record recomputes the identical matrix and never reads the live graph.
- **The relevance floor is a decision-policy knob** (`enrichment.relevance_min`, DE-R059.1-KNOBS), read from the policy the `refine` hook receives.
- **The proof trains a head on four archetypes** (both, goals only, graph only, neither) so a paper that matches one signal falls below the floor; a floor of 1.0 shows the threshold itself decides, and the stale and duplicate refusals are asserted by code.
## R036.105.2: the commit handler's order and decisions (decided)
- **Begin first.** `Coordinator::begin` writes the pending marker with the certificate request before either owner reserves, so a changed request for a decided txn id answers `CERTIFICATE_INTENT_CHANGED` before `ADMISSION_ABORTED` or `ADMISSION_INTENT_CHANGED`. An earlier ABORT refuses `CERTIFICATE_DECIDED_ABORT` and an earlier COMMIT resumes with the owners' existing reservations.
- **The vote precedes the decision.** The stale-catalog check is the vote; a NO decides ABORT. The record is written only after a durable COMMIT, and both owners resolve from `recover`, never from the local write result. If the write fails after a durable COMMIT, a retry with the same key resumes and finishes the write.
- **No backend, no commit.** A server without the redb coordinator store refuses `DECISION_COORDINATOR_UNAVAILABLE`; there is no local fallback.
- **Codes are registered.** The four certificate codes and `DECISION_COORDINATOR_UNAVAILABLE` are `DecisionErrorCode` variants.
- **Test clock.** The request time can be overridden only by a `cfg(test)` task-local, so the L3 expiry case needs no sleeping and no release build carries the seam.

## R036.105.1: certificate table placement and integrity (decided)
- The certificate table is the last entry of the graph-shard census slot (not placed beside `xshard_decision`), because the shared table-name macro has one slot for every table added since its frozen body; a predecessor is then the current slot minus the new table, and the pinned layout digest changes with it.
- The stored certificate carries its own SHA-256 trailer in addition to the at-rest cipher, so a flipped byte is refused when no key is configured as well. The intent check (`CERTIFICATE_INTENT_CHANGED`) lives in `store_certificate`, which can decode the sealed value; the storage function only writes bytes.
- The predecessor digest test uses the predecessor file builder rather than a recorded fixture, as the effect-receipts predecessor does: a recorded fixture is evidence taken from a real build run, which a writer cannot make.

## Isotonic calibration of two-class heads (R032.2.1)

- **The positive class is the first option.** A two-option item scores its first option as the positive class. The isotonic map takes that option's temperature-scaled probability on the Q32 grid and returns its calibrated probability; the served distribution is `[g(p0), 1 - g(p0)]`. The map is fitted on the target share of the first option, so a labelled pair that accepts both options counts one half.
- **The served probability equals `IsotonicFit::predict`.** Scores are fitted and served on the Q32 grid, so the stored blocks and the fit agree exactly. A score between blocks reads the lower block; a score below every block reads the first one; a score above reads the last.
- **The conformal and act thresholds run over the isotonic probabilities.** `held_out` maps each distribution through the table before the nonconformity scores, so the coverage claim describes what the head serves.
- **A head with an isotonic table serves two options only.** Any other candidate count refuses with `ISOTONIC_REQUIRES_BINARY`, at fit and at `Decide`.
- **The new field is additive.** `HeadCalibration.isotonic` and `DecisionFitRequest.calibration_method` are omitted when absent, so the bytes and digest of every existing head and request are unchanged; no stored layout changes and no upgrade step is needed. A fit that names `Vector`, `Dirichlet` or `None` refuses with `CALIBRATION_METHOD_UNSUPPORTED`.

## Route pipeline and fast path (R035.750.1)

- **`route.fast_path_max_tokens` is a decision-policy knob.** It is a kernel threshold, so it lives in `knob_families/route.rs`, is pinned through the decision policy and sealed with its digest. The kernel (`decision/route.rs`) takes the bound as an argument and reads no configuration.
- **The learned policy is the shared ladder behind the rule stages.** `route_pipeline` runs the rule stages in fixed order and refuses with the registered `ROUTE_NO_STAGE` when none answers. The family treats that as the ladder's turn, so a route question with no head still abstains with the ladder's own typed reasons; `ROUTE_NO_STAGE` is the pipeline's own refusal, tested on the kernel.
- **Rule stages read declared options.** The fast path classifies options by the `eg:route/fast-path` class the caller declared. A route question over library or graph candidates states no classification at the conclude hook, so it has no rule stage and the ladder decides.
- **A rule-stage answer is a constraint.** It is sealed as an uncalibrated advisory choice at full score, resolved as `Constraint`, with one `route_stage=<stage>` premise of class `Definition` and `Policy` provenance carrying the policy digest. `Executed.route` carries the stage name from the family to the seal.
- **The turn size is the typed int param `route.turn_tokens`.** A negative or non-int value refuses with `PARAMETER_INVALID`; an absent value leaves the question to the ladder.

## Column-to-property score (R033.2.1)

- **The kernel is pure integer Q32.** Jaccard, weights and the sample fraction are `i128` fixed-point values, so the score is reproducible bit for bit. The score is clamped to `[0, 1]`: the three weights are each in `[0, 1]` and may sum above one.
- **Weights are decision-policy knobs.** They are kernel thresholds, so they live in `knob_families/schema_mapping.rs` as `DecimalMicros` knobs and arrive as arguments; the kernel reads no configuration.
- **Names compare as token sets.** A name splits on punctuation and camelCase humps into lowercase tokens; the property side is the IRI's local name. An exact local-name match is equality once case and punctuation are removed.
- **Datatype credit is a closed table.** The same value type is exact (1), a lossless widening (integer to decimal, date to date-time) or a rendering as text is one half, anything else is zero. A property that declares no datatype earns no datatype or sample credit and cannot back a proof or an observation.
- **Evidence classes.** `Proof` needs an exact local-name match and an exact datatype match; `Observation` needs a declared datatype, at least one sample and every sample valid; everything else is `Claim`. A proof outranks an observation. A column whose SQL type is outside the closed table has no `ValueType`; the served family (R033.2.2) refuses it with `SCHEMA_MAPPING_SQL_TYPE_UNKNOWN`.

## Served ResolveEntity pair decision (R032.2.2)

- **`resolve_entity.alpha` is a decision-policy knob.** It is a kernel threshold, so it lives in `knob_families/resolve_entity.rs` (stored as micros, default 0.1, bounds [0.01, 0.5]), is pinned through the decision policy and sealed as a `knob:` premise; it is not a registry knob and has no `knob-table.md` row.
- **`refine` receives the pinned policy.** The act rule needs the policy's knob, which the post-ladder hook did not carry. The one `refine` hook gains `policy: &DecisionPolicy` beside `policy_digest`; every family impl and both callers (`decide_blocking`, the replay verifier) pass it. There is still one hook.
- **The head's alpha is compared in integers.** The head's calibrated alpha is a rational; it is compared to the knob's micros by cross-multiplication, so the bound is exact.
- **An advisory answer abstains.** An uncalibrated advisory outcome (a head that cannot act) abstains with `InsufficientConfidence` rather than naming a pair, because a pair decision that cannot act has no proposal to make.
- **The acting head in the L3 test is built by hand.** An inline-fitted head is always synthetic and never acts, so the test publishes a non-synthetic head with a certified act threshold through the real publish gate, behind a passed receipt written for exactly its bytes.

## Gold-set coverage gate (R032.2.3)

- **The gate runs beside the evaluation, not inside the named-evaluator module.** `stat_evaluator.rs` holds evaluator grants; the coverage the evaluation measures is `FullLabelMetrics.covered` from `evaluate`. `stat_gold_guarantee.rs::apply` holds a finished receipt to the guarantee, so `evaluate` and its many callers keep their signature.
- **The guarantee is asked for by name.** `DecisionEvalRequest.gold_guarantee` and `DecisionEvalReceipt.gold_guarantee` are omitted when absent, so the bytes and digest of every other request and receipt are unchanged and no stored layout changes.
- **The shortfall is a failed gate named by its registered code.** `GOLD_SET_TOO_SMALL` is judged first (a coverage over too few pairs is not believed); `CONFORMAL_COVERAGE_UNMET` otherwise. The receipt fails, and a head cannot be published on a failed receipt.
- **The target alpha is the policy's.** Coverage is held to `1 - resolve_entity.alpha` of the evaluation's policy, the same bound `Decide` acts at, so a head that passes the gate is one the decision may act on at that alpha.

## Team reuse route stage (R035.750.2)

- **Stage order is fast path, team reuse, learned policy.** A short turn is a cheap rule the caller states; evaluated history answers only when no cheaper rule does.
- **History reaches the family through `ExecutionContext.matched`.** `decide_blocking` already reads the caller's own same-context executed decisions (R020.1.1); the context carries them, so the family reads no log itself and tenant and principal isolation stay where they are enforced.
- **An option's judgements are the independent evaluations of the matched decisions that ran it.** A censored evaluation (no success value) counts toward nothing. Among qualifying options the best rate wins, then the most judgements, then the lowest id.
- **Floors are decision-policy knobs.** `route.team_reuse_min_samples` and `route.team_reuse_min_rate` live in `knob_families/route.rs`.
- **Resolution and evidence.** A team-reuse answer resolves as `Statistical`; it seals `route_stage=team_reuse` (class `Definition`, provenance `Policy`, as the fast path) and one `matched_record:<id>` premise (class `Observation`) per matched record of the chosen option. The stage's resolution kind travels on `RouteExecuted`, so `seal` has no per-stage branch.
## Shared commit: one Agent Library transaction for the record and its evidence (R113.2.5)

- The decision-record revision and the evidence graph revision are written in ONE `open_write` transaction: the graph's head compare-and-set and revision row are applied with the same `apply_revision_rows` primitive inside the component write's owner rows, so a failure after the first row leaves neither (proved with a test-only failure seam compiled under `cfg(test)`).
- The evidence graph pins the record through `synthesis_evidence`. The record does not exist yet when the pin would be resolved, so that single pin is checked against the record being written (id, kind, definition digest; a mismatch refuses `DERIVATION_REJECTED`), and every other pin is resolved in the write as an ordinary graph publish does.
- The fence check names the coordinator transaction: a fence held by the commit's own transaction never blocks it; any other holder of a candidate the record was decided over, or of a component the graph pins, refuses `ADMISSION_WRITE_BLOCKED` and nothing is written.
- Open: the evidence revision carries no separate outbox event of its own in this transaction; the decision-record event announces the commit. The handler wiring (coordinator graph participant for the lease slice) is the remaining part of R113.2.5.

## Shared commit handler: one multi-demand acquire inside the certificate coordinator (R113.2.5, R113.2.2)

- **The lease joins the existing coordinator sequence.** `DecisionCommit` already decides through the owner-only certificate coordinator, so no `GraphSlice` or multi-raft is involved. The capacity ledger is the third participant: its checked prepare (`AcquireCapacity`) runs after the owners reserve and before the vote; a denial or refusal decides ABORT and both owners resolve from it. After a durable COMMIT the record and evidence write runs (`commit_topology_decision`); ABORT releases the leases under the same transaction key.
- **One request, all cells.** The ledger already grants a multi-demand acquire all-or-nothing in one write, so one `AcquireCapacity` carries every positive demand instead of one request per cell. It is keyed by the coordinator transaction id, so a resumed commit replays instead of double-acquiring.
- **The evidence graph and the ledger graph come on the wire.** The solved record names no ledger graph and the engine cannot rebuild the evidence graph's shape, so the request carries `topology { capacity_graph, evidence }` (a serde-default optional field; existing callers are unchanged). Evidence sent for a record that leases nothing, or missing for one that does, is refused `DERIVATION_REJECTED`. The caller must hold write access to the capacity graph, and the evidence context is bound to the verified request with the idempotency key suffixed `:evidence`.
- **Premises are served.** The result's `topology.premises` lists `lease:<id>` per lease so the lease set is observable; a replay of a stored record acquires nothing and reports none.
- **Lease lifetime is a registry knob** (`topology.lease_ttl_ms`, default 15 minutes), never a constant.
- **Denial code reuses `CAPACITY_DENIED`** from the engine code vocabulary (same meaning), with the one-time retry allowance of `RetryAllowance` named in the message; R113.2.3 owns its test.
- **R113.2.2 moves to h-decide.** The persistence-records crate cannot reach `DecisionCommit`, so `topology_commit_publishes_capacity_and_evidence` lives beside the cross-owner journey (DECISIONS FQ-R707.6).
- **R113.2.1 test.** The guard test now runs with no persistence configured, because a capacity-capable backend takes the shared path.
- **Denial proof (R113.2.3).** The test takes the whole cell with another holder, so the ledger's own checked acquire answers the denial; the handler decides ABORT (the owner's Agent Library reservation resolves Aborted), writes no record, lease or evidence, and answers `CAPACITY_DENIED` naming the one-time retry allowance. The code is also listed in the `topology_enforcement` refusals table.

## Commit recovery and the repeat refusal (R113.2.4)

- **Repeat of a decided commit is refused, not replayed.** A capacity-backed commit whose record is stored refuses with `COMMIT_ALREADY_DECIDED` (a `DecisionErrorCode`), naming the record, its version and the evidence graph. Before refusing, a durable COMMIT of the same transaction finishes any owner resolution a crash left open (the resolution is idempotent), so the refusal never hides an unresolved reservation. Ordinary records keep the idempotent replay.
- **Interrupted prepare.** A pending marker with no decision whose certificate deadline has passed can no longer be decided COMMIT, so nothing is in flight for it. The next presentation of the same request records presumed abort (`Coordinator::recover`), releases the leases (the acquire replays under the transaction key) and the reservation, and answers `CERTIFICATE_DECIDED_ABORT`. Before the deadline the marker may belong to a live commit and is left alone.
- **Crash seam.** A task-local `CRASH_AT` with the three boundaries (prepare, decision, apply) exists only under `cfg(test)`; it answers `DECISION_COORDINATOR_UNAVAILABLE` and runs no cleanup. The test moved to h-decide (FQ-R707.6): persistence-records cannot reach `DecisionCommit`.
- **Quality decay is a rolling MRR against an activation baseline.** The monitor keeps the last `drift.quality_window` reciprocal ranks of cited outcomes and compares their mean with the MRR measured at activation. A decay above `drift.quality_decay_pct` percent fires after two consecutive evaluations, like every other signal. With no baseline or no outcome the signal is simply absent.
- **Staleness is the `stale_index` rule, declared once.** The lag of the graph version over the version the index is embedded through is compared with `discover.max_version_lag`. The knob is declared beside the monitor; the discover quality verdict (R089.2) reads the same knob rather than declaring a second one.
- **The changed-node bitmap is run-length compressed over revisions.** Marks keep sorted, disjoint, non-adjacent inclusive runs, so memory grows with the number of contiguous ranges, not with writes. A plan names only firing shards; a shard with changes but no firing signal contributes nothing. Beyond 4096 regions the smallest gaps are joined first, so the plan stays a superset of every change and never drops one.

## Refresh dedupe and budgets (R174.9.2)

- **One canonical URL, one digest per URL.** A result is keyed by its canonical URL: scheme and host in lower case, the scheme's default port and the fragment removed, an empty `http` or `https` path written as `/`. A held digest equal to the incoming one is a no-op; another digest supersedes the row. A batch is classified against the held rows as it is read, so a URL repeated inside one batch is stored once, and a malformed result refuses the whole batch before anything is stored.
- **The region mark is a property of the outcome.** `DedupeOutcome::marks_region` is true for a new or superseding row. The pure functions live beside the types in `decision-types`, which cannot see the drift monitor; the wiring row applies the mark through the monitor's changed-revision bitmap.
- **Request allowance is a token bucket over `freshness.window`.** `max_requests` tokens refill evenly across the window, so a burst is bounded and a steady rate is allowed. Cost units are a running total against `max_cost_units`. A refused request spends nothing, and the budget that refused is named (requests or cost).

## Refresh wiring (R174.9.3 to R174.9.5)

- **One read seam, registered by the host.** The refresh reads through a `SearchSourcePort` (binding lookup plus one query read), registered process-wide like the connector row port. Tests register a recorded source; the host registers the federation-backed one (R174.9.3.1). A graph with no binding refuses `SEARCH_SOURCE_UNREGISTERED`, including when no port is registered.
- **A result's content digest is its title, snippet and engine.** The search source returns no page body, so the content a result carries is those three fields, length-framed and hashed with SHA-256. A changed snippet supersedes the row; the page body digest is the fetch path of R174.9.7.
- **Every refusal is decided before the first read.** Policies are checked first, then the budget is charged for every query on a copy of the bucket; the copy replaces the held bucket only when the whole refresh stores. A refused refresh writes no row, no region mark and no receipt.
- **A region is the shard of the URL's host.** Marks are `{shard_id, node_type: Document, revision}` with one revision per new or superseding row, kept as a per-graph counter beside the rows and the receipt in one atomic batch. The drift monitor loads them into its changed-revision bitmap.
- **The request allowance bucket lives in process memory.** It is a rate limiter over `freshness.window`; a restart refills it, which only ever allows a fresh window.


## Ranked skill tree: design calls (R134.9)

- A prior outcome needs a skill attribution the log already carries: an evaluation's `selected_agent` names the skill it judged, and its record's sealed `context_digest` names the context. No second outcome store exists; the tree reads the decision log through the same visibility rules as the exploration match (same tenant, same principal).
- The request's context digest is the sealed `context_digest_of(question, params)` with `outcome_pins` cleared, so a pinned and an unpinned ask of one query share a context and prior plans of the same query match.
- `outcome_revision` is the record's stored evaluation count (the log only appends) and `outcome_digest` is the digest of every stored evaluation (id, success, agent) of the record; a ref is stale when either differs. A caller pins refs through `SkillPlanQuery.outcome_pins`; the engine then uses exactly those records, refusing `STALE_OUTCOME_EVIDENCE` for a missing record, another context or another principal, or a changed outcome. Unpinned asks read the matching log, newest first, at most 64 records.
- The score is exact integer arithmetic on `Q32`: `relevance + weight * (2s - n) / (2n)`, floored, clamped to `[0, 1]`; below `skill_tree.min_outcomes` it is the relevance alone. Equal scores order by skill id.
- A node's parent is the nearest kept ancestor on the `parent` attribute chain. A chain that returns to a skill refuses `SKILL_TREE_CYCLE`; a parent the tenant does not see refuses `SKILL_PARENT_UNKNOWN`.
- The trees are sealed as `TaskPlan.skill_trees`, which is part of the record answer and so of the record digest, and the plan digest is re-bound over them. A plan with no trees keeps its bytes.
- Knob premises (`knob:skill_tree.*`) are added only to a `skill_query` plan, so other task plans keep their premises.
## R038.3: held-item admission reads stored items from a named graph

The held items (compiled run programs, run output proposals) are nodes of the graph the question names, read the way R038.2 reads trust marks: `serve` opens the graph through the caller's access check and row-level filter, so an item the caller may not see is as absent as one never written (`HELD_ITEM_UNKNOWN`) and a foreign tenant is refused `ACCESS_DENIED` before any read. The row keys the family reads are declared in `orchestration-v1.ttl` under `:HeldItem`; the property names carry a `heldItem` prefix because `:verifiedBy` already exists in the SDD vocabulary with another meaning. `lifecycle` is stored data the family only reads (`active` or `retired`); the family never writes it, so it adds no state machine. The refusals (`DIGEST_MISMATCH`, `VERIFIER_NOT_INDEPENDENT`, `PASSES_INSUFFICIENT`, `RETIRED`, `HELD_ITEM_UNKNOWN`, `CRITIQUE_MISSING`) are family abstentions under registered codes; `admitted` and `rejected` are the advisory options (a critique `reject` is the one way to `rejected`). The pass floor `held_item.min_passes` is a decision-policy knob in `knob_families` (DE-R059.1-KNOBS), read from the policy `conclude` receives. The digest is compared as recorded: the stored `content_digest` is the digest of the item, so no re-hash helper is involved (the earlier mention of `digest_text` is dropped from the row). The critique verdict is bound to the item when its `critique_digest` equals the item's content digest; the row's pointer to a CQ-CONN row for the critique verdict class named no existing provider, so the verdict is recorded on the item itself.
