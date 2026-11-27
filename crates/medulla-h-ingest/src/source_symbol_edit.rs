//! `SubmitSymbolEdit` before the accept path (MEDULLA-SCM-R013.4).
//!
//! `prepare` runs the order the row fixes: project the edit to file bytes
//! (`project_symbol_edit`, R013.2), re-parse and verify the round trip
//! (`verify_roundtrip`, R013.3), and only then write the new blobs and build the
//! `SubmitChangesetRequest` the one accept path of MEDULLA-SCM-R003 takes. A
//! refusal at any step writes no blob and no changeset.

use std::collections::BTreeMap;

use medulla_compute::ast::symbol::ParseResult;
use medulla_compute::source_symbol_edit::{
    project_symbol_edit, BlobAccess, ProjectionRefusal, SymbolResolver,
};
use medulla_enrich::symbol_edit_verify::{parse_clean, verify_roundtrip, OpVerdict, RoundtripRefusal};
use medulla_source_types::lifecycle::sha256_hex;
use medulla_source_types::source_changeset::{
    PathOp, SubmitChangesetRequest, SubmittedBlobRef, SubmittedPath,
};
use medulla_source_types::source_symbol_edit::{
    SymbolEdit, SymbolEditEvidence, SymbolEditKind, SymbolEditOpEvidence,
};

use crate::source_changeset::BlobIo;

/// Why a symbol edit produced no changeset request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SymbolEditRefusal {
    /// The projection of the source refused.
    Projection(ProjectionRefusal),
    /// The round trip of the edit refused.
    Roundtrip(RoundtripRefusal),
    /// The blob store failed.
    Store(String),
}

impl SymbolEditRefusal {
    /// The stable code, one of [`REFUSALS`]; a store failure has none.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::Projection(ProjectionRefusal::SymbolUnknown(_)) => "EDIT_SYMBOL_UNKNOWN",
            Self::Projection(ProjectionRefusal::BaseStale(_)) => "EDIT_BASE_STALE",
            Self::Projection(ProjectionRefusal::OpsOverlap(_)) => "EDIT_OPS_OVERLAP",
            Self::Projection(ProjectionRefusal::NoOps) => "EDIT_NO_OPS",
            Self::Projection(ProjectionRefusal::BlobUnreadable(_)) => "EDIT_BLOB_UNREADABLE",
            Self::Roundtrip(RoundtripRefusal::Unparseable { .. }) => "EDIT_UNPARSEABLE",
            Self::Roundtrip(RoundtripRefusal::Mismatch { .. }) => "EDIT_ROUNDTRIP_MISMATCH",
            Self::Store(_) => return None,
        })
    }

    /// The refusal text: the stable code first, then the detail.
    pub fn message(&self) -> String {
        match self {
            Self::Projection(refusal) => refusal.to_string(),
            Self::Roundtrip(refusal) => refusal.to_string(),
            Self::Store(error) => error.clone(),
        }
    }
}

/// The seven stable refusal codes of a served symbol edit (MEDULLA-SCM-R013.5). Each code is
/// declared here once; `SymbolEditRefusal::code` maps every refusal onto one of them.
pub const REFUSALS: &[medulla_types::refusal_code::RefusalCode] = &[
    medulla_types::refusal_code::RefusalCode::new("EDIT_BASE_STALE"),
    medulla_types::refusal_code::RefusalCode::new("EDIT_OPS_OVERLAP"),
    medulla_types::refusal_code::RefusalCode::new("EDIT_UNPARSEABLE"),
    medulla_types::refusal_code::RefusalCode::new("EDIT_ROUNDTRIP_MISMATCH"),
    medulla_types::refusal_code::RefusalCode::new("EDIT_SYMBOL_UNKNOWN"),
    medulla_types::refusal_code::RefusalCode::new("EDIT_NO_OPS"),
    medulla_types::refusal_code::RefusalCode::new("EDIT_BLOB_UNREADABLE"),
];

/// What a projected and verified edit becomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedEdit {
    /// The request the accept path takes.
    pub request: SubmitChangesetRequest,
    /// The re-parse verdict of every op, in op order.
    pub verdicts: Vec<OpVerdict>,
    /// What the accept path stores inside the changeset (MEDULLA-SCM-R013.6).
    pub evidence: SymbolEditEvidence,
}

/// Reads old blobs through the store and holds new bytes in memory until the check passes.
struct StagedBlobs<'a> {
    blobs: &'a dyn BlobIo,
    staged: BTreeMap<String, Vec<u8>>,
}

impl BlobAccess for StagedBlobs<'_> {
    fn read(&self, digest: &str) -> Option<Vec<u8>> {
        self.blobs.read(digest).ok()
    }

    fn write(&mut self, bytes: &[u8]) -> String {
        let digest = sha256_hex(bytes);
        self.staged.insert(digest.clone(), bytes.to_vec());
        digest
    }
}

/// Project, verify and build the changeset request of one symbol edit.
pub fn prepare(
    edit: &SymbolEdit,
    author_principal: &str,
    created_at: u64,
    resolver: &dyn SymbolResolver,
    blobs: &dyn BlobIo,
) -> Result<PreparedEdit, SymbolEditRefusal> {
    let mut staged = StagedBlobs {
        blobs,
        staged: BTreeMap::new(),
    };
    let projected = project_symbol_edit(edit, author_principal, created_at, resolver, &mut staged)
        .map_err(SymbolEditRefusal::Projection)?;
    // The base parse of each edited file and the projected bytes, in path order.
    let mut old: Vec<ParseResult> = Vec::new();
    let mut new_files: Vec<(String, Vec<u8>)> = Vec::new();
    for changed in &projected.changed_paths {
        let old_digest = changed.old_blob.as_deref().unwrap_or_default();
        let old_bytes = staged
            .read(old_digest)
            .ok_or_else(|| SymbolEditRefusal::Projection(ProjectionRefusal::BlobUnreadable(old_digest.to_owned())))?;
        old.push(parse_clean(&changed.path, &old_bytes).map_err(SymbolEditRefusal::Roundtrip)?);
        let new_digest = changed.new_blob.as_deref().unwrap_or_default();
        let new_bytes = staged.staged.get(new_digest).cloned().unwrap_or_default();
        new_files.push((changed.path.clone(), new_bytes));
    }
    let verdicts = verify_roundtrip(edit, &old, &new_files).map_err(SymbolEditRefusal::Roundtrip)?;
    // The check held: the new bytes are written now, and only now.
    let mut paths = Vec::with_capacity(projected.changed_paths.len());
    for (changed, (_, bytes)) in projected.changed_paths.iter().zip(&new_files) {
        let stored: SubmittedBlobRef = blobs.store(bytes).map_err(SymbolEditRefusal::Store)?;
        paths.push(SubmittedPath {
            path: changed.path.clone(),
            op: PathOp::Modify,
            blob: Some(stored),
            mode: changed.mode,
            renamed_from: None,
        });
    }
    let evidence = SymbolEditEvidence {
        edit_digest: edit.digest(),
        ops: edit
            .ops
            .iter()
            .map(|op| SymbolEditOpEvidence {
                symbol_id: op.symbol_id.clone(),
                kind: op.kind,
                old_span_digest: op.anchor_span_digest.clone(),
                new_span_digest: sha256_hex(match op.kind {
                    SymbolEditKind::Delete => &[],
                    _ => op.new_text.as_bytes(),
                }),
                // `verify_roundtrip` held for every op, or no changeset is built.
                verdict: "confirmed".to_string(),
            })
            .collect(),
    };
    Ok(PreparedEdit {
        evidence,
        request: SubmitChangesetRequest {
            repo: edit.repo.clone(),
            branch: edit.branch.clone(),
            base_head: Some(edit.base_rev.clone()),
            message: edit.message.clone(),
            paths,
            trailers: None,
            // An empty id is a human edit: the accept path's provenance check sees no run.
            run_trace_id: (!edit.run_trace_id.is_empty()).then(|| edit.run_trace_id.clone()),
        },
        verdicts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_compute::ast::parser::{parse_file, SourceLanguage};
    use medulla_compute::source_symbol_edit::ResolvedSymbol;
    use medulla_source_types::source_symbol_edit::{SymbolEditKind, SymbolEditOp};
    use std::cell::RefCell;

    #[derive(Default)]
    struct Mem(RefCell<BTreeMap<String, Vec<u8>>>);

    impl BlobIo for Mem {
        fn read(&self, digest: &str) -> Result<Vec<u8>, String> {
            self.0.borrow().get(digest).cloned().ok_or_else(|| "absent".to_string())
        }
        fn store(&self, bytes: &[u8]) -> Result<SubmittedBlobRef, String> {
            let digest = sha256_hex(bytes);
            self.0.borrow_mut().insert(digest.clone(), bytes.to_vec());
            Ok(SubmittedBlobRef { digest, size: bytes.len() as u64 })
        }
    }

    struct One(String, ResolvedSymbol);

    impl SymbolResolver for One {
        fn resolve(&self, _rev: &str, symbol_id: &str) -> Option<ResolvedSymbol> {
            (symbol_id == self.0).then(|| self.1.clone())
        }
    }

    const SOURCE: &str = "def alpha():\n    return 1\n\n\ndef beta():\n    return 2\n";

    fn fixture(new_text: &str) -> (SymbolEdit, One, Mem) {
        let parsed = parse_file(SOURCE, "a.py", SourceLanguage::Python);
        let alpha = parsed.symbols.iter().find(|s| s.name == "alpha").expect("alpha");
        let (start, end) = (alpha.byte_start as usize, alpha.byte_end as usize);
        let blobs = Mem::default();
        let stored = blobs.store(SOURCE.as_bytes()).expect("seed");
        let edit = SymbolEdit {
            repo: "r".into(),
            branch: "main".into(),
            base_rev: "c0".into(),
            ops: vec![SymbolEditOp {
                kind: SymbolEditKind::ReplaceSymbol,
                symbol_id: alpha.id.clone(),
                anchor_span_digest: sha256_hex(&SOURCE.as_bytes()[start..end]),
                new_text: new_text.into(),
            }],
            message: "edit".into(),
            run_trace_id: String::new(),
        };
        let resolved = ResolvedSymbol {
            path: "a.py".into(),
            blob_digest: stored.digest,
            span: start..end,
            body_span: None,
            mode: 0o100644,
        };
        (edit.clone(), One(edit.ops[0].symbol_id.clone(), resolved), blobs)
    }

    // spec: MEDULLA-SCM-R013.4
    #[test]
    fn accept_path_runs_after_projection() {
        let (edit, resolver, blobs) = fixture("def alpha():\n    return 42");
        let blobs_before = blobs.0.borrow().len();
        let prepared = prepare(&edit, "p", 1, &resolver, &blobs).expect("prepared");
        assert_eq!(prepared.request.paths.len(), 1);
        assert_eq!(prepared.request.base_head.as_deref(), Some("c0"));
        assert_eq!(prepared.request.run_trace_id, None);
        assert_eq!(prepared.verdicts.len(), 1);
        assert_eq!(blobs.0.borrow().len(), blobs_before + 1);
    }

    // spec: MEDULLA-SCM-R013.4
    #[test]
    fn a_refused_edit_writes_no_blob() {
        let (edit, resolver, blobs) = fixture("def alpha(:\n    return 42");
        let blobs_before = blobs.0.borrow().len();
        let refusal = prepare(&edit, "p", 1, &resolver, &blobs).expect_err("unparseable");
        assert!(refusal.message().starts_with("EDIT_UNPARSEABLE"));
        assert_eq!(blobs.0.borrow().len(), blobs_before);
    }
}
