//! Table triggers (MEDULLA-ENGINE-PARITY-R012.1): a trigger binds a statechart
//! to the row events of one table. The definitions live in the table's schema
//! record, so they are versioned with the table's DDL and dropped with it.

use medulla_statechart::StatechartDef;
use serde::{Deserialize, Serialize};

/// A trigger's own statement was refused by its machine (`refuse` effect).
pub const TRIGGER_REFUSED: &str = "TRIGGER_REFUSED";
/// A trigger of that name already exists on the table.
pub const TRIGGER_EXISTS: &str = "TRIGGER_EXISTS";
/// No trigger of that name exists on the table.
pub const TRIGGER_UNKNOWN: &str = "TRIGGER_UNKNOWN";
/// A trigger definition or its machine cannot run (bad alphabet, unknown column,
/// invalid chart).
pub const TRIGGER_MACHINE_INVALID: &str = "TRIGGER_MACHINE_INVALID";

/// When a trigger reacts relative to the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerTiming {
    /// Before the row is written; the machine may rewrite or refuse the row.
    Before,
    /// After the row is written; the machine may only emit effects.
    After,
}

impl TriggerTiming {
    /// The event-name prefix.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
        }
    }
}

/// The row event a trigger reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerEvent {
    /// A row insert.
    Insert,
    /// A row update.
    Update,
    /// A row delete.
    Delete,
}

impl TriggerEvent {
    /// The event-name suffix.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

/// One trigger of a table. `machine_id` is the content id of `machine`
/// (`StatechartDef::def_id`); the definition is embedded so a write needs no
/// second store read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerSpec {
    /// The table the trigger belongs to.
    pub table: String,
    /// Trigger name, unique within the table.
    pub name: String,
    /// Before or after the write.
    pub timing: TriggerTiming,
    /// The row events it reacts to (non-empty).
    pub events: Vec<TriggerEvent>,
    /// Content id of `machine`.
    pub machine_id: String,
    /// The machine the trigger runs.
    pub machine: StatechartDef,
}

impl TriggerSpec {
    /// Build a trigger over `machine`, deriving `machine_id` from its content.
    pub fn new(
        table: impl Into<String>,
        name: impl Into<String>,
        timing: TriggerTiming,
        events: Vec<TriggerEvent>,
        machine: StatechartDef,
    ) -> Self {
        Self {
            table: table.into(),
            name: name.into(),
            timing,
            events,
            machine_id: machine.def_id(),
            machine,
        }
    }

    /// The statechart event name this trigger sends for `event`
    /// (`before_insert`, `after_update`, ...).
    pub fn event_name(&self, event: TriggerEvent) -> String {
        format!("{}_{}", self.timing.as_str(), event.as_str())
    }

    /// Whether the trigger reacts to `event`.
    pub fn reacts_to(&self, event: TriggerEvent) -> bool {
        self.events.contains(&event)
    }

    /// Definition-time checks: a name, at least one event, a machine whose id
    /// matches, a valid chart and an alphabet that names every event the trigger
    /// reacts to.
    pub fn validate(&self) -> Result<(), String> {
        let refuse = |why: String| {
            Err(format!(
                "{TRIGGER_MACHINE_INVALID}: trigger `{}` on `{}`: {why}",
                self.name, self.table
            ))
        };
        if self.name.is_empty() || self.name.contains('\0') {
            return refuse("name must be non-empty and NUL-free".to_string());
        }
        if self.events.is_empty() {
            return refuse("at least one event is required".to_string());
        }
        if self.machine_id != self.machine.def_id() {
            return refuse("machine_id is not the content id of the machine".to_string());
        }
        if let Err(report) = medulla_statechart::check::validate(&self.machine) {
            return refuse(format!("machine is not a valid chart: {report:?}"));
        }
        for event in &self.events {
            let name = self.event_name(*event);
            if !self.machine.in_alphabet(&name) {
                return refuse(format!("machine alphabet lacks `{name}`"));
            }
        }
        Ok(())
    }
}
