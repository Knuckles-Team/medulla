//! The closed set of node kinds a spec folder file can become
//! (MEDULLA-ORCH-VOCAB-R012.1).
//!
//! Four kinds have their own class in `sdd-v1.ttl` (`:SpecTask`, `:SpecGoal`,
//! `:SpecMemory`, `:SpecPolicy`); a document reuses `:Specification`.

/// What a spec folder file is classified as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SpecNodeKind {
    Task,
    Goal,
    Document,
    Memory,
    Policy,
}

impl SpecNodeKind {
    /// Every kind, the closed set.
    pub const ALL: [SpecNodeKind; 5] = [
        Self::Task,
        Self::Goal,
        Self::Document,
        Self::Memory,
        Self::Policy,
    ];

    /// The one wire name of each kind.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Goal => "goal",
            Self::Document => "document",
            Self::Memory => "memory",
            Self::Policy => "policy",
        }
    }

    /// Parse a wire name; anything outside the closed set is `None`.
    pub fn from_wire(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.wire_name() == name)
    }

    /// The core class the kind is stored as.
    pub fn class_name(self) -> &'static str {
        match self {
            Self::Task => "SpecTask",
            Self::Goal => "SpecGoal",
            Self::Document => "Specification",
            Self::Memory => "SpecMemory",
            Self::Policy => "SpecPolicy",
        }
    }
}

/// Refusal of the spec file classifier (MEDULLA-ORCH-VOCAB-R012.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecFolderRefusal {
    /// The file matches no rule; it is never guessed.
    UnknownKind,
}

impl std::fmt::Display for SpecFolderRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownKind => f.write_str("SPEC_FILE_UNKNOWN_KIND"),
        }
    }
}

impl std::error::Error for SpecFolderRefusal {}

/// Most keys a front-matter block may hold.
pub const MAX_FRONT_MATTER_KEYS: usize = 64;
/// Most bytes a front-matter block may hold.
pub const MAX_FRONT_MATTER_BYTES: usize = 4096;

/// Parse the flat `key: value` block between a leading `---` line and the next
/// `---` line. A malformed block, a missing closing line or a block over a bound
/// yields an empty map, never a refusal.
pub fn parse_front_matter(text: &str) -> std::collections::BTreeMap<String, String> {
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Default::default();
    }
    let mut map = std::collections::BTreeMap::new();
    let mut bytes = 0usize;
    for line in lines {
        if line.trim_end() == "---" {
            return map;
        }
        bytes += line.len() + 1;
        if bytes > MAX_FRONT_MATTER_BYTES {
            return Default::default();
        }
        if line.trim().is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Default::default();
        };
        let key = key.trim();
        if key.is_empty() || map.len() >= MAX_FRONT_MATTER_KEYS {
            return Default::default();
        }
        map.insert(key.to_string(), value.trim().to_string());
    }
    Default::default()
}

/// How one rule decides the kind of a file.
type SpecFileRule = fn(&str, &std::collections::BTreeMap<String, String>) -> Option<SpecNodeKind>;

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn rule_front_matter_kind(
    _path: &str,
    front: &std::collections::BTreeMap<String, String>,
) -> Option<SpecNodeKind> {
    front
        .get("kind")
        .and_then(|value| SpecNodeKind::from_wire(value))
}

fn rule_constitution_file(
    path: &str,
    _front: &std::collections::BTreeMap<String, String>,
) -> Option<SpecNodeKind> {
    (file_name(path) == "constitution.md").then_some(SpecNodeKind::Policy)
}

fn rule_tasks_file(
    path: &str,
    _front: &std::collections::BTreeMap<String, String>,
) -> Option<SpecNodeKind> {
    (file_name(path) == "tasks.md").then_some(SpecNodeKind::Task)
}

fn rule_goal_front_matter(
    _path: &str,
    front: &std::collections::BTreeMap<String, String>,
) -> Option<SpecNodeKind> {
    front
        .get("goal")
        .is_some_and(|value| !value.is_empty())
        .then_some(SpecNodeKind::Goal)
}

fn rule_memory_folder(
    path: &str,
    _front: &std::collections::BTreeMap<String, String>,
) -> Option<SpecNodeKind> {
    let mut segments: Vec<&str> = path.split('/').collect();
    let name = segments.pop().unwrap_or_default();
    (name.ends_with(".md") && segments.contains(&"memory")).then_some(SpecNodeKind::Memory)
}

fn rule_spec_document(
    path: &str,
    _front: &std::collections::BTreeMap<String, String>,
) -> Option<SpecNodeKind> {
    const NAMES: [&str; 6] = [
        "spec.md",
        "plan.md",
        "research.md",
        "quickstart.md",
        "data-model.md",
        "requirements.md",
    ];
    NAMES
        .contains(&file_name(path))
        .then_some(SpecNodeKind::Document)
}

/// The named classification rules in order; the first match wins. The name is
/// stored as `:classifiedByRule`.
pub const DEFAULT_SPEC_FILE_RULES: [(&str, SpecFileRule); 6] = [
    ("front-matter-kind", rule_front_matter_kind),
    ("constitution-file", rule_constitution_file),
    ("tasks-file", rule_tasks_file),
    ("goal-front-matter", rule_goal_front_matter),
    ("memory-folder", rule_memory_folder),
    ("spec-document", rule_spec_document),
];

/// Classify one spec file by the first matching rule of
/// [`DEFAULT_SPEC_FILE_RULES`]. Returns the kind and the name of the rule that
/// decided it (stored as `:classifiedByRule`).
pub fn classify_spec_file_by_rule(
    relative_path: &str,
    front_matter: &std::collections::BTreeMap<String, String>,
) -> Result<(SpecNodeKind, &'static str), SpecFolderRefusal> {
    DEFAULT_SPEC_FILE_RULES
        .iter()
        .find_map(|(name, rule)| rule(relative_path, front_matter).map(|kind| (kind, *name)))
        .ok_or(SpecFolderRefusal::UnknownKind)
}

/// Classify one spec file by named rule (MEDULLA-ORCH-VOCAB-R012.2).
pub fn classify_spec_file(
    relative_path: &str,
    front_matter: &std::collections::BTreeMap<String, String>,
) -> Result<SpecNodeKind, SpecFolderRefusal> {
    classify_spec_file_by_rule(relative_path, front_matter).map(|(kind, _)| kind)
}
