# typed-packs-and-catalog — Research

## Decision: One Turtle to ObjectModel extractor

**Decision.** A single extractor in the pack compiler (`MEDULLA-TYPED-PACKS-R650.7`) turns the pack ontology Turtle into the `ObjectModel`; it feeds the shape converter and `GraphSchemaClasses.interfaces`. One path, no second model.

## Decision: DiffOntology is a read op of the GraphSchema write method

**Decision.** `GraphSchemaOp::DiffOntology` stays an op of `Method::GraphSchema` and returns its report in the one `GraphSchemaCommitted` body (`ontology_diff`), rather than becoming a second method with its own result type. The method keeps its graph write authority; the op is classified a read in `READ_ONLY_OPS` by its audit token, and the handler installs and writes nothing. The impact snapshot projects RDF resources only, so engine bookkeeping nodes never break an import.

**Why.** A second method would duplicate the route, the capability row and the result contract for one read. The op token ties the classification to the operation, so the two cannot drift.

**Revision bodies.** Only the active revision's body is stored (R103.2), so a diff reads the active revision against a candidate or itself. A change request for a breaking import names the bound importer as owner and reviewer until stewardship assignments for ontologies exist (R802.3).

## R807: where conformance evidence lives

**Decision.** The `ConformanceEvidence` record is a field of the connector's head row, written by the same admitted commit as the head and changed only by revocation. It is not a new owner table. Its lifecycle is `ConformanceEvidenceMachine` (`Active`, terminal `Revoked`); expiry is the stored `expires_at` compared with the read time, so no clock writes the record. The lifetime is the registry knob `conformance.evidence_ttl_ms`.

**Why.** The row names a record per tenant, connector and immutable revision, and a read of "the current record". A head always names exactly one current revision, so evidence on the head is that record, atomic with the import and keyed by tenant and connector, which makes another tenant's record unreachable by construction. A new owner table would force a layout lineage row, a pinned digest and an offline upgrade step for no extra capability. Old head rows carry no evidence and read as absent. A pack that re-imports unchanged does not renew the evidence; renewal is a new revision.
## Domain pack run (R097.700)

**Decision.** `Method::DomainPack` carries the manifest inline. `Preview` is a catalog-admin read; `Run` commits one `BatchUpdate` through the mutation gateway (`Route::DomainPack`), the route the finance factor record uses. A relation target must be an entity of the same run or a stored node of the writer's tenant; a node of another tenant is never overwritten.

**Why.** There is no second registry for manifests: the request is the manifest, and its `integrity_sha256` is what makes it trustworthy. One gateway batch gives the durable log, audit and CDC the same view as every other write, without minting a `ChangeEnvelope` inside a handler (envelopes are compiled at the request boundary from a verified context).
- Decision (R002.7, R094.2): the blueprint travels in an optional `blueprint` field of the publish request, not inside the `SystemPrompt` facts, because the facts carry only the pin (`body_sha256`, `schema_revision`) and the digest of the definition covers them. The body is canonical JSON (sorted keys, no whitespace) so the pin, the stored bytes and the export agree whatever key order the caller sent. The export carries numbers past double precision as decimal strings with their JSON Pointers, so it is lossless without a second number encoding.
- Decision (R094.3): an import is a publish of the base revision's own draft with only the blueprint pin and body replaced, committed through the one publish path with the base revision as the compare-and-set head. The base is checked against the head before staging (`BLUEPRINT_STALE_BASE`) and again by the owner write, whose `PROMPT_HEAD_STALE` is renamed for the import. A profile refusal (`PROMPT_SCHEMA_INVALID`) is renamed `BLUEPRINT_MALFORMED`. There is no separate operation id: the verified request's idempotency key already makes a retried import one write.
- Decision (R094.8): the receipt fact lives in the existing tenant-scoped write-back idempotency table (`write_back_idempotency`, already documented as one binding per source effect), keyed `source-effect:<component>:<revision>:<receipt digest>`, not in a new table, so the owner layout and its pinned digest do not change. The digest is the key, so a replay is a no-op and the same digest with other bytes is `RECEIPT_DIGEST_REUSED`. The revision existence check reads the committed revision row first (revisions are append-only). The graph edge `cp:appliedToSource` is projected from these rows by R002.12.
- Decision (R002.9): the export and the render share one pinned-prompt reader (`read_pinned_prompt`), so there is one place that checks the revision row, its pin and its engine body. The receipt scope is the verified request tenant, and `receipt_digest` (framed pins, then variables digest, then scope) is one pure function in `medulla-agent-types` so R002.13 recomputes it without a cross-crate call. A schema revision other than the revision's own is `PROMPT_SCHEMA_INVALID`; an unavailable body store is `PROMPT_PRODUCER_UNAVAILABLE`.
- Decision (R094.6): a proposal fact lives in the same write-back idempotency table as the R094.8 receipt, under the `proposal:` key prefix, so no table or layout changes and no second evidence store exists. The row is keyed by payload digest: replay is a no-op, the same digest with another comparison is `PROPOSAL_DIGEST_REUSED`. Authority fields exist on the request only to be refused by name (`PROPOSAL_SELF_APPROVED`).
## R103.6: how a served SPARQL query reads a runtime-imported ontology

**Decision.** `handle_sparql` derives, on every read, a view of the active ontology Turtle of each `SchemaSourceOrigin::Ontology` source composed into the request graph and unions it into the default graph beside the pack graphs. Engine core vocabulary and attached packs are not added, so existing query results do not change.

**Why.** The composed schema is the only record of an imported ontology, and a derived view of it cannot outlive a revision or a withdrawal. No second store is written; the graph's composed digest joins the result-cache key so a cached answer never survives a schema change.

## R105: one parent-catalog link rule, no standalone ontology

**Decision.** Every connector ontology joins the core through `rdfs:subClassOf` edges. One module, `crates/medulla-h-admin/src/ontology_link.rs`, holds the rule for both entry points, `ConnectorPack.Import` and `GraphSchema.ImportOntology`. An `owl:imports` of the parent catalog or one of its modules refuses with `ONTOLOGY_PACK_IMPORTS_CORE`. A declared class with no subclass path to a parent class refuses with `ONTOLOGY_ORPHAN`. Composition keeps no `pack:` exception for a parent import. There is no `standalone` request, no standalone grant, no warn mode and no environment switch.

**Why.** An import edge pulls the whole core into a pack's identity and lets a pack look linked when its classes are not. A subclass edge is the only link a class query can follow. `R105` and `R107` disagreed about a governed `standalone` exception. `R105` wins (decision TYPED-PACKS-NO-STANDALONE): an orphan can never be stored, so a grant has nothing to cover. The parent-import test of `R105.1` now asserts the refusal. Its row named the earlier acceptance only as a test name. The `ontology-import:` pack requirement that names the parent catalog stays a resolved claim (`R105.1`). It is a requirement annotation, not an `owl:imports` edge.

## R107: release contracts without a standalone exception

**Decision.** The grant record and its refusal code are removed with no replacement (hard cutover). `R107.3` and `R107.4` keep only the contract-release half: `diff_release` runs in `run_import` before the commit, a breaking release is held pending with `CONTRACT_BREAK_PENDING` (first `catalog.contract_break_report_max` breaks), a removed class still named by stored data refuses `CLASS_IN_USE`, and a refused or pending release leaves the active version unchanged. There is no `orphan_report` read op.

**Factual fix.** The earlier `R107.3` text said a grant check runs "where `R105` resolves `standalone`". `R105` never had a `standalone` request: its only orphan control was a warn switch, now removed. An orphan report would always be empty, because an orphan class is refused before commit.

## R107.3 and R107.4: where the release lives

**Decision.** The active release summary (version record, tool field paths with declared types, mapping ids) is a field of the connector's head row, written by the import commit that moves the head. A breaking release is stored as `pending_release` on the same row by one compare-and-set write and never moves the head. Ontology and shapes documents of the prior release are read from the head's own published members, not copied into the summary. The version string is the import record id.

**Why.** One row holds the head and both releases, so a pending release cannot outlive or race the head it was diffed against, and queries keep resolving through the active head with no extra read. The presence of `pending_release` is the `pending` state of `PackReleaseMachine`; approval and rejection (R107.6) move it through that machine.

**Factual fixes.** `rejection_for` is not used: the results carry violations directly, because the first N breaks are data, not an error string. A tool removed as a whole is a pack withdrawal, not a field break, so only fields of tools the release keeps are compared. `CLASS_IN_USE` matches graphs by the record id their `pack:<connector>` source was composed from and by the tenant that source records. Medulla is multi-tenant: one store holds many tenants' graphs, and two tenants that import the same archive get the same record id, so the record id alone does not separate them.

**LAYOUT-REGEN needed** is not required: the head row gains optional fields with serde defaults.

## R107.4: which graphs the class-in-use check reads

**Decision.** The check reads only the importing tenant's graphs that compose its `pack:<connector>` source, chosen from catalog metadata without opening any graph. The `Pack` schema-source origin records the verified tenant whose head it attaches (`GraphSchema.AttachPack` and the pack projection both write it). That origin is part of the graph's schema sources, which the durable catalog row (`graph_meta`) already holds. A resident graph answers from its installed sources. A catalog-only graph answers from its catalog row (`PersistenceBackend::graph_catalog_schema_sources`); it is never lazily opened. A source attached before the tenant was recorded counts for every tenant, and records the tenant the next time the same record is attached. A source never moves to another tenant's pack (`SCHEMA_SOURCE_REGRESSION`).

For a candidate that is fully resident, a class is in use when the graph's `type` label index (the `rdf:type` fold) holds it, or when the class node has an incoming `rdf:type` edge (a resource typed more than once keeps only one folded `type`). For a candidate that is catalog-only, hibernated or not fully materialized, a class is in use when the durable node table holds the class node; that read is a point read by node id, and it counts any stored reference to the class. The check runs on the import's blocking thread and reads engine state with a blocking lock acquisition there; it never blocks a runtime worker.

**Why.** The catalog already records each graph's schema sources, so the tenant goes on the source it describes: one record, no second registry. The label index answers a resident graph in time proportional to the classes checked, not to the graph's triples. A graph that is not resident has no type index in memory, and reading the node table by the class node id fails closed: it can refuse a release that an index would allow, never the reverse. The origin's digest bytes are unchanged for a source with no recorded tenant, so stored graphs keep their composed digest.
**WIRE-REGEN needed:** `PackViolationCode` gained `CONTRACT_BREAK_PENDING` and `CLASS_IN_USE` and `ServerErrorCode` the same two; `medulla/generated/connector_pack.py` is edited by hand and the wire vectors need a refresh.

## R001.100 nested schema compatibility

- The pack entry wire shape gains no field. The nested field schema is the entry's existing `input_schema` / `output_schema` document, and `schema_version`, `compat_revision` and the base digest are top-level `x-medulla-*` members of that document, so the section digest already binds them and the pack digest, the archive builder and the generated clients are unchanged (one source of truth, no second registry).
- The active release (`PackRelease.schemas`) records each entry's schema (key: entry URI plus `#in` or `#out`) with its digest and, when it replaced another, the verdict, base digest and changed paths. `judge` runs `judge_schemas` before the release diff: a base digest the active release does not hold is `PACK_PLAN_STALE`, a `Breaking` verdict is `PACK_SCHEMA_INCOMPATIBLE`; both refuse before anything commits and leave the head unchanged. Import dispatches no tool.
- The requirements row named `validate_schema` at `validation.rs:399`; the function is at line 505, corrected in the row (a factual path fix, not a narrowing).
- WIRE-REGEN: `PackViolationCode` gains `PACK_SCHEMA_INCOMPATIBLE` and `PACK_PLAN_STALE`; `medulla/generated/connector_pack.py` is edited by hand in the same commit.

## R002.12 native-component projection
- The pack worker stays a single consumer on the pack import topic (R054), so native component events are drained by a second durable consumer, `native-component-projection`, on the component revision topic. Both are started by `connector_pack_projection::spawn` and share the worker's claim, settle, tenant and service-identity helpers.
- One `PromptRevision` node per component and entry revision (`prompt-revision:<component>:<revision>`): a re-delivered or stale event finds its node and writes nothing, so an old event never overwrites or hides a newer revision. Dependencies become `ComponentRef` nodes with a typed `requires` edge carrying the pinned definition digest.
- The write goes through the existing graph gateway under the projection service identity. The gateway context assembly that `commit_pack_projection` held inline is now `commit_internal_graph_write`, used by both projections (one path, no copy).
- The tenant graph is `component__<digest>` (`native_component_projection_graph_name`), hashed like the pack graph so the name leaks nothing. A failed write that may succeed later leaves the lease unresolved (redelivered); a refused one is rejected as a failed projection and can be rewound through the Agent Library outbox operator.
- The requirements row named `commit_schema_projection` as the admission helper; that helper carries pack documents only, so the row text now names the shared gateway commit. This is a mechanism correction, not a narrowing of the proof.

## R100.3 to R100.5: skill package rendering on read

- A skill's ontology references are the whitespace-separated component ids of its `ontology_refs` front-matter key, recorded by the pack importer as the attribute `skill.ontology_refs`. A reference is defined when a published ontology component of that id exists in the reading tenant; anything else renders as `RENDER_DANGLING_ONTOLOGY_REF` naming the term.
- The listing loads each skill's body from the engine body store before the pure projection runs (`read_skill_body`, behind the `blob` feature; without it no body is readable and the row says `RENDER_BODY_MISSING`). Nothing is cached: every list and lookup renders from what it just read.
- A skill row carries `package` (the rendered package with graph revision and content digest) and `package_refusal` (the typed refusal text when no package could render). The graph revision of a package is the skill's `entry_revision`; its code delta is the pack's `source_revision` and its digest.
- The L3 proofs are hosted in the `medulla-h-admin` served pack harness: `medulla-capabilities` cannot reach `medulla-server-consensus` (decision FQ-R707.6).
- Wire change: `FleetSkillRow` gains `package` and `package_refusal` (hand-edited in `medulla/generated/fleet_catalog.py`); a wire-vector refresh is needed. New codes `RENDER_BODY_MISSING` and `RENDER_DANGLING_ONTOLOGY_REF` are registered in `server_error_code.rs`.
