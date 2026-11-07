//! Slice a Rust source by CALL GRAPH rather than by byte offset.
//!
//! A gate asserting "the dispatch path does X" must keep passing when X moves one call down into a
//! helper or is reindented. Resolving the property through the call graph, on whitespace-squashed
//! text, makes the assertion about the architecture and not about the layout. A helper the entry
//! point never calls contributes nothing, so a token cannot be satisfied by dead code.
//!
//! This is a lexical approximation, not a compiler: it assumes rustfmt'd source (a top-level item
//! header starts at column 0 and its body ends at the first line that is exactly `}`), matches
//! callees by `name(` (so it over-approximates, never under-approximates), and fails LOUD with an
//! empty body, hence a failing gate, when the layout assumption stops holding.

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;

/// Every top-level `fn` in `src`, name to body (header through its closing `}` line).
pub fn top_level_fns(src: &str) -> BTreeMap<String, String> {
    let header = Regex::new(
        r#"^(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)"#,
    )
    .expect("static regex");
    let lines: Vec<&str> = src.split_inclusive('\n').collect();
    let starts: Vec<(usize, String)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| header.captures(line).map(|c| (i, c[1].to_string())))
        .collect();
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for (i, name) in starts {
        let mut end = i;
        while end < lines.len() && lines[end].trim_end_matches('\n') != "}" {
            end += 1;
        }
        // A name may be defined twice under mutually exclusive `#[cfg]`s; keep both.
        let body: String = lines[i..(end + 1).min(lines.len())].concat();
        out.entry(name).or_default().push_str(&body);
    }
    out
}

/// Names called (`name(`) in `body`.
pub fn called_names(body: &str) -> BTreeSet<String> {
    let call = Regex::new(r"([A-Za-z_][A-Za-z0-9_]*)\s*\(").expect("static regex");
    call.captures_iter(body).map(|c| c[1].to_string()).collect()
}

/// `root`'s body plus every top-level fn in `src` it transitively calls ("" when `root` is absent).
pub fn reachable_source(src: &str, root: &str) -> String {
    let fns = top_level_fns(src);
    if !fns.contains_key(root) {
        return String::new();
    }
    let mut seen: BTreeSet<String> = BTreeSet::from([root.to_string()]);
    let mut queue = vec![root.to_string()];
    while let Some(name) = queue.pop() {
        for callee in called_names(&fns[&name]) {
            if fns.contains_key(&callee) && seen.insert(callee.clone()) {
                queue.push(callee);
            }
        }
    }
    seen.iter()
        .map(|n| fns[n].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "pub fn entry() {\n    helper();\n}\n\nfn helper() {\n    leaf();\n}\n\nfn leaf() {\n    marker();\n}\n\nfn dead() {\n    marker_dead();\n}\n";

    #[test]
    fn collects_top_level_fns() {
        let fns = top_level_fns(SRC);
        assert_eq!(
            fns.keys().map(String::as_str).collect::<Vec<_>>(),
            ["dead", "entry", "helper", "leaf"]
        );
        assert!(fns["entry"].starts_with("pub fn entry"));
    }

    #[test]
    fn reachability_follows_calls_and_ignores_dead_code() {
        let slice = reachable_source(SRC, "entry");
        assert!(slice.contains("marker()") && !slice.contains("marker_dead"));
        assert_eq!(reachable_source(SRC, "absent"), "");
    }

    #[test]
    fn same_name_under_two_cfgs_keeps_both() {
        let src = "#[cfg(a)]\nfn f() {\n    one();\n}\n#[cfg(b)]\nfn f() {\n    two();\n}\n";
        let body = &top_level_fns(src)["f"];
        assert!(body.contains("one()") && body.contains("two()"));
    }
}
