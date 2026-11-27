//! The `retrieval_plan` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! A declared candidate claiming the reserved registry namespace must name
//! a registered retrieval plan kind (MEDULLA-DECISION-ENGINE-R029); the gate
//! runs at the candidates stage.

use medulla_agent_types::agent_component::{AgentComponentKind, ComponentDependency};
use medulla_agent_types::agent_library::AgentLibraryLifecycle;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::retrieval_plan::{
    is_prefixed_sha256, RetrievalPlanKind, RetrievalProviderBinding, COMMAND_CONFIG_DIGEST_PARAM,
    INFERENCE_MODE_BINDING_INCOMPLETE, INFERENCE_MODE_DIGEST_INVALID, INFERENCE_MODE_DIGEST_PARAM,
    PROVIDER_PARAM, REASONING_MODEL_PARAM, RETRIEVAL_PROVIDER_STALE,
    RETRIEVAL_PROVIDER_UNAVAILABLE,
};
use medulla_decision_types::decision::statistical::{
    DecideRequest, StatisticalErrorCode, TypedValue,
};
use medulla_types::refusal_code::RefusalCode;

use medulla_decision_types::decision::{PremiseClass, PremiseRef};

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_support::{invalid_parameter, refusal};

use super::DecisionFamily;

/// The family of the `retrieval_plan` question kind.
pub struct RetrievalPlanFamily;

/// The component pin a parameter names: a JSON `ComponentDependency` in a
/// `Text` value. `None` when the parameter is absent.
fn pin_param(request: &DecideRequest, name: &str) -> Result<Option<ComponentDependency>, String> {
    let Some(param) = request.params.iter().find(|p| p.name == name) else {
        return Ok(None);
    };
    let TypedValue::Text(text) = &param.value else {
        return Err(invalid_parameter(format!(
            "{name} must be a text value holding a component pin"
        )));
    };
    serde_json::from_str(text)
        .map(Some)
        .map_err(|error| invalid_parameter(format!("{name} is not a component pin: {error}")))
}

fn coded(code: RefusalCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

/// Resolve one pin to the revision it names, independently of every other pin.
/// A component the tenant cannot see, or one that is retired or withdrawn, is
/// unavailable; a live component at another revision than the pin names is
/// stale. No other component ever stands in for it.
fn resolve_pin(
    ctx: &ExecutionContext,
    role: &str,
    pin: &ComponentDependency,
    allowed: &[AgentComponentKind],
) -> Result<medulla_agent_types::agent_component::AgentComponentEntry, String> {
    if !allowed.contains(&pin.kind) {
        return Err(invalid_parameter(format!(
            "{role} {} is pinned as {:?}, which cannot serve that role",
            pin.component_id, pin.kind
        )));
    }
    let head = ctx
        .store
        .current_component(ctx.tenant_id, &pin.component_id)?
        .filter(|entry| entry.kind == pin.kind)
        .ok_or_else(|| {
            coded(
                RETRIEVAL_PROVIDER_UNAVAILABLE,
                format!("{role} {} is not visible to this tenant", pin.component_id),
            )
        })?;
    if head.lifecycle != AgentLibraryLifecycle::Published {
        return Err(coded(
            RETRIEVAL_PROVIDER_UNAVAILABLE,
            format!("{role} {} is {:?}", pin.component_id, head.lifecycle),
        ));
    }
    if head.definition_digest != pin.definition_digest {
        return Err(coded(
            RETRIEVAL_PROVIDER_STALE,
            format!(
                "{role} {} is pinned at {} but is now at {}",
                pin.component_id, pin.definition_digest, head.definition_digest
            ),
        ));
    }
    Ok(head)
}

/// The command-config and inference-mode digests the request carries: both or
/// neither, each `sha256:` plus 64 lowercase hex characters
/// (MEDULLA-DECISION-ENGINE-R029.100).
fn mode_digests(request: &DecideRequest) -> Result<Option<(String, String)>, String> {
    let text = |name: &str| -> Result<Option<String>, String> {
        match request.params.iter().find(|p| p.name == name) {
            None => Ok(None),
            Some(param) => match &param.value {
                TypedValue::Text(value) => Ok(Some(value.clone())),
                _ => Err(invalid_parameter(format!("{name} must be a text value"))),
            },
        }
    };
    let config = text(COMMAND_CONFIG_DIGEST_PARAM)?;
    let mode = text(INFERENCE_MODE_DIGEST_PARAM)?;
    let (config, mode) = match (config, mode) {
        (None, None) => return Ok(None),
        (Some(config), Some(mode)) => (config, mode),
        _ => {
            return Err(coded(
                INFERENCE_MODE_BINDING_INCOMPLETE,
                format!(
                    "{COMMAND_CONFIG_DIGEST_PARAM} and {INFERENCE_MODE_DIGEST_PARAM} \
                     are supplied together or not at all"
                ),
            ))
        }
    };
    for (name, value) in [
        (COMMAND_CONFIG_DIGEST_PARAM, &config),
        (INFERENCE_MODE_DIGEST_PARAM, &mode),
    ] {
        if !is_prefixed_sha256(value) {
            return Err(coded(
                INFERENCE_MODE_DIGEST_INVALID,
                format!("{name} must be sha256: plus 64 lowercase hex characters"),
            ));
        }
    }
    Ok(Some((config, mode)))
}

impl DecisionFamily for RetrievalPlanFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        mode_digests(request).map(|_| ())
    }

    fn premises(
        &self,
        request: &DecideRequest,
        _executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        let Ok(Some((config, mode))) = mode_digests(request) else {
            return Vec::new();
        };
        [
            (
                "command_config_digest",
                config,
                "params.retrieval.command_config_digest",
            ),
            (
                "inference_mode_digest",
                mode,
                "params.retrieval.inference_mode_digest",
            ),
        ]
        .into_iter()
        .map(|(name, digest, field)| {
            FamilyPremise {
                subject: "retrieval_plan".into(),
                fact: format!("{name}={digest}"),
                class: PremiseClass::Claim,
                source: FamilyPremiseSource::Request(field),
            }
            .seal(policy_digest)
        })
        .collect()
    }

    fn bind(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
    ) -> Result<Option<RetrievalProviderBinding>, String> {
        let provider = pin_param(request, PROVIDER_PARAM)?;
        let model = pin_param(request, REASONING_MODEL_PARAM)?;
        let Some(provider) = provider else {
            if model.is_some() {
                return Err(invalid_parameter(format!(
                    "{REASONING_MODEL_PARAM} needs {PROVIDER_PARAM}"
                )));
            }
            return Ok(None);
        };
        let entry = resolve_pin(
            ctx,
            "retrieval provider",
            &provider,
            &[
                AgentComponentKind::Tool,
                AgentComponentKind::Toolset,
                AgentComponentKind::McpServer,
            ],
        )?;
        if let Some(model) = &model {
            resolve_pin(
                ctx,
                "reasoning model",
                model,
                &[AgentComponentKind::ModelProfile],
            )?;
        }
        Ok(Some(RetrievalProviderBinding {
            provider,
            config_digest: entry.content_digest,
            request_digest: medulla_query_types::solve_scalar::digest_text(
                "eg/retrieval-binding-request/v1",
                &(&request.question, request.params.as_slice()),
            ),
            tenant_id: ctx.tenant_id.to_string(),
            reasoning_model: model,
        }))
    }

    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        for option in options {
            if RetrievalPlanKind::claims_registry_namespace(&option.option_id)
                && RetrievalPlanKind::from_option_id(&option.option_id).is_none()
            {
                return Err(refusal(
                    StatisticalErrorCode::ParameterInvalid,
                    format!(
                        "UNSUPPORTED_RETRIEVAL_PLAN: '{}' is not in the retrieval plan \
                         registry (leanrag, reciprocal-rank-fusion, direct-sparql)",
                        option.option_id
                    ),
                ));
            }
        }
        Ok(())
    }
}
