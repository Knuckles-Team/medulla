# Research: connector competency

## Served vocabulary coverage reads source records through Uql, not SPARQL (MEDULLA-CQ-CONN-R089)

Question: how does the served vocabulary test read back a record it ingested?

Finding: `SourceIngest` writes each record as a property-graph node whose id is
`source:<digest>`. The SPARQL projection (`export_view_triples`) emits only nodes whose id is an
RDF term (`<iri>` or a blank node), so an ingested source record does not appear in served SPARQL.

Decision: the test reads the record through served `Uql` (`MATCH () WHERE type = '<pack class>'`)
and the node properties (`source_record_id`, `source_stream`, `source_updated_at`,
`source_provenance`, and the value under the term IRI). The term's declaration is proved by the
served `ConnectorPack.Import`: a pack class that extends a core class the catalog lacks is refused
`ONTOLOGY_ORPHAN`. Projecting source records into the RDF view is a separate engine feature and
is not needed for the vocabulary proof.

## Virtual-graph provenance (MEDULLA-CQ-CONN-R067.5)

Question: where does a `SparqlVirtual` row get its mapping IRI and fetch time?

Decision: the mapping IRI is declared by the mapping itself as an `rr:constant` object map, so the
engine returns it on every row; the host stamps the fetch time around the call, as the other
contract-kind journeys do.
