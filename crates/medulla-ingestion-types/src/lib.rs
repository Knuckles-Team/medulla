//! medulla-ingestion-types — the ingestion slice of the contract layer
//! (MEDULLA-SDLC-R021.6): parse and index results, ingest rules and records,
//! knowledge atoms and streams, and source governance. It depends on
//! `medulla-types` only.

// MEDULLA-TYPED-PACKS-R804.1 — code health records.
pub mod code_health;
// MEDULLA-INGEST-WORKER-R001.1006: data-prep declaration model.
pub mod data_prep_declaration;
// MEDULLA-INGEST-WORKER-R701.1 — extracted facts, candidate claims and evidence spans.
pub mod extracted_fact;
// MEDULLA-REPO-INGEST-R002.650: ingest capability types.
pub mod ingest_capabilities;
// MEDULLA-INGEST-WORKER-R605.1: drains, page checkpoints, graph routes, stage profiles.
pub mod ingest_drain;
// MEDULLA-REPO-INGEST-R002.100: independent ingest readback receipt.
pub mod ingest_readback;
// MEDULLA-INGEST-WORKER-R900.4/R901.4/R902.4/R903.4/R904.4/R701.2/R701.4/R002.4/R003.4 — pure ingest refusal rules.
pub mod ingest_rules;
// Parse, index and screen-observation wire results.
pub mod ingestion_wire;
// MEDULLA-INGEST-WORKER-R003.1 — typed knowledge atoms with mandatory provenance.
pub mod knowledge_atom;
// The knowledge stream wire types, gated `knowledge-batch`.
#[cfg(feature = "knowledge-batch")]
pub mod knowledge_stream;
// MEDULLA-INGEST-WORKER-R002.1/.2 — induced ontology proposals and admission scores.
pub mod ontology_proposal;
// MEDULLA-REPO-INGEST-R002.1.1: skill-document compilation records.
pub mod skill_plan;
// MEDULLA-INGEST-WORKER-R900.1/R902.1/R903.1/R904.1 — source governance records.
pub mod source_governance;
