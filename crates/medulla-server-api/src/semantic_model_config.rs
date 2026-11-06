//! The engine-owned semantic model config of the promotion gate
//! (MEDULLA-UNIFIED-DATA-PLANE-R040.3, CONTRACT-R058).
//!
//! `semantic.model_config` names an absolute path to one JSON
//! [`SemanticModelConfig`]: the model id, the operator-bumped
//! `model_version`, the served modalities and one digest-pinned probe set per
//! modality. It is a structured value, not a scalar, so it is read here beside
//! the scalar knob table, from the last `--knob semantic.model_config=<path>`
//! or `MEDULLA_SEMANTIC_MODEL_CONFIG`. An unreadable, malformed or invalid
//! config refuses startup with `KNOB_INVALID`; absent means no gated model.

use std::sync::OnceLock;

use medulla_types::contract::ServerErrorCode;
use medulla_types::embedding_promotion::SemanticModelConfig;

use crate::knob_config::KnobRefusal;

/// The server config key naming the model config file.
pub const SEMANTIC_MODEL_CONFIG: &str = "semantic.model_config";

fn invalid(detail: String) -> KnobRefusal {
    KnobRefusal {
        code: ServerErrorCode::KnobInvalid,
        knob: SEMANTIC_MODEL_CONFIG.to_string(),
        detail,
    }
}

/// Parse and validate one config body.
pub fn parse_semantic_model_config(body: &str) -> Result<SemanticModelConfig, KnobRefusal> {
    let config: SemanticModelConfig =
        serde_json::from_str(body).map_err(|error| invalid(format!("malformed: {error}")))?;
    config
        .validate()
        .map_err(|error| invalid(format!("invalid: {error}")))?;
    Ok(config)
}

/// Read `semantic.model_config` at startup.
pub fn load_semantic_model_config(
    args: &[String],
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<SemanticModelConfig>, KnobRefusal> {
    let from_cli = args.iter().rev().find_map(|arg| {
        let (name, value) = arg.split_once('=')?;
        (name.trim() == SEMANTIC_MODEL_CONFIG).then(|| value.trim().to_string())
    });
    let Some(raw) = from_cli.or_else(|| env("MEDULLA_SEMANTIC_MODEL_CONFIG")) else {
        return Ok(None);
    };
    let path = std::path::PathBuf::from(&raw);
    if !path.is_absolute() {
        return Err(invalid(format!("path {raw:?} is not absolute")));
    }
    let body = std::fs::read_to_string(&path)
        .map_err(|error| invalid(format!("unreadable {raw:?}: {error}")))?;
    parse_semantic_model_config(&body).map(Some)
}

static INSTALLED: OnceLock<Option<SemanticModelConfig>> = OnceLock::new();

/// Hand the startup-loaded config to the semantic stores; the first install wins.
pub fn install_semantic_model_config(config: Option<SemanticModelConfig>) {
    let _ = INSTALLED.set(config);
}

/// The startup-loaded model config, `None` when none was configured.
pub fn semantic_model_config() -> Option<&'static SemanticModelConfig> {
    INSTALLED.get().and_then(Option::as_ref)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R040.3
    #[test]
    fn model_config_key_refuses_bad_values() {
        let env = |_: &str| None;
        assert_eq!(load_semantic_model_config(&[], env), Ok(None));
        let relative = vec![format!("{SEMANTIC_MODEL_CONFIG}=relative.json")];
        let missing = vec![format!("{SEMANTIC_MODEL_CONFIG}=/nonexistent/model.json")];
        for args in [relative, missing] {
            let refusal = load_semantic_model_config(&args, env).unwrap_err();
            assert_eq!(refusal.code, ServerErrorCode::KnobInvalid);
        }
        for body in ["{", "{\"model_id\":\"m\"}"] {
            assert!(parse_semantic_model_config(body).is_err());
        }
    }
}
