//! `ModalityContract` retrofit for [`Symbol`] (CONCEPT:E4/X1).
//!
//! Behind the crate's own opt-in `contract` feature (default OFF). `Symbol` contributes a
//! code-symbol address only when its AST identity is already a valid opaque token. Raw file
//! paths and symbol display names never cross the governed evidence boundary.

use medulla_modality::{
    encode_staged, ConformanceTestable, EvidenceAddress, ModalityContract, OpaqueRef, RowSetShape,
    StagedWrite,
};

use crate::ast::symbol::Symbol;

impl ModalityContract for Symbol {
    fn storage_kind(&self) -> &'static str {
        "code"
    }

    /// A parsed symbol is a SOURCE-shaped candidate (keyed by `Symbol::id`), not
    /// intrinsically ranked — unranked, exactly like `medulla-tsdb::SeriesMeta`/
    /// `medulla-geo::Geometry`.
    fn to_rowset(&self, id: &str) -> RowSetShape {
        RowSetShape::unranked(id)
    }

    fn txn_stage(&self, id: &str) -> StagedWrite {
        StagedWrite::put(id, encode_staged(self))
    }

    /// A parsed symbol lands via a batch AST-parse pass (`ast::parser`), not a live
    /// transactional write path today — not (yet) on the CDC/streaming surface,
    /// mirroring `MergeProposal`'s `None`.
    fn cdc_topic(&self) -> Option<&'static str> {
        None
    }

    fn evidence_address(&self) -> Option<EvidenceAddress> {
        let revision_ref = OpaqueRef::scoped("revision", &self.ast_hash).ok()?;
        let symbol_ref = OpaqueRef::scoped("symbol", &self.ast_hash).ok()?;
        Some(EvidenceAddress::CodeSymbol {
            revision_ref,
            symbol_ref,
            start_line: self.line_start,
            end_line: self.line_end,
        })
    }
}

impl ConformanceTestable for Symbol {
    fn conformance_sample() -> Self {
        Symbol {
            id: "sym:deadbeef".to_string(),
            name: "handle_request".to_string(),
            qualified_name: "crate::server::handle_request".to_string(),
            symbol_type: crate::ast::symbol::SymbolType::Function,
            file_path: "src/server.rs".to_string(),
            line_start: 42,
            line_end: 88,
            column: 0,
            ast_hash: "deadbeefdeadbeefdeadbeefdeadbeef".to_string(),
            dependencies: Vec::new(),
            documentation: "Handles an inbound request.".to_string(),
            language: "rust".to_string(),
            is_exported: true,
            annotations: Vec::new(),
            byte_start: 900,
            byte_end: 1500,
        }
    }
}

// `modality_conformance_tests!` expands to a FIXED `mod medulla_modality_conformance`
// name — nested here (rather than invoked a second time at this file's top level,
// where `MergeProposal`'s invocation already claims that name) so the two batteries
// don't collide (CONCEPT:E4/X1).
mod symbol_conformance {
    // The macro's generated conformance battery is itself `#[cfg(test)]`-gated (it
    // expands to nothing in a non-test build), so `Symbol` has no real consumer
    // outside `cfg(test)` here — gate the import the same way to avoid an
    // "unused import" warning under a plain `cargo build`/`clippy` (no `--tests`).
    #[cfg(test)]
    use super::Symbol;

    medulla_modality::modality_conformance_tests!(Symbol);
}

// A direct test of the governed evidence-address mapping itself.
#[cfg(test)]
mod symbol_evidence {
    use super::*;

    #[test]
    fn maps_file_path_and_line_range_losslessly() {
        let sym = Symbol::conformance_sample();
        let address = sym
            .evidence_address()
            .expect("a parsed Symbol always has evidence");
        assert_eq!(
            address,
            EvidenceAddress::CodeSymbol {
                revision_ref: OpaqueRef::scoped("revision", "deadbeefdeadbeefdeadbeefdeadbeef")
                    .unwrap(),
                symbol_ref: OpaqueRef::scoped("symbol", "deadbeefdeadbeefdeadbeefdeadbeef")
                    .unwrap(),
                start_line: 42,
                end_line: 88,
            }
        );
    }
}
