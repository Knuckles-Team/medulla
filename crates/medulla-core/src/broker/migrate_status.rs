//! Conversion of stored broker message `status` text to the `MessageState` vocabulary
//! (MEDULLA-CQ-ORCH-R810.4 foundation).
//!
//! Before the message machine a stored `claimed` value meant two things depending on the clock.
//! `MessageState` makes it mean a live lease only. `convert_status` decides the new stored text of
//! one old message; the offline layout upgrade applies it to every stored message through the
//! durable mutation path. Nothing here runs at an ordinary open.

use super::message_machine::{MessageEvent, MessageState};
use super::{consume_scan, f_str};
use medulla_types::transition_refused::MachineState;

/// A stored status the conversion does not know. The upgrade refuses the whole store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownStatus {
    /// The text found in the stored `status` field.
    pub found: String,
}

/// The new stored `status` text of one old message at `now_ms`, or `None` when it is already right.
///
/// `claimed` with a lapsed lease becomes `pending` (the `LeaseLapse` edge); `claimed` with a live
/// lease stays; the old hyphenated `dead-lettered` becomes `dead_lettered`; every other text that is
/// not a `MessageState` stored form is refused.
pub fn convert_status(
    obj: &serde_json::Map<String, serde_json::Value>,
    now_ms: u64,
) -> Result<Option<&'static str>, UnknownStatus> {
    let text = f_str(obj, "status");
    if text == "dead-lettered" {
        return Ok(Some(MessageState::DeadLettered.stored()));
    }
    let state = match text {
        "pending" => MessageState::Pending,
        "claimed" => MessageState::Claimed,
        "acked" => MessageState::Acked,
        "expired" => MessageState::Expired,
        "dead_lettered" => MessageState::DeadLettered,
        other => {
            return Err(UnknownStatus {
                found: other.to_string(),
            })
        }
    };
    if state == MessageState::Claimed {
        let facts = consume_scan::message_facts(obj);
        if let Ok(next) = state.apply(MessageEvent::LeaseLapse, facts.guard(now_ms)) {
            return Ok(Some(next.stored()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(status: &str, lease_until: Option<u64>) -> serde_json::Map<String, serde_json::Value> {
        let mut v = json!({"status": status});
        if let Some(l) = lease_until {
            v["lease_until"] = json!(l);
        }
        v.as_object().unwrap().clone()
    }

    // spec: MEDULLA-CQ-ORCH-R810.4
    #[test]
    fn lapsed_claim_becomes_pending() {
        assert_eq!(
            convert_status(&msg("claimed", Some(100)), 200),
            Ok(Some("pending"))
        );
    }

    #[test]
    fn live_claim_stays_claimed() {
        assert_eq!(convert_status(&msg("claimed", Some(300)), 200), Ok(None));
    }

    #[test]
    fn dead_lettered_text_is_renamed() {
        assert_eq!(
            convert_status(&msg("dead-lettered", None), 200),
            Ok(Some("dead_lettered"))
        );
    }

    #[test]
    fn unknown_text_is_refused() {
        assert_eq!(
            convert_status(&msg("done", None), 200),
            Err(UnknownStatus {
                found: "done".into()
            })
        );
    }
}
