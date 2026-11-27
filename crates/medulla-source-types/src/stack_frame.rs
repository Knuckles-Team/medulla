//! Stack-frame resolution (MEDULLA-SDLC-R003.3, R004.3): the one rule that maps a stack-frame
//! line to the `SYMBOL` node it points into. The incident intake and the defect triage read
//! frames through this function, so both see the same symbol for the same line. Pure: the
//! caller reads the `SYMBOL` nodes first and passes them in.

use std::collections::BTreeMap;

use serde_json::Value;

/// The node type of a code symbol.
pub const SYMBOL_TYPE: &str = "SYMBOL";

/// One indexed code symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolRef {
    /// The node id of the symbol.
    pub id: String,
    /// The file the symbol is declared in.
    pub file_path: String,
    /// First line of the declaration, 1-based.
    pub line: u64,
    /// Last line of the declaration, 1-based, inclusive.
    pub end_line: u64,
    /// The lexically qualified name.
    pub qualified_symbol: String,
}

impl SymbolRef {
    /// Read one `SYMBOL` node (decoded properties). A node of another type, or without a file
    /// path, a numeric line or a qualified name, is not a symbol.
    pub fn from_node(id: &str, node: &Value) -> Option<Self> {
        if node.get("type").and_then(Value::as_str) != Some(SYMBOL_TYPE) {
            return None;
        }
        let text = |key: &str| node.get(key).and_then(Value::as_str);
        let number = |key: &str| {
            let value = node.get(key)?;
            value.as_u64().or_else(|| value.as_str()?.parse().ok())
        };
        let line = number("line")?;
        Some(Self {
            id: id.to_owned(),
            file_path: text("file_path")?.to_owned(),
            line,
            end_line: number("end_line").unwrap_or(line).max(line),
            qualified_symbol: text("qualified_symbol").unwrap_or_default().to_owned(),
        })
    }
}

/// The symbols of one graph, indexed by file and by qualified name.
#[derive(Clone, Debug, Default)]
pub struct SymbolIndex {
    by_file: BTreeMap<String, Vec<SymbolRef>>,
    by_name: BTreeMap<String, Vec<SymbolRef>>,
}

/// Characters trimmed from both ends of a frame token.
const TOKEN_TRIM: &[char] = &['(', ')', '[', ']', ',', ';', '\'', '"', '`', '<', '>'];

impl SymbolIndex {
    /// Index `symbols`.
    pub fn new(symbols: Vec<SymbolRef>) -> Self {
        let mut index = Self::default();
        for symbol in symbols {
            if !symbol.qualified_symbol.is_empty() {
                index
                    .by_name
                    .entry(symbol.qualified_symbol.clone())
                    .or_default()
                    .push(symbol.clone());
            }
            index
                .by_file
                .entry(symbol.file_path.clone())
                .or_default()
                .push(symbol);
        }
        index
    }

    /// Whether nothing is indexed.
    pub fn is_empty(&self) -> bool {
        self.by_file.is_empty()
    }

    /// The symbol one stack-frame line points into: the first whitespace-separated token that
    /// resolves. A token `<path>:<line>` or `<path>:<line>:<col>` whose path is an indexed file
    /// resolves to the symbol of that file with `line <= n <= end_line` and the smallest span
    /// (the lowest id on a tie). A token equal to the qualified name of exactly one symbol
    /// resolves to it. A frame that resolves to nothing is `None`.
    pub fn resolve_frame(&self, frame: &str) -> Option<&SymbolRef> {
        frame
            .split_whitespace()
            .map(|token| token.trim_matches(TOKEN_TRIM))
            .filter(|token| !token.is_empty())
            .find_map(|token| self.resolve_token(token))
    }

    fn resolve_token(&self, token: &str) -> Option<&SymbolRef> {
        if let Some(symbols) = self.by_name.get(token) {
            if let [only] = symbols.as_slice() {
                return Some(only);
            }
        }
        let (path, line) = split_location(token)?;
        self.by_file
            .get(path)?
            .iter()
            .filter(|symbol| symbol.line <= line && line <= symbol.end_line)
            .min_by(|a, b| (a.end_line - a.line, &a.id).cmp(&(b.end_line - b.line, &b.id)))
    }
}

/// Split `<path>:<line>` or `<path>:<line>:<col>` into the path and the line.
fn split_location(token: &str) -> Option<(&str, u64)> {
    let mut parts = token.rsplitn(3, ':');
    let last = parts.next()?;
    let middle = parts.next()?;
    match parts.next() {
        Some(path) => {
            let (line, _column): (u64, u64) = (middle.parse().ok()?, last.parse().ok()?);
            Some((path, line))
        }
        None => Some((middle, last.parse().ok()?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(id: &str, file: &str, line: u64, end: u64, name: &str) -> SymbolRef {
        SymbolRef {
            id: id.into(),
            file_path: file.into(),
            line,
            end_line: end,
            qualified_symbol: name.into(),
        }
    }

    fn index() -> SymbolIndex {
        SymbolIndex::new(vec![
            symbol("symbol:a", "src/a.rs", 1, 100, "Outer"),
            symbol("symbol:b", "src/a.rs", 10, 20, "Outer::run"),
            symbol("symbol:c", "src/c.rs", 5, 9, "dup"),
            symbol("symbol:d", "src/d.rs", 5, 9, "dup"),
        ])
    }

    #[test]
    fn a_location_resolves_to_the_smallest_enclosing_symbol() {
        let index = index();
        let found = index
            .resolve_frame("at run (src/a.rs:15:3)")
            .expect("resolves");
        assert_eq!(found.id, "symbol:b");
        assert_eq!(
            index.resolve_frame("src/a.rs:50").expect("outer").id,
            "symbol:a"
        );
        assert!(index.resolve_frame("src/a.rs:101").is_none());
    }

    #[test]
    fn a_qualified_name_resolves_only_when_unique() {
        let index = index();
        assert_eq!(
            index.resolve_frame("in Outer::run").expect("unique").id,
            "symbol:b"
        );
        assert!(index.resolve_frame("in dup").is_none());
    }

    #[test]
    fn the_first_resolving_token_wins_and_unknown_frames_resolve_to_nothing() {
        let index = index();
        assert_eq!(
            index
                .resolve_frame("nowhere.rs:1 Outer::run src/c.rs:6")
                .expect("name")
                .id,
            "symbol:b"
        );
        assert!(index.resolve_frame("no frame here").is_none());
    }
}
