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
