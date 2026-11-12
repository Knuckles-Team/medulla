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
