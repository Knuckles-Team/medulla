//! The `credential_reuse` question family (MEDULLA-DECISION-ENGINE-R832).
//!
//! Whether an agent may reuse an owner's stored credential binding for one
//! connector, provider, scope set and purpose. The binding the caller holds
//! arrives in the `binding` parameter (the owner's record, never the secret);
//! the request names the use. The family answers one of three outcomes:
//! `admit`, `deny:<CODE>` (the binding exists and the use falls outside it, the
//! code is the reason) or an abstention (`CREDENTIAL_BINDING_MISSING`, no
//! binding to decide on). Expiry reads the engine clock (`ctx.now_ms`). It
//! writes nothing; `DecisionLog` persists the record, whose evidence class is
//! `Claim`.

use serde::Deserialize;

use medulla_decision_types::decision::codes;
use medulla_decision_types::decision::record::outcome::DecisionRefusal;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef, ResolutionKind};
use medulla_graph_core::graph::GraphView;
use medulla_types::evidence_class::EvidenceClass;
use medulla_types::refusal_code::RefusalCode;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{abstained, advisory, invalid, json_param, param, single_option, text_param};
use super::DecisionFamily;

/// The option an admitted use names.
pub const ADMIT_OPTION: &str = "admit";
/// The prefix of the option a denied use names; the registered code follows.
pub const DENY_PREFIX: &str = "deny:";

/// The family of the `credential_reuse` question kind.
pub struct CredentialReuseFamily;

/// The owner's stored binding of one credential, as the caller holds it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    owner: String,
    agents: Vec<String>,
    connectors: Vec<String>,
    providers: Vec<String>,
    scopes: Vec<String>,
    purposes: Vec<String>,
    expires_at_ms: u64,
    #[serde(default)]
    revoked: bool,
    #[serde(default)]
    revision: u64,
}

/// The use the question asks about.
struct Use<'a> {
    owner: &'a str,
    agent_id: &'a str,
    connector: &'a str,
    provider: &'a str,
    purpose: &'a str,
    scopes: Vec<String>,
}

/// Why a use falls outside its binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Breach {
    OwnerMismatch,
    AgentNotBound,
    Revoked,
    Expired,
    ConnectorNotBound,
    ScopeExceeded,
    PurposeNotAllowed,
}

impl Breach {
    fn code(self) -> RefusalCode {
        match self {
            Self::OwnerMismatch => codes::CREDENTIAL_OWNER_MISMATCH,
            Self::AgentNotBound => codes::CREDENTIAL_AGENT_NOT_BOUND,
            Self::Revoked => codes::CREDENTIAL_REVOKED,
            Self::Expired => codes::CREDENTIAL_EXPIRED,
            Self::ConnectorNotBound => codes::CREDENTIAL_CONNECTOR_NOT_BOUND,
            Self::ScopeExceeded => codes::CREDENTIAL_SCOPE_EXCEEDED,
            Self::PurposeNotAllowed => codes::CREDENTIAL_PURPOSE_NOT_ALLOWED,
        }
    }
}

/// Every way `usage` falls outside `binding` at `now_ms`, in code order.
fn breaches(binding: &Binding, usage: &Use<'_>, now_ms: u64) -> Vec<Breach> {
    let mut found = Vec::new();
    if binding.owner != usage.owner {
        found.push(Breach::OwnerMismatch);
    }
    if !binding.agents.iter().any(|agent| agent == usage.agent_id) {
        found.push(Breach::AgentNotBound);
    }
    if binding.revoked {
        found.push(Breach::Revoked);
    }
    if binding.expires_at_ms <= now_ms {
        found.push(Breach::Expired);
    }
    if !binding.connectors.iter().any(|c| c == usage.connector)
        || !binding.providers.iter().any(|p| p == usage.provider)
    {
        found.push(Breach::ConnectorNotBound);
    }
    if !usage.scopes.iter().all(|s| binding.scopes.contains(s)) {
        found.push(Breach::ScopeExceeded);
    }
    if !binding.purposes.iter().any(|p| p == usage.purpose) {
        found.push(Breach::PurposeNotAllowed);
    }
    found
}

/// The binding is absent: nothing to decide on.
struct BindingMissing;

impl DecisionRefusal for BindingMissing {
    fn code(&self) -> RefusalCode {
        codes::CREDENTIAL_BINDING_MISSING
    }

    fn detail(&self) -> String {
        "no credential binding accompanies the question".to_string()
    }
}

fn required<'a>(request: &'a DecideRequest, name: &str) -> Result<&'a str, String> {
    text_param(request, name)?
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid(format!("a credential_reuse question requires {name}")))
}

fn usage(request: &DecideRequest) -> Result<Use<'_>, String> {
    let scopes: Vec<String> = json_param(request, "scopes")?;
    if scopes.iter().any(|scope| scope.trim().is_empty()) {
        return Err(invalid("parameter scopes holds an empty scope"));
    }
    Ok(Use {
        owner: required(request, "owner")?,
        agent_id: required(request, "agent_id")?,
        connector: required(request, "connector")?,
        provider: required(request, "provider")?,
        purpose: required(request, "purpose")?,
        scopes,
    })
}

fn conclude_reuse(request: &DecideRequest, now_ms: u64) -> Result<Executed, String> {
    let usage = usage(request)?;
    if param(request, "binding")?.is_none() {
        return Ok(abstained(QuestionKind::CredentialReuse, &BindingMissing));
    }
    let binding: Binding = json_param(request, "binding")?;
    let found = breaches(&binding, &usage, now_ms);
    let mut premises = vec![FamilyPremise {
        subject: "credential".into(),
        fact: format!("credential.binding_revision={}", binding.revision),
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request("params.binding"),
    }];
    let mut executed = match found.first() {
        None => advisory(vec![ADMIT_OPTION.to_string()])?,
        Some(first) => {
            premises.extend(found.iter().map(|breach| FamilyPremise {
                subject: "credential".into(),
                fact: format!("credential.reason={}", breach.code().as_str()),
                class: PremiseClass::Definition,
                source: FamilyPremiseSource::Request("params.binding"),
            }));
            advisory(vec![format!("{DENY_PREFIX}{}", first.code().as_str())])?
        }
    };
    executed.family_premises = premises;
    Ok(executed)
}

impl DecisionFamily for CredentialReuseFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, ADMIT_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        usage(request).map(|_| ())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_reuse(request, ctx.now_ms))
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Claim)
    }
}
