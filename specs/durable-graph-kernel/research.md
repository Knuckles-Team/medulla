# Research: durable graph kernel decisions

## RESP listener enters the served KV methods (MEDULLA-DURABLE-KERNEL-R860.3)

Decision: the RESP listener keeps no store. Every served string command enters the
dispatch through one verified request context built after `AUTH`
(`VerifiedRequestContext::authenticated_resp_actor`, grant `kv:read` and `kv:write`
only, never `kg:*` or `broker:*`), and reaches `KvGet`, `KvPut`, `KvDelete` and `KvCas`.
The principal's private KV scope is the keyspace, so isolation is the engine's own.

Why: one admission and policy path for every wire; one durable store; no second
`kv.redb` with its own physical identity.

Consequences, decided from the north star (one engine, no sync; hard cutover):
- `MULTI`/`EXEC`/`DISCARD` are refused. The served methods commit one key at a time, so
  an atomic multi-command transaction would be a false promise.
- `SET` options (`EX`, `PX`, `NX`, `XX`) are refused; expiry and conditional writes
  have no served KV method.
- `DECR`, `INCRBY`, `MSET`, `SCAN` and `TYPE` are refused rather than half-served.
- The `INCR` retry bound is the registry knob `resp.incr_retry_limit`.
- Request ids start from the process start time in microseconds so the durable mutation
  ledger never sees a repeated idempotency key after a restart.

## Graph change feed over the one CDC feed (R043.100.2, R043.100.3)

- Decision: `OpenGraphSnapshot` and `ReadGraphChanges` read the change feed `CdcHub` already keeps
  for every committed graph write. A second ledger or store would be a second write path and two
  orders of the same commits.
- Decision: a feed's incarnation is minted when the feed is created and again when the graph is
  cleared. A restart creates new feeds. A cursor of an earlier incarnation is stale, never
  silently re-read from the same numbers.
- Decision: the authorization revision is a digest of the agent and the sorted scopes of the
  verified token. No separate revision counter exists; a grant or revocation changes the token
  scopes, so the digest is the revision.
- Decision: an event is withheld when none of its images passes the caller's row-level predicate.
  The page cursor advances past withheld events, so a page never reveals how many were withheld.
- Retention is the retained ring window of the hub; a cursor behind it is `CURSOR_EXPIRED`.
## S3 objects are blobs the native blob methods read (MEDULLA-DURABLE-KERNEL-R860.6)

Decision: the S3 listener stores each object as a blob manifest (content-defined chunks plus
manifest) whose owner scope is the carrier scope of its fixed service principal
`service:s3-api-client`. The object digest and the ETag are the manifest digest. The listener
keeps its bucket and object listing index in its own durable KV store.

Why: the hot-path proof reads an S3 object back through `BlobFetchBegin` and `BlobChunkGet`,
which serve only a manifest owned by the caller's scope. A raw chunk digest with no manifest
was not readable through the served blob methods, so the S3 surface was a second store in
practice. Owning the manifest by the service principal keeps one blob authority with one
ownership rule and needs no new method.

Consequences: an ETag is a manifest digest, not the digest of the bytes; identical bytes still
share one digest. A multipart upload keeps its parts as staged chunks and writes one manifest
at completion.


## Concept ids and lineage edges are tenant-keyed Agent Library rows (MEDULLA-DURABLE-KERNEL-R094.7, R094.7.1)

Decision: `concept_ids` is keyed `(tenant, concept id)` and `concept_lineage_edges` is keyed
`(tenant, superseded, successor)`, both in the Agent Library owner. The served method is
`ConceptLineage` with ops `claim`, `add_edge` and `read`; writes commit through the admitted
pack-write lifecycle (replay identity, staged batch, owner rows, receipt), reads come from one
committed snapshot.

Why: a principal or repository name can exist in two tenants, and a key without the tenant would
let one tenant's claim collide with, or make visible, another's (ISO-OWNER-KEY). The owner already
carries the receipt, replay and outbox rows a durable write needs, so no second store is added.

Decisions made while building:
- The decision to allow a claim or an edge stays in `ConceptPathSet::claim` and
  `ConceptLineage::add_edge`. The adapter loads only the rows those calls need: the two endpoint
  rows, and the edges reachable from the successor (a cycle through a new edge `a -> b` is a path
  `b -> ... -> a`), so the write costs the number of edges touched, as the row requires.
  `ConceptLineage::restore_edge` puts stored edges back without re-checking them; the store only
  ever holds edges `add_edge` accepted.
- Ids travel as text and are parsed by the handler, so a malformed id refuses `CONCEPT_ID_MALFORMED`
  and a path claimed by another repository refuses `CONCEPT_ID_PATH_COLLISION` (the landed
  `ConceptIdError`, given codes), instead of failing the wire decode with an untyped error.
- Authorization reuses `agent:component-write` and `agent:component-read`: the concept store is part
  of the component catalog and no new privilege is introduced.
- `Read` returns the concept and at most `concept_lineage.read_max_successors` (registry knob, default 1024, range [1, 65536]) direct successors with a
  `truncated` flag, so a page is never mistaken for the whole set.
- Re-claiming an id by its owner, and re-adding an edge, return the first stored evidence and write
  nothing new.
- LAYOUT-REGEN needed: the Agent Library layout digest pin in `owner/lineage.rs` is replaced by the
  compiled digest (the foundation commit already declares the predecessor and upgrade).
- WIRE-REGEN needed: `ConceptLineage` is a new method; the wire vectors are refreshed by the gate.
## R871: the served Batch method (decision)

`Batch` is one closed `BatchOperation` set (create-if-absent, compare-and-set, claim-next,
reinforce, and the read `GetNodeProperties`) so every inner operation has exactly one served
method whose scope it needs. The request boundary checks every inner scope (through the policy of
the method each operation stands for) before the handler runs, and the handler validates size and
blobs before the first operation: a refusal writes nothing. The commit is the existing staged
gateway commit, so a read fault inside the batch (a durable-tier error) aborts it whole.
Durable replay, the Raft state machine and the embedded engine run the same
`apply_batch_operations`. The latency budgets are two report groups, because they are two
budgets: `decide/agent_loop` (a served read and write, p99 2 ms) and `decide/agent_loop_batch`
(a served 64-operation batch, p99 10 ms). Wire vectors and the generated Python senders need
the WIRE-REGEN refresh at the gate.

R871 review calls: the operation cap is the registry knob `batch.max_operations` (default 64,
[1, 1024]); the wire crate no longer holds a constant, and `validate_batch` takes the resolved value
from the handler that reads the knobs (KNOB-SWEEP-NOMOVE). Replay never drops a read fault: a
committed batch that faults while replaying is a `ReplayDivergence` (`REPLAY_DIVERGED`), returned by
`durable_apply::apply` and `mutation_apply::apply`; the Raft apply and the embedded engine return it as
their error, and the follower pull loop stops applying rather than advance past history it could not
reproduce. LAYOUT-REGEN is not needed; WIRE-REGEN is (the generated `ServerErrorCode` list).

## R875: census renames of non-lifecycle wire types (decision)

`SyncState` is `SyncStanding` (field `standing`); the `TerminalEventRead` tag is `availability`; the
`ChangeEnvelopeOutcome` tag is `result`; the `SemanticIndexOutcome` tag is `disposition`, because its
`accepted` and `partial` variants already carry a `result` field that an internal tag of the same name
would collide with. The wire words are unchanged; each key is a hard cutover, so the old key no longer
decodes. WIRE-REGEN needed: the generated mirrors in `medulla/generated/models.py` were edited by hand.
- Scheduler settings are registry knobs (DK-R076.2). `GetSchedulerConfig` is the served read of the values in force; a setting changes by restarting with the knob, because no served knob write path exists and a persisted override would be a second store. `load_knobs` validates the whole scheduler policy, so over-subscribed floors refuse at load as `SCHEDULER_CONFIG_INVALID`.
- Admission measures lane load from the live grants (workers per lane) and the durable task queue (`JobStore::ready_count` of family `task.<lane>`); the time source is `ServerState.clock`, so expiry is tested without sleeping.

## Task queue served methods (MEDULLA-DURABLE-KERNEL-R072)
- Decision: the lane a grant names is the queue its worker claims from. A producer enqueues on the lane that serves the work; the engine, not the claim, picks the lane (MEDULLA-PLACEMENT-R002.5), so a claim carries no lane field.
- Decision: a grant id alone never lets a caller act as a worker. The claim and the ack both require the worker to be live and owned by the verified principal, and the claim requires the grant to be live and issued to that worker. Grant ids are derived from the worker and the clock, so ownership is the real check.
- Decision: an ack is tenant-scoped. The job's tenant must equal the verified tenant, or the task is `TASK_ACK_NOT_CLAIMED`, so two tenants that reuse a worker id never act on each other's tasks.
- Decision: the per-tenant lease ceiling is the registry knob `scheduler.task_queue_tenant_max_active`, not an environment variable.
- Decision: the three methods are local-only authority (`LOCAL_ONLY_METHODS`): they commit through the process-local job store and clustered admission refuses them, like the other `jobs.redb` writes.

## One task queue store (MEDULLA-DURABLE-KERNEL-R866)
- Decision: a WorkItem is a graph row, and its claim is a lease property of that row written by the claim arm of `apply_work_item_rows`. It is not a claim table, so the one table family that holds claims on queued work is the jobs store's lease and ready tables. R750's own children (enqueue counts in-flight rows of the graph, refusals, hot path) already describe the row form, so the R866 sentence that routed the dispatch queue through the job store is corrected to say that a WorkItem adds no claim table. Moving WorkItem rows into the job store would give one work row two homes.
- Decision: the static check is the declared-table census of every owner layout. Any table whose name mentions a claim, lease or ready queue must be classified in the test as not holding work claims (with a reason) or live in the jobs layout; a new table that mentions one of those words fails until it is classified. A served enqueue and claim then shows the lease on the job record.

## Downtime scheduler tick (MEDULLA-DURABLE-KERNEL-R101.3)
- Decision: the tick is engine-owned work whose tenant is the commons graph it reads; no request carries a tenant. Leases name tenant `__commons__` and owner `engine:downtime`, and jobs are stamped with that tenant.
- Decision: job types are `dw:JobType` graph facts (with a `dw:Budget` node reached by `dw:hasBudget`) registered at startup through the served `BatchUpdate` path, so they are durable and replicate. Property keys are the full ontology IRIs, read back with one raw-projection SPARQL `SELECT`; a precondition is a boolean query run as an `ASK` over the same snapshot.
- Decision: the downtime lease draws on one capacity cell `downtime:cpu` (CPU class) created on first use through `UpdateCapacityCell`; its capacity is the window CPU budget (`downtime.window_cpu_permille` times `downtime.tick_secs`). A lease is taken per chosen type before its job is submitted; a denial refuses that tick's submission.
- Decision: the job is submitted on the process job store through two new `JobsApi` seam methods (`register_downtime_intents`, `submit_downtime_job`) implemented by `JobsSeam`, never from `medulla-server` directly; a type's intent is registered as a manual `JobIntent` so `DowntimeJobType.intent` names a registered intent as R101.1 says. The submit is idempotent on a key naming the type and the tick instant.
- Decision: the served read `DowntimeSelections` moves to R101.5, which writes the selection nodes it returns; R101.3 has no selection data to serve. The row text of both rows says so.
- Decision: the in-process last-run map orders ties until R101.5 persists selections; R101.5 replaces it with a read of the newest selection per type.
- Decision: the foreground meter's `ForegroundReading` feeds `ForegroundActivity` of `medulla-jobs`; the meter counts at dispatch entry, so no separate queue exists and `queue_empty` is the absence of a busy second.
- Wire: no wire type is added by this row. The ontology source `core:downtime-work@1` joins the core catalog (66 sources, byte pin recorded).
- Decision: a type that holds an unexpired lease or has a job in flight is not eligible, and budget fit is measured against the window less the budgets of the types that hold a lease. The tick otherwise chose a type whose lease the ledger then refused as exhausted. The order stays priority, then oldest last run, then IRI; in-flight jobs are tracked in process by the job id each submit returned.

## Downtime refusals and the job body (MEDULLA-DURABLE-KERNEL-R101.4, R101.6 groundwork)
- Decision: `DOWNTIME_BUDGET_REQUIRED` and `DOWNTIME_BUDGET_EXHAUSTED` are registered `ServerErrorCode`s. `DowntimeJobType::validate` and `check_budget` (medulla-jobs) carry them; `register_type` is the one registration path and checks the type and the window before it writes any fact.
- Decision: the window check is `SchedulerConfig::validate_downtime_window`; the over-subscription error is the existing `InvalidSchedulerConfig::OversubscribedFloors`. Downtime jobs run in the pool `downtime`; a floor on that lane makes it a foreground lane, and the tick refuses to submit.
- Decision: a downtime job body (`executor_downtime.rs`, dispatched from `execute_claim` on algo family `downtime`) works in slices of at most 100 ms, checkpoints after each, stops at a checkpoint with `DOWNTIME_BUDGET_EXHAUSTED` when its work exceeds its CPU budget, and records the budget it used in its typed result row. The work a slice does is metered wall time: intent-specific steps attach to the slice loop when the intents' own rows land.
- Decision: preemption is observed by the job body at each checkpoint from the foreground meter (a served request within `downtime.idle_quiet_secs` pauses it; the quiet period resuming it), through the explicit `downtime_phase` machine (Working, Paused; events Preempt, Resume) persisted as the checkpoint stage. This bounds the pause by the checkpoint interval, not by the one-second tick, and leaves the job's cancel flag for real cancellations. R101.6's row text is to be updated to say so when its served test lands; `request_cancel_batch` / `resume_batch` are not used for preemption.
- Open for the R101.6 item: the served L4 test needs a served read of each downtime job's state and order (R101.5's `DowntimeSelections` read, extended with the job id and checkpoint stage of each submitted job).
