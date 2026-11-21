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
- Decision: job types are `dw:JobType` graph facts (with a `dw:ResourceBudget` node reached by `dw:hasBudget`) registered at startup through the served `BatchUpdate` path, so they are durable and replicate. Property keys are the full ontology IRIs, read back with one raw-projection SPARQL `SELECT`; a precondition is a boolean query run as an `ASK` over the same snapshot.
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
- Decision: the served L4 test reads each downtime job's state through `DowntimeSelected.job` (R101.5 (4a)), filled at read time from the job store; a preempted job reads `submitted` with a checkpoint, because `Running` always holds a lease.

## Downtime selections and real work (MEDULLA-DURABLE-KERNEL-R101.5, R101.6)
- Decision: a tick that finds a candidate records its selection as graph facts in the commons graph through the served `BatchUpdate` path (a `dw:Selection` node, one `dw:SelectionEntry` per candidate, and a `dw:SelectionIndex` node); the jobs are submitted first, so a selection never names a job that was not submitted. The jobs store has its own redb, so one batch cannot span the graph write and the job submit; the graph write is the durable path and the jobs are the engine's own idempotent work.
- Decision: the next tick orders types by the newest run in the index node (`lastRun`), replacing the in-process run map; the index also lists the selection ids newest first, so the served read and the pruning need no node enumeration. The cap is `downtime.selections_keep`.
- Decision: a candidate is a type whose window is open at the tick. `window_closed` is recorded only when another type was a candidate, so an idle engine with a closed idle window writes nothing.
- Decision: the served `DowntimeSelections` page is keyed by the selection id (the cursor names the last selection of the page). The selections are graph nodes of the commons graph, not rows of a redb table, so the redb row-scan page of `keyset_page.rs` does not apply; the commons graph holds engine-owned scheduler facts only, so the read is gated by `jobs:read` and reads no tenant row.
- Decision (DK-DOWNTIME-REAL-WORK): a downtime job slice runs the intent's served operation through the one downtime work seam (`DowntimeWork`, installed by the server) and its CPU use is the elapsed time of that work. `live_artifact_refresh` refreshes the commons graph's live artifact with the oldest last attempt through the served `LiveArtifactRefresh` under the engine's downtime identity; an artifact another principal owns is refused `ACCESS_DENIED` by that method and skipped; when no artifact is left the slice reports `drained` and the job ends. `plan_evolution` has no served planning step in the engine (policy evolution records and relates, it never plans), so it is not a registered type; registration refuses any intent without served work as `DOWNTIME_INTENT_UNSERVED`.
- Decision: slice length is `downtime.slice_ms` (default 100); the pause poll period is a tenth of `downtime.preempt_deadline_ms`; both replace constants in the job body.
- Decision: the L4 test widens the window through the registry knob (`--knob downtime.window_cpu_permille=1000` on the server command line, the `extra_args` of the served-test `server` helper), never an environment variable. The generic `MEDULLA_<NAME>` fallback inside `load_knobs` is removed by the knob sweep (KNOB-SWEEP, CONTRACT-R063), not by this row; this test no longer depends on it.
- Decision: a tick submits at most one job per type while an earlier job of that type (policy purpose `downtime:<type iri>`) is not terminal; the selection still names the type, but no lease is taken and no job is submitted for it. Proven by `two_ticks_in_an_open_window_submit_one_job`.
- Open for the gate: the core ontology catalog byte pins (`downtime-work@1`, `state-machines`) and the wire vectors and `schema_digest` for `DowntimeSelections` need a refresh (WIRE-REGEN, LAYOUT-REGEN).
## Job executor: stop reasons, failure codes and the cancel reason (R082.5, R082.7, R082.8)

The executor ends every attempt through one function, `settle_claim`, which both the worker slots and the tests call. The status tick and the lease renewal both derive from the claim's own lease (`min(jobs.status_tick_max_ms, lease_ms / jobs.status_tick_lease_divisor)` (registry knobs, defaults 5000 and 3)), and a stop reason, once set, is not overwritten by a later tick.

A cancelled job needs a reason (`kernel_cancelled`), but `JobState::Cancelled` stores no reason and adding one would change the stored layout. The reason is therefore the closing stage label of the checkpoint the cancelled state already carries (`mark_cancelled_fenced` now takes it), so no upgrade step is needed.

Failure codes are the closed `FAILURE_CODES` set; `record_failure` refuses anything outside it. The terminal failure record drops the checkpoint state blob because that blob is the input payload. A kernel failure is a panicked kernel task; the only way to provoke it deterministically is a `cfg(test)` fault set keyed by job id, which a release build does not contain.

The status-tick ceiling and divisor are the registry knobs `jobs.status_tick_max_ms` (default 5000, range [10, 60000]) and `jobs.status_tick_lease_divisor` (default 3, range [2, 20]), read where the tick is computed and listed in the knob table. Decision-plane thresholds do not apply: these are engine operation limits.

The remote worker claim bounds (R082.10) are the registry knobs `jobs.worker_claim_max_capabilities` (default 64, range [1, 1024]) and `jobs.worker_capability_max_len` (default 128 bytes, range [1, 1024]), read in `handle_worker_claim` and listed in the knob table; the earlier constant is removed (DK-R082.10-CAP-KNOB). The stage input of the served tests is a result whose rows, identity and content digest are computed by the test from the published rules: the Rust test builds it with the engine's own association result constructor over the claimed job, the Python test recomputes the digest from the encoded schema and rows. The Rust test gives the state a redb graph backend because publication commits through the authoritative graph gateway.

The remote worker input refusals carry their own registered codes, declared in the `AnalyticsJob` error contract: `WORKER_CAPABILITIES_INVALID` (claim bounds), `WORKER_CHECKPOINT_INVALID`, `WORKER_FAILURE_CODE_INVALID` and `WORKER_INSTANCE_INVALID`; a publication outside the Publishing state is a `CONFLICT` (R082.10). The L3 and L4 sequence tests assert each code.
## R042.7: engine parity bench gate

Decision: `scripts/engine_parity_bench_gate.py` is a standalone script (the `bench_gate.py` loader reads criterion output, a different artifact, so nothing is shared). It reads `target/engine_parity/records.jsonl`, requires the five workloads of the parity suite (`single_row_commit`, `concurrent_64`, `shards_8`, `bulk_load`, `conflict_heavy`) and refuses with typed codes `BENCH_WORKLOAD_MISSING`, `BENCH_RECORD_INELIGIBLE` (no `host`, `revision` or `data_digest`) and `BENCH_RECORDS_UNREADABLE`. The baseline-regression check (`BENCH_REGRESSION`) belongs to MEDULLA-ENGINE-PARITY-R005.2 and extends this script.

## R035.5: query cancellation

Decision: cancellation uses the pgwire `ConnectionManager`/`ConnectionHandle` mechanism rather than a second token map in `WireSession`: pgwire races every simple and extended statement against the connection's handle, so the running statement future is dropped at its next await and the client sees 57014. One manager lives for the listener (built in `serve_prepared`), the startup handler registers each connection, and `EngineCancelHandler` logs and ignores a pair that matches no connection. Pid is a counter; the secret is 32 random bits from the OS random source (256 bits on protocol 3.2). The long statement the proof needs is `SELECT pg_sleep(<seconds>)`, a Postgres statement handled in `WireSession::execute` before classification (0 to 86400 s, otherwise refused); it awaits, so it is cancellable. The listener builds a fail-closed `PgCompatibilityMatrix` and declares only the surfaces it serves; `EngineCapabilities::wire_features_of` derives `wire_features` from that same matrix (the listener never reports a surface it did not declare), so `Cancellation` shows in the report; the served `EngineCapabilities` method itself is MEDULLA-ENGINE-PARITY-R020.2 and reads it.

## R035.4 and R035.6: EXPLAIN and pooler transaction mode

Decision: plain `EXPLAIN` already reaches the DataFusion planner as a read, so the plan rows (`logical_plan`, `physical_plan`) come from the one planner and nothing is re-implemented. `EXPLAIN ANALYZE` is checked before anything runs: only a statement the classifier calls a read executes, anything else is refused with `FEATURE_NOT_SUPPORTED` (0A000). The read path stages no write, so it is the read-only execution the row names. The summary row carries the wall-clock `elapsed_ms` and the root operator's `output_rows`.

Decision: pooler transaction mode is a closed two-value enum on the session, not a flag; its default is the registry knob `wire.pooler_transaction_mode` and `SET medulla.pooler_mode` overrides it per connection (the owner of a pooler configures its backends this way; no environment variable). Prepared statements are invalidated by a generation counter read at `Parse` and checked at execute and describe, because the protocol library offers no way to enumerate named statements; a stale statement is refused with the typed `PREPARED_STATEMENT_UNKNOWN` (26000), as PostgreSQL does for a missing statement. `DISCARD ALL` returns the graph to the connection's startup database, clears isolation defaults, the copy state and the aborted flag, and keeps the pooling mode. In transaction mode a transaction's end (`COMMIT`, `ROLLBACK`) also advances the generation. `ParameterStatus` and `pg_settings` both take `server_version` from the one function `EngineCapabilities::server_version_of`, which the capability report uses too; there is no second version constant.
Decision: pooler transaction mode is a closed two-value enum on the session, not a flag; its default is the registry knob `wire.pooler_transaction_mode` and `SET medulla.pooler_mode` overrides it per connection (the owner of a pooler configures its backends this way; no environment variable). Prepared statements are invalidated by a generation counter read at `Parse` and checked at execute and describe, because the protocol library offers no way to enumerate named statements; a stale statement is refused with the typed `PREPARED_STATEMENT_UNKNOWN` (26000), as PostgreSQL does for a missing statement. `DISCARD ALL` returns the graph to the connection's startup database, clears isolation defaults, the copy state and the aborted flag, and keeps the pooling mode. In transaction mode a transaction's end (`COMMIT`, `ROLLBACK`) also advances the generation. `ParameterStatus` takes `server_version` from the one constant `WIRE_SERVER_VERSION`, which `pg_settings` reads too.

## R039.4.2: the paired consumption bench seam

Decision: `plan_pair` stays private and the durable shard fixtures stay in the store, so the bench reaches the real three-row transaction through one seam, `control_lease::paired_bench::PairedConsumptionScenario`, compiled only under `test-support` (or `cfg(test)`). The root crate gets the cargo feature `bench-claims` (it enables `medulla-work-store/test-support`) and the `paired_pending_consumption` group runs after the engine scenarios of `redb_write_latency_bench` when that feature is on, so a default or `full` build never carries the seam. Registration goes through the `IssueControlLease` applier and the measured call is the `TransitionControlLease` consumption, committed on a real redb shard. The budget applies to the `redb_commit` commit phase p99 (10 ms); the group also prints the end-to-end and per-phase numbers and fails the process when the commit-phase p99 reaches the budget. Driving the same group through the served path stays possible once ARBORA-A2A-R005.2.1 lands; the seam is not a second path, it is the same applier the served handler calls.

## R038.3: how a namespace declares its durability class

Decision: a declaration is one row of the existing KV table under a reserved namespace that served callers cannot name (a served namespace is always the opaque tenant- and principal-keyed key), so it needs no layout change and no upgrade step, and it commits through the same admitted batch as every KV write. The redeclaration rule is one function, `DurabilityClass::admit_redeclaration`: a class is an attribute that is only ever strengthened, so it has no lifecycle machine; the function is the only writer-side check and returns a typed mismatch naming both classes. An undeclared namespace that already holds rows is treated as `sync`, so acknowledged durable data is never relabelled `ephemeral`. An ephemeral namespace is served from process memory with a process-lifetime version clock seeded from wall time, so a version never repeats across a restart. Wire vectors need a refresh (WIRE-REGEN) for the two new methods; models.py was edited by hand in the same commit.

## R038.100: the deployment profile

Decision: `deployment.profile` is a typed two-value enum, parsed in one place (`DeploymentProfile::parse_declared`) and refused with the registered `DEPLOYMENT_PROFILE_UNKNOWN`; it is deployment topology, so a startup argument is its right home (KNOB-SWEEP category a). The profile never selects a durability class: embedded and served share the one declaration row and the one redeclaration rule, so a class cannot differ between profiles. The embedded engine gains the key-value surface in process (declare, put, get) rather than a second store; it opens the same `kv.redb`. An in-process engine refuses the `served` value and the server binary refuses `embedded`, each with the same registered code, so a profile is never silently reinterpreted.
## R042.2 and R064: benchmark receipts (design calls)

- Both benches write through `benches/common/claim.rs::write_receipt`; the receipt shapes (`CrossModalCase`, `ShapeBenchCase`) live in `benches/common/latency_receipt.rs`, beside `wire_receipt.rs`, so the benches and `tests/bench_claim_cli.rs` share one definition.
- The first nightly run records the p99 target (1.2 times the measured p99) in `benches/receipts/targets.json`; a later run exits non-zero when its p99 exceeds the recorded target. Existing targets are never overwritten.
- R064: parse-per-write is `validate`; the memoized variant is `CompiledShapes` (root shapes parsed once). Adoption needs a p50 win of at least 20 percent; the bench exits non-zero without it, which is the signal to delete `CompiledShapes`. Wire-vector refresh is not needed (no wire change).
- Bindings name tests only: the bench targets (shape_validation, hybrid_queries) and the `write_receipt` helper are not test functions, so a test binding to them makes the checker run every bench target with that filter. R064 and R042.2 bind the `tests/bench_claim_cli.rs` receipt tests; the benches stay nightly targets.
R875.2 (work item reply outcomes): the commit and cancel replies answer under `outcome` (`WorkItemCommitOutcome`, `WorkItemCancelOutcome`). Hard cutover, no alias. No stored data needs an upgrade step. The only engine reader of the key is the terminal outbox check at commit time, and it reads the reply that the same commit produced. Committed batch results are kept as opaque bytes and compared byte for byte at recovery, never decoded by key. An idempotent retry of a request committed before the upgrade returns its original bytes, which is the documented operator upgrade boundary.
Decision (EP-R035-LIVE-STATE): the engine has no lock table, so `pg_locks` lists live transaction state: one row per open transaction that holds a write intent (a buffered table or graph write), from the same board. The row text no longer names `lock_recovery.rs`, which only holds poisoned-lock recovery helpers. The relations are registered only on the uncached wire read path (`exec_sql_typed_with_tables_live`), never in a cached context, because process state would go stale in a cache; the served `Method::Sql` path does not offer them rather than offering them empty. `query_start` is rendered as UTC text, not a timestamp column, so the wire encodes it without a second type path. Admin is `IsolationLayer::has_admin_capability` for the verified actor, the check `require_admin_capability` applies; a carrier-scope admin flag is not used because no native SQL login carries it. A write to a view is refused before the classifier, from the statement head, with the typed code `SYSTEM_VIEW_NOT_WRITABLE` mapped to SQLSTATE `42809` in the one table. The surfaces are registered in `listener_matrix` and reported as `WireFeature::PgStatViews` and `WireFeature::PgLocks`; the wire vectors need the WIRE-REGEN refresh because the wire feature list changed. No stored layout changes: LAYOUT-REGEN is not needed.

## R035.2: pg_dump and pg_restore

Decision: the dump client's probes are answered in `crates/medulla-wire/src/pgwire/dump.rs`, matched on the statement text the client sends (one `DumpProbe` per probe, derived from the client's own query strings). The dump client sends every probe as a simple query, so the hook is the simple-query handler, ahead of the session; a statement that is not a probe falls through to the session untouched. The relation, column, index and foreign-key probes read `pg_class`, `pg_attribute`, `pg_index` and `pg_constraint` through the session's authorised read path (the same relations MEDULLA-ENGINE-PARITY-R008.1 serves), so a caller dumps only what its authority can read and nothing is keyed by a bare name. A probe about an object kind the engine has none of (triggers, rules, policies, publications, user types and functions, extensions) is answered with the empty relation, which is the true catalog state.

Decision: the restore side needs no new write path. `pg_restore` replays the classifier's DDL and `COPY ... FROM STDIN`, which commit through `GraphTxn`. The dump file's statement kinds the classifier does not serve (`UNSUPPORTED_RESTORE_FORMS`) are refused with SQLSTATE `0A000` and the code `FEATURE_NOT_SUPPORTED`; a statement the classifier accepts is never intercepted, so a form the engine later serves needs no change here.

Decision: the catalog reports the exact column types, so a dump spells every column from `format_type(atttypid, atttypmod)`. The type oid and modifier come from the stored `ColumnType` (integer, bigint, real, double precision, numeric(p,s), timestamp with and without time zone, uuid, jsonb, bytea, vector(n) and the six array types), the spelling is one function (`format_type_name`) shared by the `format_type` scalar and the dump, and a property test proves each spelling parses back to the same `ColumnType`. `format_type(20, -1)` therefore spells `bigint`, as PostgreSQL does. `NOT NULL` is `!nullable || primary_key`; the dump names the not-null constraint `<table>_<column>_not_null`, the name the client derives itself, so it writes a plain `NOT NULL`. A `SERIAL` column's default is `nextval('public.<table>_<column>_seq'::regclass)`, which the classifier maps back to a serial column on restore. Check constraints are rendered by one function per shape (column check, table check) into the `CHECK (...)` text of `pg_constraint.condef`; a column check restores as a named table check and renders to the same text, so the catalog rows are equal across the round trip.

Decision: the sequence of a serial column is its allocator. It is listed as a `relkind = 'S'` relation with a `pg_sequence` row (`seqlast` is the last issued value) and an automatic `pg_depend` row to its owning column, and it is readable as a one-row relation. A dump file's `CREATE SEQUENCE` and `ALTER SEQUENCE ... OWNED BY` are accepted only when they name the sequence of an existing serial column (anything else goes to the classifier, which refuses it); `setval` moves the allocator through the authorised table path with the `Insert` privilege and only forward, because the allocator value is also the next physical row id and a lower value would overwrite a stored row (typed `SEQUENCE_REWIND`). Stand-alone named sequences (R013.1) have no SQL surface yet and are not part of this dump.

Open item for the gate: `LAYOUT-REGEN` is not needed (no stored layout changed); the dump probes for sequences follow the dump client's `getSequences` shapes (per-sequence form and the all-sequences form) and are verified by the served test with the real client binaries.
## The KV-cache tier ladder keeps one durable copy (MEDULLA-DURABLE-KERNEL-R083.5, R083.6)

- Decision: the cold rung is the namespace `kvcache-blocks` of the `kv` table (the namespace the
  block routes already used), not `kvcache.blocks`. A block's durable copy is that one row; a
  hit keeps the row (a retained cold copy) so a later demotion needs no second write.
- Decision: the `eg_kvcache_cold` table and `RedbColdStore` are removed with the crate's
  `durable` feature and its redb, storage and transaction dependencies. No production code wrote
  the table. The removal is an offline upgrade (`KV_BEFORE_NAMESPACE_COLD`, CQ-ORCH-R810.4):
  lineage row, frozen predecessor digest, one `offline_upgrade!` row, and a retired table
  contract that serves predecessor validation only. The upgrade deletes the table in the same
  commit that bumps the manifest and keeps every `kv` row. A normal open of the old layout is
  refused with `KV_FORMAT_UPGRADE_REQUIRED`.
- Decision: the graph topology arrives with the version bump (`PUT /kv/version/<n>` body
  `{"edges":[[a,b],..]}`) and prefetch neighbors arrive with the GET (`x-medulla-neighbors`).
  Importance derived from the topology lives only in the ladder (`clear_importance`, then
  `apply_importance_scores`) and is never a stored row, so a restart recomputes it at the next
  bump.
- LAYOUT-REGEN needed: the compiled `Kv` layout digest changed. The pin in
  `pinned_layout_digest(Kv)` still holds the predecessor's value; the gate fixer runs the
  digest-printing test and replaces it.
- R099.5: `MultiGraphBatchReport.fanout` (serde default empty, so stored saga results from before the field still decode) holds one `ShardFanoutReport` per sub-batch in request order; the shard tasks return the request index and the report sorts on it, because group order is by shard. A failed sub-batch's code is the registered code in front of its refusal text, or the internal code when the answer carried none. Metrics are `medulla_shard_writer_inflight{shard}` and `medulla_shard_writer_commit_seconds{shard}`, labelled by shard index (bounded by k), recorded around each dispatch. The hand-maintained `models.py` gains `ShardFanoutReport` and `ShardFanoutOutcome`; the wire vectors need a refresh (WIRE-REGEN: `MEDULLA_WRITE_VECTORS=1 cargo test -p medulla-capabilities --test wire_vectors`). The R099.3 cross-tenant case sends the request as a verified caller of another tenant and asserts nothing lands in the graphs and every sub-batch reports a failure.
- R099.4: the decoder returns plain text, so the served arm `multi_graph_batch_update` prefixes every decode refusal with the registered `INVALID_ARGUMENT` code (count over the cap, duplicate or invalid graph identifier); the wire never carried an uncoded refusal before. A failed sub-batch keeps `failed_outcome`'s stable code from R099.5.
- R099.6: the served test sets the shard count through the existing `MEDULLA_REDB_SHARDS` topology variable before the backend opens (as the shard-writer-width test does), seeds 64 graphs of 5,000 nodes, picks one graph per shard from the replies' `shard_index`, and asserts the summed `commit_us` exceeds 1.5 times the request wall time (integer percent arithmetic, no float). The nightly bench `benches/multi_graph_fanout.rs` drives the same served request for 30 samples and asserts p99 under 500 ms.
