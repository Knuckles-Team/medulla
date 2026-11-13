//! Atom decomposition (MEDULLA-INGEST-WORKER-R003.2).
//!
//! Each chunk of `chunk_text` becomes one or more typed [`KnowledgeAtom`]s. A
//! chunk longer than the word limit splits at sentence ends. The kind comes from
//! the first matching rule: procedure, event, preference, otherwise fact.
//! Context selection over the atoms is `medulla_types::ingest_rules::select_atoms`.

use medulla_types::contract::Digest256;
use medulla_types::knowledge_atom::{AtomKind, KnowledgeAtom};

use crate::processing::ChunkSpan;

/// The digest domain every atom id is framed under.
pub const ATOM_ID_DOMAIN: &[u8] = b"eg/knowledge-atom";

/// Verbs a list item may start with to count as a procedure step.
const STEP_VERBS: &[&str] = &[
    "add",
    "apply",
    "assign",
    "build",
    "check",
    "choose",
    "click",
    "close",
    "configure",
    "confirm",
    "connect",
    "copy",
    "create",
    "delete",
    "deploy",
    "disable",
    "download",
    "enable",
    "enter",
    "execute",
    "export",
    "find",
    "import",
    "install",
    "launch",
    "log",
    "make",
    "move",
    "open",
    "paste",
    "press",
    "provide",
    "remove",
    "rename",
    "replace",
    "restart",
    "restore",
    "review",
    "run",
    "save",
    "search",
    "select",
    "send",
    "set",
    "sign",
    "start",
    "stop",
    "submit",
    "test",
    "type",
    "uninstall",
    "update",
    "upload",
    "use",
    "verify",
    "wait",
    "write",
];

/// The list-item body of `line` (`-`, `*`, `+`, `1.` or `1)` markers), if it is one.
fn list_item(line: &str) -> Option<&str> {
    let line = line.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest.trim_start());
        }
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = &line[digits..];
    for marker in [". ", ") "] {
        if let Some(body) = rest.strip_prefix(marker) {
            return Some(body.trim_start());
        }
    }
    None
}

fn is_procedure(text: &str) -> bool {
    text.lines()
        .filter_map(list_item)
        .filter(|body| {
            body.split_whitespace()
                .next()
                .map(|w| {
                    let w = w.trim_matches(|c: char| !c.is_alphabetic()).to_lowercase();
                    STEP_VERBS.contains(&w.as_str())
                })
                .unwrap_or(false)
        })
        .count()
        >= 2
}

fn digits_at(bytes: &[u8], from: usize, n: usize) -> bool {
    bytes.len() >= from + n && bytes[from..from + n].iter().all(u8::is_ascii_digit)
}

/// Whether a token is an ISO date (`YYYY-MM-DD`, optionally followed by a time) or a
/// clock time (`HH:MM`).
fn is_time_token(token: &str) -> bool {
    let t = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let b = t.as_bytes();
    let date = digits_at(b, 0, 4)
        && b.get(4) == Some(&b'-')
        && digits_at(b, 5, 2)
        && b.get(7) == Some(&b'-')
        && digits_at(b, 8, 2)
        && (b.len() == 10 || matches!(b.get(10), Some(b'T') | Some(b't')));
    let time = digits_at(b, 0, 2)
        && b.get(2) == Some(&b':')
        && digits_at(b, 3, 2)
        && (b.len() == 5 || (b.len() == 8 && b.get(5) == Some(&b':') && digits_at(b, 6, 2)));
    date || time
}

fn is_event(text: &str) -> bool {
    text.split_whitespace().any(is_time_token)
}

fn is_preference(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["prefer", "favorite", "always use", "never use"]
        .iter()
        .any(|needle| lower.contains(needle))
}

/// The kind of a chunk's text: the first matching rule wins.
pub fn classify(text: &str) -> AtomKind {
    if is_procedure(text) {
        AtomKind::Procedure
    } else if is_event(text) {
        AtomKind::Event
    } else if is_preference(text) {
        AtomKind::Preference
    } else {
        AtomKind::Fact
    }
}

/// Byte ranges of the sentences of `text`: each ends after `.`, `!` or `?` that is followed
/// by whitespace or the end, and the last runs to the end.
fn sentences(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(c, '.' | '!' | '?') {
            let end = i + c.len_utf8();
            if chars.peek().map(|(_, n)| n.is_whitespace()).unwrap_or(true) {
                out.push((start, end));
                while let Some((j, n)) = chars.peek().copied() {
                    if n.is_whitespace() {
                        chars.next();
                    } else {
                        start = j;
                        break;
                    }
                }
                if chars.peek().is_none() {
                    start = text.len();
                }
            }
        }
    }
    if start < text.len() && !text[start..].trim().is_empty() {
        out.push((start, text.len()));
    }
    out
}

/// Pack sentences into parts of at most `max_tokens` words. A sentence alone over the limit
/// stays one part. Parts are byte ranges inside `text`.
fn parts(text: &str, max_tokens: usize) -> Vec<(usize, usize)> {
    if text.split_whitespace().count() <= max_tokens {
        return vec![(0, text.len())];
    }
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut words = 0usize;
    for (s, e) in sentences(text) {
        let n = text[s..e].split_whitespace().count();
        match out.last_mut() {
            Some(last) if words + n <= max_tokens => {
                last.1 = e;
                words += n;
            }
            _ => {
                out.push((s, e));
                words = n;
            }
        }
    }
    out
}

/// Decompose the `chunks` of `text` (as cut by `chunk_text`) into atoms for `tenant`.
///
/// `max_tokens` is `knob ingest.atom_max_tokens`. The `atom_id` is SHA-256 framed under
/// [`ATOM_ID_DOMAIN`] over tenant, document id, chunk ordinal and part index; the
/// `provenance_key` is `<document_id>#<start_char>-<end_char>` of the part.
pub fn decompose(
    tenant: &str,
    document_id: &str,
    chunks: &[ChunkSpan],
    text: &str,
    max_tokens: usize,
) -> Result<Vec<KnowledgeAtom>, String> {
    let mut atoms = Vec::new();
    for chunk in chunks {
        let body = text
            .get(chunk.start_byte..chunk.end_byte)
            .ok_or_else(|| format!("chunk {} lies outside the text", chunk.ordinal))?;
        let kind = classify(body);
        for (index, (s, e)) in parts(body, max_tokens.max(1)).into_iter().enumerate() {
            let part = body[s..e].trim();
            if part.is_empty() {
                continue;
            }
            let start_char = chunk.start_char + body[..s].chars().count();
            let end_char = start_char + body[s..e].chars().count();
            let atom_id = Digest256::framed(
                ATOM_ID_DOMAIN,
                &[
                    tenant.as_bytes(),
                    document_id.as_bytes(),
                    &chunk.ordinal.to_be_bytes(),
                    &(index as u32).to_be_bytes(),
                ],
            )?
            .to_hex();
            atoms.push(KnowledgeAtom {
                atom_id,
                kind,
                text: part.to_string(),
                provenance_key: format!("{document_id}#{start_char}-{end_char}"),
                tenant: tenant.to_string(),
            });
        }
    }
    Ok(atoms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::{chunk_text, ChunkingConfig};

    fn atoms_of(text: &str, max: usize) -> Vec<KnowledgeAtom> {
        let chunks = chunk_text(text, &ChunkingConfig::default()).unwrap();
        decompose("t1", "doc-1", &chunks, text, max).unwrap()
    }

    // spec: MEDULLA-INGEST-WORKER-R003.2
    #[test]
    fn decompose_kinds_and_split() {
        let procedure =
            "Upgrade steps:\n- Stop the worker\n- Install the new build\n- Restart it\n";
        let event = "The cutover ran on 2026-11-02 and finished at 14:30 without errors.";
        let preference = "The owner will always use short reports and prefers tables.";
        let fact = "The engine stores facts in one graph.";
        // First matching rule wins: a procedure that mentions a date stays a procedure.
        let both = "- Open the log from 2026-11-02\n- Check the entries\n";
        // One list item is not a procedure.
        let one_step = "- Stop the worker\nNothing else here.";

        assert_eq!(atoms_of(procedure, 200)[0].kind, AtomKind::Procedure);
        assert_eq!(atoms_of(event, 200)[0].kind, AtomKind::Event);
        assert_eq!(atoms_of(preference, 200)[0].kind, AtomKind::Preference);
        assert_eq!(atoms_of(fact, 200)[0].kind, AtomKind::Fact);
        assert_eq!(atoms_of(both, 200)[0].kind, AtomKind::Procedure);
        assert_eq!(atoms_of(one_step, 200)[0].kind, AtomKind::Fact);
        assert_eq!(atoms_of("Set in 12:61x", 200)[0].kind, AtomKind::Fact);

        // An over-limit chunk splits at sentence ends, each part within the limit.
        let long = "Alpha beta gamma delta. Epsilon zeta eta theta. Iota kappa lambda mu.";
        let atoms = atoms_of(long, 7);
        assert_eq!(atoms.len(), 3);
        assert_eq!(atoms[0].text, "Alpha beta gamma delta.");
        assert_eq!(atoms[2].text, "Iota kappa lambda mu.");
        assert!(atoms.iter().all(|a| a.text.split_whitespace().count() <= 7));
        // Sentences pack up to the limit: two 4-word sentences fit in 8, all three in 12.
        assert_eq!(atoms_of(long, 8).len(), 2);
        assert_eq!(atoms_of(long, 12).len(), 1);
        // A chunk within the limit is one atom.
        assert_eq!(atoms_of(long, 200).len(), 1);

        // Provenance is `<document>#<start>-<end>` in characters of the part.
        assert_eq!(atoms[0].provenance_key, "doc-1#0-23");
        assert_eq!(atoms[1].provenance_key, "doc-1#24-47");
        // Ids are stable, distinct per part, per tenant and per document.
        let again = atoms_of(long, 7);
        assert_eq!(atoms, again);
        assert_ne!(atoms[0].atom_id, atoms[1].atom_id);
        let chunks = chunk_text(long, &ChunkingConfig::default()).unwrap();
        let other_tenant = decompose("t2", "doc-1", &chunks, long, 7).unwrap();
        let other_doc = decompose("t1", "doc-2", &chunks, long, 7).unwrap();
        assert_ne!(atoms[0].atom_id, other_tenant[0].atom_id);
        assert_ne!(atoms[0].atom_id, other_doc[0].atom_id);
        assert_eq!(atoms[0].atom_id.len(), 64);
        assert!(atoms.iter().all(|a| a.tenant == "t1"));
    }
}
