//! medulla-rdf — native RDF/SPARQL surface over the medulla property-graph
//! (Lane W increment 1: W1 `CONCEPT:MEDULLA-KG.ontology.kg-native-rdf-sparql`, W2 `CONCEPT:MEDULLA-KG.ontology.concept-11`).
//!
//! This is the productionized form of the `spike/rdf-owl` feasibility spike. It
//! proves — and now ships — that RDF/SPARQL sit NATIVELY over the engine's
//! property-graph + redb substrate (NOT as a Python layer):
//!
//!   * `mapping` (W1) — the RDF dataset ⇄ property-graph mapping plus Turtle /
//!     N-Triples parse + serialize round-trip. A resource object becomes a typed
//!     edge `{relationship: predicate}`; a literal object becomes a typed JSON property
//!     cell preserving its xsd datatype + `@lang`; `rdf:type` folds into the engine
//!     `type` label; a named graph is a graph in the registry.
//!   * multi-valued literals live in the same authoritative node image under a
//!     reserved typed property, so RDF has no secondary commit authority.
//!   * `sparql` (W2) — `spargebra`'s SPARQL 1.1 algebra COMPILED to scans over the
//!     engine's `GraphView` (BGP + FILTER + OPTIONAL + UNION + basic property
//!     paths). No second copy of the graph, no embedded oxigraph store.
//!
//! Pi contract: every dep is feature-gated OFF by default, so a `default`/`pi`
//! build links none of oxrdf/oxttl/spargebra (all pure-Rust regardless).

/// Re-export the oxrdf term model so callers (the engine handler) can name `Triple`
/// etc. without taking their own oxrdf dependency.
#[cfg(feature = "rdf")]
pub use oxrdf;

#[cfg(feature = "rdf")]
pub mod mapping;

/// MEDULLA-FEDERATED-QUERY-R072.1 — the typed, read-only model of ontology-driven
/// cross-source coverage: a [`source_selection::CoverageResult`] names, per
/// queried class, the approved virtual mapping that serves it (directly or via an
/// entailed `subClassOf` superclass), the class itself as uncovered, or a
/// disconnected class outside the hop budget — with every selection's premises
/// (label/relation/mapping facts plus source generation). No external deps, so it
/// is not feature-gated. Plans no execution: see the module doc for the parallel
/// R073 mapping-type lane this builds toward.
pub mod source_selection;

/// MEDULLA-CQ-XSRC-R006.5 — Medulla's side of the served supply-chain impact
/// method: plans the question's ontology classes through
/// [`source_selection::select_sources`] and assembles the typed
/// `SupplyChainImpactReport` (medulla-types) from the planned rows and premises.
/// The served intent tool itself is cross-repo (owner: Arbora).
pub mod supply_chain_impact;

/// Pure connector-pack RDF preprocessing shared with admission.
#[cfg(feature = "rdf")]
pub mod pack;

#[cfg(feature = "rdf-xml")]
mod rdfxml;

/// MEDULLA-300 — the write-time constraint-guard hook the commit path invokes. Defined here
/// (below the SHACL/ICV engine in the DAG) so an upper layer — medulla-shacl's ICV policy —
/// can IMPLEMENT it without a dependency cycle. See [`update::execute`].
#[cfg(feature = "rdf")]
pub mod guard;

/// MEDULLA-136 — hand-rolled JSON-LD 1.1 serialization + parse over the medulla-rdf term model,
/// built directly on `serde_json` (NO heavy JSON-LD crate). Expanded + compacted
/// (`@context`) writers and an expanded/compacted parser (`@id`/`@type`/`@value`/
/// `@graph`). Behind `rdf` — so it links under plain `rdf`/`sparql` (and is pi-safe),
/// unlike the `oxjsonld`-backed [`mapping::to_jsonld`] (out-of-pi `json-ld` feature).
#[cfg(feature = "rdf")]
pub mod jsonld;

/// Re-export the spargebra SPARQL algebra/term/update model so the engine (the
/// `/sparql` HTTP endpoint) can name `Update` / `GraphUpdateOperation` without taking
/// its own spargebra dependency.
#[cfg(feature = "sparql")]
pub use spargebra;

#[cfg(feature = "sparql")]
pub mod sparql;

/// MEDULLA-101 — OBDA virtual graphs / R2RML (Phase Q). Expose a FOREIGN tabular source as a
/// virtual, SPARQL-queryable RDF graph: an R2RML-style [`obda::TriplesMap`] (subject-IRI
/// template + predicate→object maps) turns rows into triples ON DEMAND, and a SPARQL query
/// over a [`obda::VirtualGraph`] pulls only the query-relevant predicates/columns from the
/// backing [`obda::ObdaSource`] — never materializing the whole dataset. Behind `sparql`
/// (needs the evaluator + oxrdf term model). CONCEPT:MEDULLA-KG.ontology.foreign-source-seam.
#[cfg(feature = "sparql")]
pub mod obda;

/// SPARQL 1.1 UPDATE executed over the native property-graph write ops (CONCEPT:MEDULLA-KG.query.named-graph-support):
/// INSERT/DELETE DATA, DELETE/INSERT … WHERE, CLEAR/CREATE/DROP GRAPH, with named-graph
/// routing through the `GraphStore` trait. Behind `sparql` (needs spargebra's Update).
#[cfg(feature = "sparql")]
pub mod update;

/// OGC GeoSPARQL baseline (CONCEPT:MEDULLA-KG.ontology.concept-10): the `geo:`/`geof:` vocabulary, `geo:wktLiteral`
/// parsing, and the core `geof:` spatial FILTER functions lowered onto the pure-Rust
/// `medulla-geo` crate. Behind `geosparql` (implies `sparql` + pulls `medulla-geo`); OUT of `pi`.
#[cfg(feature = "geosparql")]
pub mod geosparql;




