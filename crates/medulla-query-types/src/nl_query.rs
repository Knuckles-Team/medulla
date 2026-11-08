//! The disclosed natural-language query result on the wire
//! (MEDULLA-FEDERATED-QUERY-R058.2.2.1).
//!
//! `Method::NlQueryExplain` answers with the exact UQL text the configured planner
//! produced and the engine executed, alongside the SAME `[node id, score | nil]` rows
//! `Method::NlQuery` returns for that text. A caller can therefore show the generated
//! query next to its rows instead of trusting an opaque natural-language answer.

use serde::{Deserialize, Serialize};

/// Most characters an [`NlTurn::question`] (and a rewritten question) may hold
/// (MEDULLA-FEDERATED-QUERY-R058.8).
pub const NL_TURN_QUESTION_MAX_CHARS: usize = 2000;
/// Most characters an [`NlTurn::answer_summary`] may hold (MEDULLA-FEDERATED-QUERY-R058.8).
pub const NL_TURN_ANSWER_SUMMARY_MAX_CHARS: usize = 1000;

/// One earlier turn of a conversation, oldest first on the wire
/// (MEDULLA-FEDERATED-QUERY-R058.8). A follow-up question is rewritten to a
/// standalone question against these turns before planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct NlTurn {
    /// The earlier question, 1 to 2000 characters.
    pub question: String,
    /// A summary of the earlier answer, at most 1000 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_summary: Option<String>,
}

/// Check `history` against the turn bounds and `max_turns`
/// (MEDULLA-FEDERATED-QUERY-R058.8). The error names the offending field.
pub fn validate_nl_history(history: &[NlTurn], max_turns: usize) -> Result<(), String> {
    if history.len() > max_turns {
        return Err(format!(
            "history holds {} turns, more than max_history_turns {max_turns}",
            history.len()
        ));
    }
    for (i, turn) in history.iter().enumerate() {
        let chars = turn.question.chars().count();
        if chars == 0 || chars > NL_TURN_QUESTION_MAX_CHARS {
            return Err(format!(
                "history[{i}].question holds {chars} characters, outside 1..={NL_TURN_QUESTION_MAX_CHARS}"
            ));
        }
        if let Some(summary) = &turn.answer_summary {
            let chars = summary.chars().count();
            if chars > NL_TURN_ANSWER_SUMMARY_MAX_CHARS {
                return Err(format!(
                    "history[{i}].answer_summary holds {chars} characters, more than {NL_TURN_ANSWER_SUMMARY_MAX_CHARS}"
                ));
            }
        }
    }
    Ok(())
}

/// The executed UQL and its rows, the served form of `medulla_plan::nl::NlQueryResult` in
/// executing mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct NlQueryExplainResult {
    /// The exact UQL text the planner produced; the engine parsed and ran this text.
    pub uql: String,
    /// The `[node id, score | nil]` rows of that UQL, identical to `NlQuery`'s rows.
    pub rows: Vec<(String, Option<f32>)>,
    /// The standalone question planned in place of a follow-up; `Some` exactly when the
    /// request carried history (MEDULLA-FEDERATED-QUERY-R058.8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewritten_question: Option<String>,
}

/// The unexecuted natural-language candidate and the pins for the exact
/// UQL/empty-parameter request. A caller may use this receipt to obtain its
/// approved foreign execution and then dispatch that exact UQL via `Method::Uql`;
/// the NL planner is never run again for that execution. Plan-only does not read
/// graph data or establish source authority; those checks remain on the later
/// exact-UQL admission path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct NlQueryPlanResult {
    /// The exact UQL produced by the server-owned planner.
    pub uql: String,
    /// SHA-256 of the parsed plan for the exact UQL.
    pub plan_digest: medulla_types::contract::Digest256,
    /// SHA-256 of the exact UQL and typed empty parameter map.
    pub query_digest: medulla_types::contract::Digest256,
    /// The standalone question planned in place of a follow-up; `Some` exactly when the
    /// request carried history (MEDULLA-FEDERATED-QUERY-R058.8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewritten_question: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R058.2.2.1
    #[test]
    fn explain_result_encodes_uql_and_rows_by_name() {
        let result = NlQueryExplainResult {
            uql: "MATCH (:Doc) |> LIMIT 2".into(),
            rows: vec![("d1".into(), Some(0.5)), ("d2".into(), None)],
            rewritten_question: None,
        };
        let bytes = rmp_serde::to_vec_named(&result).unwrap();
        let value: serde_json::Value = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"uql": "MATCH (:Doc) |> LIMIT 2", "rows": [["d1", 0.5], ["d2", null]]})
        );
        let back: NlQueryExplainResult = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(back, result);
    }

    #[test]
    fn plan_result_encodes_exact_uql_and_server_pins() {
        let digest = medulla_types::contract::Digest256::sha256(b"pin");
        let result = NlQueryPlanResult {
            uql: "MATCH (:Doc) |> LIMIT 1".into(),
            plan_digest: digest,
            query_digest: digest,
            rewritten_question: Some("Which tickets does Ann own?".into()),
        };
        let bytes = rmp_serde::to_vec_named(&result).unwrap();
        let back: NlQueryPlanResult = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(back, result);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R058.8
    #[test]
    fn history_bounds_name_the_field() {
        let turn = |q: &str| NlTurn {
            question: q.into(),
            answer_summary: None,
        };
        assert_eq!(validate_nl_history(&[turn("who owns T1?")], 4), Ok(()));
        let err = validate_nl_history(&[turn("a"), turn("b")], 1).unwrap_err();
        assert!(err.contains("max_history_turns"), "{err}");
        let err = validate_nl_history(&[turn("")], 4).unwrap_err();
        assert!(err.contains("history[0].question"), "{err}");
        let long = NlTurn {
            question: "q".into(),
            answer_summary: Some("x".repeat(1001)),
        };
        let err = validate_nl_history(&[long], 4).unwrap_err();
        assert!(err.contains("history[0].answer_summary"), "{err}");
    }
}
