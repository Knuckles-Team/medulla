//! MEDULLA-SCM-R013.3: the re-parse of a projected symbol edit.

use medulla_compute::ast::parser::{parse_file, SourceLanguage};
use medulla_compute_assemble::ast::symbol::ParseResult;
use medulla_enrich::symbol_edit_verify::{verify_roundtrip, RoundtripRefusal};
use medulla_source_types::source_symbol_edit::{SymbolEdit, SymbolEditKind, SymbolEditOp};

const PATH: &str = "src/lib.rs";
const BASE: &str = "fn alpha() -> u32 {\n    1\n}\n\nfn beta() -> u32 {\n    2\n}\n\nfn gamma() -> u32 {\n    3\n}\n";

fn base() -> ParseResult {
    parse_file(BASE, PATH, SourceLanguage::Rust)
}

fn symbol_id(parsed: &ParseResult, name: &str) -> String {
    parsed
        .symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("base parse has no symbol {name}"))
        .id
        .clone()
}

fn edit(kind: SymbolEditKind, symbol_id: String, new_text: &str) -> SymbolEdit {
    SymbolEdit {
        repo: "r".into(),
        branch: "b".into(),
        base_rev: "0".repeat(64),
        ops: vec![SymbolEditOp {
            kind,
            symbol_id,
            anchor_span_digest: String::new(),
            new_text: new_text.into(),
        }],
        message: "edit".into(),
        run_trace_id: "run".into(),
    }
}

/// The base text with the symbol's span replaced by `replacement`.
fn project(parsed: &ParseResult, name: &str, replacement: &str) -> Vec<(String, Vec<u8>)> {
    let symbol = parsed.symbols.iter().find(|s| s.name == name).unwrap();
    let mut text = BASE.to_string();
    text.replace_range(
        symbol.byte_start as usize..symbol.byte_end as usize,
        replacement,
    );
    vec![(PATH.to_string(), text.into_bytes())]
}

// spec: MEDULLA-SCM-R013.3
#[test]
fn reparse_confirms_edit_and_untouched_symbols() {
    let old = base();
    let old_files = std::slice::from_ref(&old);
    let beta = symbol_id(&old, "beta");

    // replace_symbol and replace_body keep the symbol and every other hash.
    let replaced = "fn beta() -> u32 {\n    22\n}";
    for kind in [SymbolEditKind::ReplaceSymbol, SymbolEditKind::ReplaceBody] {
        let verdicts = verify_roundtrip(
            &edit(kind, beta.clone(), replaced),
            old_files,
            &project(&old, "beta", replaced),
        )
        .unwrap();
        assert_eq!(verdicts.len(), 1);
        assert_eq!(verdicts[0].path, PATH);
        assert!(!verdicts[0].structure_hash.is_empty());
    }

    // insert_before and insert_after: the declared symbol appears.
    let inserted = "fn delta() -> u32 {\n    4\n}\n\nfn beta() -> u32 {\n    2\n}";
    for kind in [SymbolEditKind::InsertBefore, SymbolEditKind::InsertAfter] {
        verify_roundtrip(
            &edit(kind, beta.clone(), "fn delta() -> u32 {\n    4\n}\n\n"),
            old_files,
            &project(&old, "beta", inserted),
        )
        .unwrap();
    }

    // delete: the symbol is gone and the others are unchanged.
    verify_roundtrip(
        &edit(SymbolEditKind::Delete, beta.clone(), ""),
        old_files,
        &project(&old, "beta", ""),
    )
    .unwrap();

    // A projected file that does not parse refuses EDIT_UNPARSEABLE.
    let broken = project(&old, "beta", "fn beta( {");
    assert_eq!(
        verify_roundtrip(
            &edit(SymbolEditKind::ReplaceSymbol, beta.clone(), "x"),
            old_files,
            &broken
        ),
        Err(RoundtripRefusal::Unparseable { path: PATH.into() })
    );

    // A replace that renames the symbol, and a delete that leaves it, mismatch.
    let renamed = "fn beta2() -> u32 {\n    2\n}";
    assert!(matches!(
        verify_roundtrip(
            &edit(SymbolEditKind::ReplaceSymbol, beta.clone(), renamed),
            old_files,
            &project(&old, "beta", renamed),
        ),
        Err(RoundtripRefusal::Mismatch { .. })
    ));
    assert!(matches!(
        verify_roundtrip(
            &edit(SymbolEditKind::Delete, beta.clone(), ""),
            old_files,
            &project(&old, "beta", "fn beta() -> u32 {\n    2\n}"),
        ),
        Err(RoundtripRefusal::Mismatch { .. })
    ));

    // An untouched symbol whose bytes changed outside the edit mismatches.
    let mut tampered = project(&old, "beta", replaced);
    let text = String::from_utf8(tampered[0].1.clone())
        .unwrap()
        .replace("    3", "    33");
    tampered[0].1 = text.into_bytes();
    match verify_roundtrip(
        &edit(SymbolEditKind::ReplaceSymbol, beta, replaced),
        old_files,
        &tampered,
    ) {
        Err(RoundtripRefusal::Mismatch { path, symbol }) => {
            assert_eq!(path, PATH);
            assert!(symbol.contains("gamma"));
        }
        other => panic!("expected a mismatch on gamma, got {other:?}"),
    }
}
