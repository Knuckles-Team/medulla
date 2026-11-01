//! medulla-virtual-graph: Attached sources read as virtual graphs: OBDA mappings, source selection and source authority checks.
//!
//! Level L4 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: unified-data-plane, source-authority.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// MEDULLA-FEDERATED-QUERY-R072.1 — the typed, read-only model of ontology-driven
/// cross-source coverage: a [`source_selection::CoverageResult`] names, per
/// queried class, the approved virtual mapping that serves it (directly or via an
/// entailed `subClassOf` superclass), the class itself as uncovered, or a
/// disconnected class outside the hop budget — with every selection's premises
/// (label/relation/mapping facts plus source generation). No external deps, so it
/// is not feature-gated. Plans no execution: see the module doc for the parallel
/// R073 mapping-type lane this builds toward.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod source_selection;
/// MEDULLA-CQ-XSRC-R006.5 — Medulla's side of the served supply-chain impact
/// method: plans the question's ontology classes through
/// [`source_selection::select_sources`] and assembles the typed
/// `SupplyChainImpactReport` (medulla-types) from the planned rows and premises.
/// The served intent tool itself is cross-repo (owner: Arbora).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod supply_chain_impact;
/// MEDULLA-101 — OBDA virtual graphs / R2RML (Phase Q). Expose a FOREIGN tabular source as a
/// virtual, SPARQL-queryable RDF graph: an R2RML-style [`obda::TriplesMap`] (subject-IRI
/// template + predicate→object maps) turns rows into triples ON DEMAND, and a SPARQL query
/// over a [`obda::VirtualGraph`] pulls only the query-relevant predicates/columns from the
/// backing [`obda::ObdaSource`] — never materializing the whole dataset. Behind `sparql`
/// (needs the evaluator + oxrdf term model). CONCEPT:MEDULLA-KG.ontology.foreign-source-seam.
#[cfg(feature = "sparql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod obda;
