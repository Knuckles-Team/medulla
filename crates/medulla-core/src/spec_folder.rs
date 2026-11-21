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
