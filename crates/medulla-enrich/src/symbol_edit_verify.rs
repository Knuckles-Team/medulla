//! Round-trip check of a projected symbol edit (MEDULLA-SCM-R013.3).
//!
//! The edit was projected to new file bytes by `medulla-compute`. Before the
//! changeset is accepted, each projected file is parsed again and compared with
//! the parse of the base revision: the edited symbols must be what the op kind
//! promises, and every untouched symbol must keep its `ast_hash`. Nothing is
//! written; the verdicts go to the changeset record.

use std::collections::BTreeMap;

use medulla_compute::ast::parser::{parse_file, SourceLanguage};
use medulla_compute::ast::symbol::{ParseResult, Symbol, SymbolType};
use medulla_source_types::source_symbol_edit::{SymbolEdit, SymbolEditKind};

use crate::fingerprint::fingerprint;
use crate::Id;

/// Why a projected edit failed the round-trip check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoundtripRefusal {
    /// A projected file does not parse cleanly (`EDIT_UNPARSEABLE`).
    Unparseable { path: String },
    /// The re-parse differs from the edit's promise or from the base on an
    /// untouched symbol (`EDIT_ROUNDTRIP_MISMATCH`). `path` is empty when an op
    /// names a symbol that is absent from the base parse.
    Mismatch { path: String, symbol: String },
}

impl std::fmt::Display for RoundtripRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unparseable { path } => write!(f, "EDIT_UNPARSEABLE: {path}"),
            Self::Mismatch { path, symbol } => {
                write!(f, "EDIT_ROUNDTRIP_MISMATCH: {path} {symbol}")
            }
        }
    }
}

impl std::error::Error for RoundtripRefusal {}

/// The verdict of one op: the edit held and the new file structure hash
/// (MEDULLA-REPO-INGEST-R603).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpVerdict {
    pub symbol_id: String,
    pub kind: SymbolEditKind,
    pub path: String,
    pub structure_hash: String,
}

pub fn parse_clean(path: &str, bytes: &[u8]) -> Result<ParseResult, RoundtripRefusal> {
    let unparseable = || RoundtripRefusal::Unparseable {
        path: path.to_string(),
    };
    let extension = path.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    let language = SourceLanguage::from_extension(extension).ok_or_else(unparseable)?;
    let source = std::str::from_utf8(bytes).map_err(|_| unparseable())?;
    let parsed = parse_file(source, path, language);
    if parsed.errors.is_empty() {
        Ok(parsed)
    } else {
        Err(unparseable())
    }
}

fn find_old<'a>(old: &'a [ParseResult], symbol_id: &str) -> Option<(&'a str, &'a Symbol)> {
    old.iter().find_map(|file| {
        file.symbols
            .iter()
            .find(|symbol| symbol.id == symbol_id)
            .map(|symbol| (file.file_path.as_str(), symbol))
    })
}

fn structure_hash(path: &str, bytes: &[u8], parsed: &ParseResult) -> Result<String, RoundtripRefusal> {
    let mut signatures: Vec<String> = parsed
        .symbols
        .iter()
        .map(|s| format!("{}\t{}\t{}", s.qualified_name, s.symbol_type, s.ast_hash))
        .collect();
    signatures.sort();
    let id = Id::new(path).map_err(|_| RoundtripRefusal::Unparseable {
        path: path.to_string(),
    })?;
    Ok(fingerprint(id, bytes, Ok(signatures.as_slice()))
        .structure_hash
        .unwrap_or_default())
}

/// Check `projected` (path, new bytes) against `old` (the base parse of each
/// edited file) for `edit`.
///
/// Every projected file must parse without errors. For `replace_symbol` and
/// `replace_body` the new parse holds a symbol with the old symbol's
/// `qualified_name` and `symbol_type`; for `delete` it holds none; for
/// `insert_before` and `insert_after` it holds each symbol the inserted text
/// declares (parsed alone; text that does not parse alone declares none). Each
/// symbol of an edited file that is neither an op's symbol nor encloses one is
/// matched by `qualified_name` and must keep its `ast_hash`.
pub fn verify_roundtrip(
    edit: &SymbolEdit,
    old: &[ParseResult],
    projected: &[(String, Vec<u8>)],
) -> Result<Vec<OpVerdict>, RoundtripRefusal> {
    let mut parsed: BTreeMap<&str, ParseResult> = BTreeMap::new();
    for (path, bytes) in projected {
        parsed.insert(path.as_str(), parse_clean(path, bytes)?);
    }
    let mismatch = |path: &str, symbol: &str| RoundtripRefusal::Mismatch {
        path: path.to_string(),
        symbol: symbol.to_string(),
    };
    // The byte ranges the ops touch, per file.
    let mut touched: BTreeMap<&str, Vec<(u32, u32)>> = BTreeMap::new();
    let mut verdicts = Vec::with_capacity(edit.ops.len());
    for op in &edit.ops {
        let (path, symbol) =
            find_old(old, &op.symbol_id).ok_or_else(|| mismatch("", &op.symbol_id))?;
        let new = parsed
            .get(path)
            .ok_or_else(|| mismatch(path, &symbol.qualified_name))?;
        let holds = |name: &str, kind: &SymbolType| {
            new.symbols
                .iter()
                .any(|s| s.qualified_name == name && &s.symbol_type == kind)
        };
        let held = match op.kind {
            SymbolEditKind::ReplaceSymbol | SymbolEditKind::ReplaceBody => {
                holds(&symbol.qualified_name, &symbol.symbol_type)
            }
            SymbolEditKind::Delete => !new
                .symbols
                .iter()
                .any(|s| s.qualified_name == symbol.qualified_name),
            SymbolEditKind::InsertBefore | SymbolEditKind::InsertAfter => {
                let declared = parse_clean(path, op.new_text.as_bytes())
                    .map(|fragment| fragment.symbols)
                    .unwrap_or_default();
                declared.iter().all(|d| {
                    new.symbols.iter().any(|s| {
                        s.symbol_type == d.symbol_type
                            && s.name == d.name
                            && s.qualified_name.ends_with(&d.qualified_name)
                    })
                })
            }
        };
        if !held {
            return Err(mismatch(path, &symbol.qualified_name));
        }
        touched
            .entry(path)
            .or_default()
            .push((symbol.byte_start, symbol.byte_end));
        let bytes = projected
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b.as_slice())
            .unwrap_or_default();
        verdicts.push(OpVerdict {
            symbol_id: op.symbol_id.clone(),
            kind: op.kind,
            path: path.to_string(),
            structure_hash: structure_hash(path, bytes, new)?,
        });
    }
    for file in old {
        let (Some(new), Some(ranges)) = (
            parsed.get(file.file_path.as_str()),
            touched.get(file.file_path.as_str()),
        ) else {
            continue;
        };
        for symbol in &file.symbols {
            let encloses_edit = ranges
                .iter()
                .any(|(start, end)| symbol.byte_start < *end && *start < symbol.byte_end);
            if encloses_edit {
                continue;
            }
            let kept = new
                .symbols
                .iter()
                .any(|s| s.qualified_name == symbol.qualified_name && s.ast_hash == symbol.ast_hash);
            if !kept {
                return Err(mismatch(&file.file_path, &symbol.qualified_name));
            }
        }
    }
    Ok(verdicts)
}
