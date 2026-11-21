//! The closed status of one operation-audit event (MEDULLA-DURABLE-KERNEL-R874.12,
//! organization constitution Article XXXI).
//!
//! A reservation is written before the effect and an outcome closes it. The four stored
//! texts are the only ones a record can carry; any other text is refused when decoded.
//! The transition function lives in [`super::audit_machine`].

use medulla_types::transition_refused::MachineState;

/// The refusal text of an audit event whose fields are not privacy-safe, status included.
pub const AUDIT_FIELDS_INVALID: &str = "invalid privacy-safe audit event fields";

/// The closed state set of `AuditMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditStatus {
    /// The reservation, written before the effect runs.
    Reserved,
    /// The effect succeeded.
    Ok,
    /// The effect failed.
    Error,
    /// The effect was refused.
    Denied,
}

impl AuditStatus {
    /// Every status, in declaration order.
    pub const ALL: [Self; 4] = [Self::Reserved, Self::Ok, Self::Error, Self::Denied];

    /// The stored text of this status.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Denied => "denied",
        }
    }

    /// Decode stored or requested text; any text outside the four is refused.
    pub fn parse(text: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == text)
            .ok_or_else(|| AUDIT_FIELDS_INVALID.to_string())
    }

    /// Whether this status is the reservation that an outcome closes.
    pub fn is_reservation(self) -> bool {
        self == Self::Reserved
    }
}

impl MachineState for AuditStatus {
    fn stored(self) -> &'static str {
        self.as_str()
    }
}
