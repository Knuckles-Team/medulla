//! The compiler-declared module tree: `mod` and literal `include!` declarations to files.
//!
//! A production view blanks every item whose `cfg` cannot hold without `test`; the test-inclusive
//! view keeps everything. Missing, ambiguous, cyclic, unsupported and orphaned declarations are
//! errors, never silently skipped, so no contract-bearing source can escape a scan built on this.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use regex::Regex;

use super::cfg::{self, Cfg};
use super::mask::{balanced_span_from, code_mask, comments_mask};

/// One loaded view of a module tree.
pub struct Loaded {
    pub source: String,
    pub paths: BTreeSet<PathBuf>,
    pub module_dir: PathBuf,
}

#[derive(Clone)]
struct Input {
    path: PathBuf,
    module_dir: PathBuf,
    predicates: Vec<Cfg>,
}

const MOD_RS_NAMES: &[&str] = &["mod.rs", "lib.rs", "main.rs"];

/// Source of `relative` and every file it declares. `include_tests` keeps `cfg(test)` items.
pub fn read(root: &Path, relative: &str, include_tests: bool) -> Result<String, String> {
    Ok(load(root, relative, include_tests, false)?.source)
}

/// Load a module tree. `crate_root` marks a cargo target root, which owns its containing directory.
pub fn load(
    root: &Path,
    relative: &str,
    include_tests: bool,
    crate_root: bool,
) -> Result<Loaded, String> {
    let allowed = root
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", root.display()))?;
    let start = allowed.join(relative);
    if !start.is_file() {
        return Err(format!("missing Rust facade or module tree: {relative}"));
    }
    let start = start.canonicalize().map_err(|e| e.to_string())?;
    let name = start.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let module_dir = if crate_root || MOD_RS_NAMES.contains(&name) {
        start.parent().expect("a file has a parent").to_path_buf()
    } else {
        start.with_extension("")
    };
    let mut state = State {
        allowed,
        include_tests,
        loaded: HashSet::new(),
        visiting: HashSet::new(),
        sources: vec![],
        paths: BTreeSet::new(),
    };
    state.visit(Input {
        path: start,
        module_dir: module_dir.clone(),
        predicates: vec![],
    })?;
    Ok(Loaded {
        source: state.sources.join("\n"),
        paths: state.paths,
        module_dir,
    })
}

/// A module family: its production source and every declared file's raw text, by repository path.
pub struct Family {
    pub production: String,
    pub files: BTreeMap<String, String>,
}

/// The family of `relative`, after proving a test-inclusive walk reaches every `*.rs` below the
/// root: rejects any `*.rs` below the root that no declaration reaches.
pub fn compiler_family(root: &Path, relative: &str) -> Result<Family, String> {
    let production = load(root, relative, false, false)?;
    let with_tests = load(root, relative, true, false)?;
    if production.module_dir != with_tests.module_dir {
        return Err("Rust module root changed between views".into());
    }
    if !production.paths.is_subset(&with_tests.paths) {
        return Err(format!(
            "production Rust module is absent from test-inclusive tree: {relative}"
        ));
    }
    if production.module_dir.is_dir() {
        let orphans: Vec<String> = walkdir::WalkDir::new(&production.module_dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "rs"))
            .filter_map(|e| e.path().canonicalize().ok())
            .filter(|p| !with_tests.paths.contains(p))
            .map(|p| p.display().to_string())
            .collect();
        if !orphans.is_empty() {
            return Err(format!(
                "orphan Rust module files are not compiler-reachable from {relative}: {orphans:?}"
            ));
        }
    }
    let canonical = root.canonicalize().map_err(|e| e.to_string())?;
    let mut files = BTreeMap::new();
    for path in &with_tests.paths {
        let name = path
            .strip_prefix(&canonical)
            .map_err(|_| {
                format!(
                    "Rust module path escapes root directory: {}",
                    path.display()
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {name}: {e}"))?;
        files.insert(name, text);
    }
    Ok(Family {
        production: production.source,
        files,
    })
}

struct State {
    allowed: PathBuf,
    include_tests: bool,
    loaded: HashSet<String>,
    visiting: HashSet<PathBuf>,
    sources: Vec<String>,
    paths: BTreeSet<PathBuf>,
}

impl State {
    fn visit(&mut self, input: Input) -> Result<(), String> {
        let path = input
            .path
            .canonicalize()
            .map_err(|_| format!("missing declared Rust module: {}", input.path.display()))?;
        if !path.starts_with(&self.allowed) {
            return Err(format!(
                "Rust module path escapes root directory: {}",
                path.display()
            ));
        }
        if self.visiting.contains(&path) {
            return Err(format!(
                "cyclic Rust module declaration: {}",
                path.display()
            ));
        }
        let key = format!(
            "{}|{}|{:?}",
            path.display(),
            input.module_dir.display(),
            input.predicates
        );
        if !self.loaded.insert(key) {
            return Ok(());
        }
        self.visiting.insert(path.clone());
        self.paths.insert(path.clone());
        let source = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let (sanitized, children) = scan_file(
            &Input {
                path: path.clone(),
                ..input
            },
            &source,
            self.include_tests,
        )?;
        self.sources.push(sanitized);
        for child in children {
            self.visit(child)?;
        }
        self.visiting.remove(&path);
        Ok(())
    }
}

struct Attr {
    start: usize,
    end: usize,
    inner: bool,
    name: String,
    /// Offsets of the text between the parentheses of `name(...)`.
    args: Option<(usize, usize)>,
}

fn skip_ws(mask: &str, mut at: usize) -> usize {
    let b = mask.as_bytes();
    while at < b.len() && b[at].is_ascii_whitespace() {
        at += 1;
    }
    at
}

/// The attribute opening at `at`, if `at` holds `#[`/`#![`.
fn attr_at(mask: &str, at: usize) -> Result<Option<Attr>, String> {
    let b = mask.as_bytes();
    if b.get(at) != Some(&b'#') {
        return Ok(None);
    }
    let mut cur = skip_ws(mask, at + 1);
    let inner = b.get(cur) == Some(&b'!');
    if inner {
        cur = skip_ws(mask, cur + 1);
    }
    if b.get(cur) != Some(&b'[') {
        return Ok(None);
    }
    let close = balanced_span_from(mask, cur, b'[', b']')?;
    let body_start = skip_ws(mask, cur + 1);
    let mut name_end = body_start;
    while name_end < close
        && (b[name_end].is_ascii_alphanumeric() || b[name_end] == b'_' || b[name_end] == b':')
    {
        name_end += 1;
    }
    let after = skip_ws(mask, name_end);
    let args = if b.get(after) == Some(&b'(') {
        let end = balanced_span_from(mask, after, b'(', b')')?;
        Some((after + 1, end))
    } else {
        None
    };
    Ok(Some(Attr {
        start: at,
        end: close + 1,
        inner,
        name: mask[body_start..name_end].to_string(),
        args,
    }))
}

/// Attributes that cannot introduce an item, so a conditional form of them is inert.
const INERT_ATTRIBUTES: &[&str] = &[
    "allow",
    "deny",
    "derive",
    "doc",
    "forbid",
    "recursion_limit",
    "schemars",
    "serde",
    "warn",
];
const KNOWN_DERIVES: &[&str] = &[
    "schemars::JsonSchema",
    "serde::Serialize",
    "serde::Deserialize",
    "strum::IntoStaticStr",
];

fn top_level_commas(text: &str) -> Vec<&str> {
    let mut parts = vec![];
    let (mut depth, mut from) = (0i32, 0usize);
    for (i, c) in text.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&text[from..i]);
                from = i + 1;
            }
            _ => {}
        }
    }
    if !text[from..].trim().is_empty() {
        parts.push(&text[from..]);
    }
    parts
}

/// The `cfg` predicates one attribute list contributes. A `cfg_attr` may only wrap an audited inert attribute.
fn predicates_of(cmask: &str, attrs: &[Attr]) -> Result<Vec<Cfg>, String> {
    let mut out = vec![];
    for attr in attrs {
        let Some((from, to)) = attr.args else {
            continue;
        };
        match attr.name.as_str() {
            "cfg" => out.push(cfg::parse(&cmask[from..to])?),
            "cfg_attr" => {
                let parts = top_level_commas(&cmask[from..to]);
                for part in parts.iter().skip(1) {
                    let part = part.trim();
                    let name = part.split('(').next().unwrap_or("").trim();
                    if !INERT_ATTRIBUTES.contains(&name) {
                        return Err(format!(
                            "conditional attribute `{name}` may produce items; unsupported by the module-tree walk"
                        ));
                    }
                    if name == "derive" {
                        let inside = part
                            .trim_start_matches("derive")
                            .trim()
                            .trim_start_matches('(')
                            .trim_end_matches(')');
                        for derive in top_level_commas(inside) {
                            if !KNOWN_DERIVES.contains(&derive.trim()) {
                                return Err(format!(
                                    "conditional derive `{}` is not in the audited inert list",
                                    derive.trim()
                                ));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

fn static_string(value: &str, context: &str) -> Result<String, String> {
    let value = value.trim();
    let normal = Regex::new(r#"^"([^"\\]*)"$"#).expect("static regex");
    if let Some(c) = normal.captures(value) {
        return Ok(c[1].to_string());
    }
    let raw = Regex::new(r#"(?s)^r(#*)"(.*)"(#*)$"#).expect("static regex");
    if let Some(c) = raw.captures(value) {
        if c[1] == c[3] {
            return Ok(c[2].to_string());
        }
    }
    Err(format!("{context} must be one static Rust string literal"))
}

fn path_override(cmask: &str, attrs: &[Attr]) -> Result<Option<String>, String> {
    let paths: Vec<_> = attrs.iter().filter(|a| a.name == "path").collect();
    if paths.len() > 1 {
        return Err("Rust module has multiple path attributes".into());
    }
    let Some(attr) = paths.first() else {
        return Ok(None);
    };
    let re = Regex::new(r"(?s)^#\s*\[\s*path\s*=\s*(.*?)\s*\]$").expect("static regex");
    let c = re
        .captures(&cmask[attr.start..attr.end])
        .ok_or("unsupported Rust path attribute")?;
    static_string(&c[1], "Rust path attribute").map(Some)
}

/// Whether an item ends at its first `;` even when braces occur earlier (`use a::{b};`, `static X: T = T {..};`).
fn ends_at_semicolon(head: &str) -> bool {
    let vis = Regex::new(r"^pub(?:\s*\([^)]*\))?\s+").expect("static regex");
    let rest = vis.replace(head, "");
    let word: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let tail = rest[word.len()..].trim_start();
    match word.as_str() {
        "use" | "static" | "type" | "let" => true,
        "const" => !["fn", "unsafe", "async", "extern"]
            .iter()
            .any(|w| tail.starts_with(w)),
        _ => false,
    }
}

/// Where an item starting at `start` ends: its first top-level `;`, or its balanced block.
fn item_end(mask: &str, start: usize, limit: usize) -> Result<usize, String> {
    let b = mask.as_bytes();
    let semicolon_item = ends_at_semicolon(&mask[start..limit.min(start + 96)]);
    let mut depth = 0i32;
    let mut i = start;
    while i < limit {
        match b[i] {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b';' if depth == 0 => return Ok(i + 1),
            b'{' if depth == 0 => {
                let close = balanced_span_from(mask, i, b'{', b'}')?;
                if semicolon_item {
                    i = close + 1;
                    continue;
                }
                let next = skip_ws(mask, close + 1);
                return Ok(if b.get(next) == Some(&b';') && next < limit {
                    next + 1
                } else {
                    close + 1
                });
            }
            _ => {}
        }
        i += 1;
    }
    Ok(limit)
}

struct Walker<'a> {
    path: &'a Path,
    mask: String,
    cmask: String,
    include_tests: bool,
    excluded: Vec<(usize, usize)>,
    children: Vec<Input>,
    /// Spans of the `mod name {`/`mod name;` heads and `include!(` openers the walk owns.
    consumed: Vec<(usize, usize)>,
    templates: Vec<(usize, usize)>,
}

impl Walker<'_> {
    fn visit_scope(
        &mut self,
        start: usize,
        limit: usize,
        module_dir: &Path,
        path_base: &Path,
        inherited: &[Cfg],
    ) -> Result<(), String> {
        let mut cursor = start;
        let mut inner = vec![];
        loop {
            cursor = skip_ws(&self.mask, cursor);
            match attr_at(&self.mask, cursor)? {
                Some(a) if a.inner && a.end <= limit => {
                    cursor = a.end;
                    inner.push(a);
                }
                _ => break,
            }
        }
        let mut scope: Vec<Cfg> = inherited.to_vec();
        if !self.include_tests {
            scope.extend(predicates_of(&self.cmask, &inner)?);
            if !cfg::possible_without_test(&scope)? {
                self.excluded.push((start, limit));
                return Ok(());
            }
        }
        let module_head = Regex::new(r"^(?:pub(?:\s*\([^)]*\))?\s+)?(?:unsafe\s+)?mod\s+(?:r#)?([A-Za-z_][A-Za-z0-9_]*)\s*([;{])")
            .expect("static regex");
        let include_head = Regex::new(r"^include\s*!\s*\(").expect("static regex");
        while cursor < limit {
            cursor = skip_ws(&self.mask, cursor);
            if cursor >= limit {
                break;
            }
            let attrs_start = cursor;
            let mut attrs = vec![];
            while let Some(a) = attr_at(&self.mask, cursor)? {
                if a.inner {
                    return Err(format!(
                        "unexpected inner attribute in {}",
                        self.path.display()
                    ));
                }
                cursor = skip_ws(&self.mask, a.end);
                attrs.push(a);
            }
            let item_start = cursor;
            if item_start >= limit {
                break;
            }
            let end = item_end(&self.mask, item_start, limit)?;
            cursor = end;
            let head = self.mask[item_start..end].to_string();
            let mut item_preds = scope.clone();
            if !self.include_tests {
                item_preds.extend(predicates_of(&self.cmask, &attrs)?);
            }
            if !self.include_tests && !cfg::possible_without_test(&item_preds)? {
                self.excluded.push((attrs_start, end));
                continue;
            }
            if head.starts_with("macro_rules") {
                self.templates.push((item_start, end));
            } else if let Some(m) = module_head.captures(&head) {
                let name = m[1].to_string();
                let whole = m.get(0).expect("whole match");
                self.consumed.push((item_start, item_start + whole.end()));
                if &m[2] == ";" {
                    let child =
                        self.resolve_child(module_dir, path_base, &name, &attrs, &item_preds)?;
                    self.children.push(child);
                } else {
                    let opener = item_start + whole.end() - 1;
                    let closer = balanced_span_from(&self.mask, opener, b'{', b'}')?;
                    let child_dir = module_dir.join(&name);
                    self.visit_scope(opener + 1, closer, &child_dir, &child_dir, &item_preds)?;
                }
            } else if let Some(m) = include_head.find(&head) {
                let opener = item_start + m.end() - 1;
                self.consumed.push((item_start, item_start + m.end()));
                let closer = balanced_span_from(&self.mask, opener, b'(', b')')?;
                let relative = static_string(
                    &self.cmask[opener + 1..closer],
                    &format!("include! path in {}", self.path.display()),
                )?;
                self.children.push(Input {
                    path: self
                        .path
                        .parent()
                        .expect("a file has a parent")
                        .join(relative),
                    module_dir: module_dir.to_path_buf(),
                    predicates: item_preds,
                });
            }
        }
        Ok(())
    }

    fn resolve_child(
        &self,
        module_dir: &Path,
        path_base: &Path,
        name: &str,
        attrs: &[Attr],
        predicates: &[Cfg],
    ) -> Result<Input, String> {
        if let Some(explicit) = path_override(&self.cmask, attrs)? {
            // A file loaded through `#[path]` owns its own directory, like `mod.rs`.
            let resolved = path_base.join(explicit);
            let dir = resolved
                .parent()
                .expect("a file has a parent")
                .to_path_buf();
            return Ok(Input {
                path: resolved,
                module_dir: dir,
                predicates: predicates.to_vec(),
            });
        }
        let child_dir = module_dir.join(name);
        let candidates = [
            module_dir.join(format!("{name}.rs")),
            child_dir.join("mod.rs"),
        ];
        let existing: Vec<_> = candidates.iter().filter(|c| c.is_file()).collect();
        if existing.len() != 1 {
            return Err(format!(
                "declared Rust module must resolve to exactly one file: {}::{name}",
                self.path.display()
            ));
        }
        Ok(Input {
            path: existing[0].clone(),
            module_dir: child_dir,
            predicates: predicates.to_vec(),
        })
    }
}

fn scan_file(
    input: &Input,
    source: &str,
    include_tests: bool,
) -> Result<(String, Vec<Input>), String> {
    let mut walker = Walker {
        path: &input.path,
        mask: code_mask(source)?,
        cmask: comments_mask(source)?,
        include_tests,
        excluded: vec![],
        children: vec![],
        consumed: vec![],
        templates: vec![],
    };
    let parent = input
        .path
        .parent()
        .expect("a file has a parent")
        .to_path_buf();
    walker.visit_scope(
        0,
        source.len(),
        &input.module_dir,
        &parent,
        &input.predicates,
    )?;
    let mut sanitized = source.as_bytes().to_vec();
    for &(from, to) in &walker.excluded {
        for byte in &mut sanitized[from..to] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    let sanitized = String::from_utf8(sanitized).map_err(|e| e.to_string())?;
    require_declarations_consumed(&input.path, &sanitized, &walker)?;
    Ok((sanitized, walker.children))
}

/// Fail closed on a compiler-active `mod` or `include!` the traversal did not own.
fn require_declarations_consumed(
    path: &Path,
    sanitized: &str,
    walker: &Walker,
) -> Result<(), String> {
    let active = code_mask(sanitized)?;
    let in_template = |pos: usize| walker.templates.iter().any(|&(s, e)| s <= pos && pos < e);
    let owned = |pos: usize| walker.consumed.iter().any(|&(s, e)| s <= pos && pos < e);
    let failure = format!(
        "compiler-active Rust module declaration was not consumed by the module-tree traversal: {}",
        path.display()
    );
    let keyword = Regex::new(r"\bmod\b").expect("static regex");
    for hit in keyword.find_iter(&active) {
        let raw = hit.start() >= 2 && &active[hit.start() - 2..hit.start()] == "r#";
        if !raw && !owned(hit.start()) && !in_template(hit.start()) {
            return Err(failure);
        }
    }
    let out_of_line = Regex::new(r"\bmod\s+(?:r#)?\w+\s*;").expect("static regex");
    if out_of_line
        .find_iter(&active)
        .any(|hit| in_template(hit.start()))
    {
        return Err(failure);
    }
    let include = Regex::new(r"include\s*!\s*\(").expect("static regex");
    if include.find_iter(&active).any(|hit| !owned(hit.start())) {
        return Err(format!(
            "compiler-active Rust include was not consumed by the module-tree traversal: {}",
            path.display()
        ));
    }
    Ok(())
}
