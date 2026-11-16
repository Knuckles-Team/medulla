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
