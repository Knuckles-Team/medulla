//! The engine's published refusal-code registry: every closed refusal-code
//! vocabulary a served response may carry, read across all domains. It sits in
//! the protocol hub because it names every domain's codes.

/// Whether a response prefix is one of the engine's published refusal codes.
/// The enums remain the single source of truth for the generated catalog.
pub fn declared_error_code(code: &str) -> bool {
    [
        medulla_types::contract::EngineErrorCode::ALL
            .iter()
            .any(|item| item.as_str() == code),
        medulla_types::contract::ServerErrorCode::ALL
            .iter()
            .any(|item| item.as_str() == code),
        medulla_types::contract::UqlCode::ALL
            .iter()
            .any(|item| item.as_str() == code),
        medulla_decision_types::decision::DecisionErrorCode::ALL
            .iter()
            .any(|item| item.as_str() == code),
        medulla_decision_types::decision::statistical::StatisticalErrorCode::ALL
            .iter()
            .any(|item| item.as_str() == code),
        medulla_types::orchestration::HarnessVocabularyErrorCode::ALL
            .iter()
            .any(|item| item.as_str() == code),
        medulla_decision_types::decision::harness_contrast::HarnessContrastErrorCode::ALL
            .iter()
            .any(|item| item.as_str() == code),
        medulla_catalog_types::graph_schema::GraphSchemaErrorCode::ALL
            .iter()
            .any(|item| item.as_str() == code),
        medulla_catalog_types::connector_pack::result::PackWriteErrorCode::ALL
            .iter()
            .any(|item| item.as_str() == code),
        medulla_decision_types::solve::SolveErrorCode::ALL
            .iter()
            .any(|item| item.as_str() == code),
    ]
    .into_iter()
    .any(|known| known)
}

/// Classify a refusal for `Response::err` without hiding a code it already
/// carries. A declared `CODE: detail` keeps its code with `context` placed
/// before the detail; a bare declared code passes through unchanged; free text
/// is reported under `default` with `context` kept as its human prefix.
pub fn classify_refusal(default: &str, context: &str, refusal: &str) -> String {
    match refusal.split_once(": ") {
        Some((code, detail)) if declared_error_code(code) => format!("{code}: {context}{detail}"),
        _ if declared_error_code(refusal) => refusal.to_string(),
        _ => format!("{default}: {context}{refusal}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_refusal, declared_error_code};

    #[test]
    fn classify_refusal_keeps_declared_codes_and_classifies_free_text() {
        assert_eq!(
            classify_refusal(
                "INVALID_ARGUMENT",
                "SQL error: ",
                "ACCESS_DENIED: table hidden"
            ),
            "ACCESS_DENIED: SQL error: table hidden"
        );
        assert_eq!(
            classify_refusal("INVALID_ARGUMENT", "SQL error: ", "ACCESS_DENIED"),
            "ACCESS_DENIED"
        );
        assert_eq!(
            classify_refusal("INVALID_ARGUMENT", "SQL error: ", "table t does not exist"),
            "INVALID_ARGUMENT: SQL error: table t does not exist"
        );
        assert_eq!(
            classify_refusal("INTERNAL", "", "Not A Code: boom"),
            "INTERNAL: Not A Code: boom"
        );
    }

    /// A refusal is served as its wire token, and the served boundary keeps only
    /// tokens the engine declares: an undeclared one would reach the caller as an
    /// unclassified internal error.
    #[test]
    fn every_refusal_is_reported_as_a_declared_wire_code() {
        for refusal in medulla_types::identity::IdentityRefusal::ALL {
            let code = refusal.to_string();
            assert!(code.starts_with("IDENTITY_"), "{code}");
            assert!(declared_error_code(&code), "{code}");
        }
    }
}
