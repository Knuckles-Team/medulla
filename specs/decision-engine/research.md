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

## Scoped adapter scope membership and monitor trigger (R174.7.3)
Decision: a judged retrieval belongs to a corpus scope when its recorded plan template anchors on that corpus class, and to a capability scope when its template's task class is that capability; the graph scope covers all. A scope that no judged retrieval of the tenant belongs to is refused `ADAPTER_SCOPE_UNKNOWN`. The monitor is one governed learning write (`MonitorAdapter`) that reads the scope's active adapter and the outcomes judged in a window, and moves the pointer back itself when the Wilson upper bound of the win rate falls below one half, so rollback has one writer and one receipt. A refit's `parent_digest` is the scope's active adapter and its `revision` the parent's plus one.

## Index-side adapters (R174.7.3.2)
Decision: the transform `W = I + sum(g_i u_i u_i^T)` is symmetric, so `q . (W d) = (W q) . d`: an index-side adapter is the same fit as the query-side one, evaluated by the same held-out comparison, and its receipt records `side = index`. The side is a field of the adapter body (left out of the canonical bytes when it is `query`, so existing digests hold), because the stored adapter is keyed by body digest and two sides of equal directions would otherwise overwrite one receipt. An index adapter is materialized, never pointer-activated: the generation cycle applies it to the active generation's stored vectors, the existing generation receipt qualifies the shadow, and the generation pointer move is the activation. The cycle then queries the shadow with the base query. A node the active generation does not embed (or the caller cannot see) is left unembedded, which the receipt's embedded-count rule rejects if it leaves the shadow smaller than the active generation. Background capacity for such a cycle is CPU, not an embedding endpoint.
## Ordering usage guidance steps (R177.2)
Decision: `:precedes` is a hard constraint and a success rate only chooses among steps that are ready at the same time. Ready steps are scanned in tool-id order, so the pairwise rule (rate when both tools have `usage.min_trials` outcomes, otherwise tool id) is applied the same way for any input order. Why: a pairwise rule that mixes a rate with an id fallback is not transitive, so a fixed scan order is what keeps the answer reproducible.
