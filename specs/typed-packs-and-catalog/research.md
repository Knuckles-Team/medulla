# typed-packs-and-catalog — Research

## Decision: One Turtle to ObjectModel extractor

**Decision.** A single extractor in the pack compiler (`MEDULLA-TYPED-PACKS-R650.7`) turns the pack ontology Turtle into the `ObjectModel`; it feeds the shape converter and `GraphSchemaClasses.interfaces`. One path, no second model.

## Decision: DiffOntology is a read op of the GraphSchema write method

**Decision.** `GraphSchemaOp::DiffOntology` stays an op of `Method::GraphSchema` and returns its report in the one `GraphSchemaCommitted` body (`ontology_diff`), rather than becoming a second method with its own result type. The method keeps its graph write authority; the op is classified a read in `READ_ONLY_OPS` by its audit token, and the handler installs and writes nothing. The impact snapshot projects RDF resources only, so engine bookkeeping nodes never break an import.

**Why.** A second method would duplicate the route, the capability row and the result contract for one read. The op token ties the classification to the operation, so the two cannot drift.

**Revision bodies.** Only the active revision's body is stored (R103.2), so a diff reads the active revision against a candidate or itself. A change request for a breaking import names the bound importer as owner and reviewer until stewardship assignments for ontologies exist (R802.3).
