//! Deterministic runtime rendering of a validated prompt blueprint
//! (MEDULLA-TYPED-PACKS-R001.8).
//!
//! The engine renders; the harness applies the bytes. [`render_prompt`] is a
//! pure function: it resolves only the variables the blueprint declares and
//! serializes the typed object as canonical JSON (keys sorted at every level)
//! under the versioned [`RenderPolicy`]. Equal inputs give equal bytes and the
//! same SHA-256 `render_digest`; any refusal yields no result.

use std::collections::{BTreeMap, BTreeSet};

use medulla_agent_types::agent_component::{facts::ValidatedPrompt, frame_pins};
use medulla_decision_outcome_types::decision::prompt_candidate::PromptCandidate;
use medulla_decision_types::decision::record::outcome::AbstainReason;
use medulla_types::contract::Digest256;
use serde_json::Value;

/// The only rendering version this engine produces.
pub const RENDERING_VERSION: u32 = 1;
/// Default for the config key `prompt.max_render_bytes`.
pub const DEFAULT_MAX_RENDER_BYTES: u32 = 1_048_576;
/// Admitted range for `prompt.max_render_bytes`, in bytes.
pub const MAX_RENDER_BYTES_RANGE: std::ops::RangeInclusive<u32> = 1024..=16_777_216;

/// The versioned rendering policy. The handler reads `prompt.max_render_bytes`
/// and passes it here; this crate reads no config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderPolicy {
    pub max_render_bytes: u32,
    pub rendering_version: u32,
}

impl Default for RenderPolicy {
    fn default() -> Self {
        Self {
            max_render_bytes: DEFAULT_MAX_RENDER_BYTES,
            rendering_version: RENDERING_VERSION,
        }
    }
}

/// The rendered bytes and their SHA-256 digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPrompt {
    pub bytes: Vec<u8>,
    pub render_digest: Digest256,
}

/// Why a render refused. A refusal produces no render result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalCode {
    /// A declared variable has no value.
    PromptVariableMissing,
    /// A value names a variable the blueprint does not declare.
    PromptVariableUnknown,
    /// The pins do not resolve: the blueprint body does not hash to the
    /// pinned `body_sha256`, or the policy names an unknown rendering version.
    PromptPinUnresolved,
    /// The rendered bytes exceed `max_render_bytes`.
    PromptOutputTooLarge,
}

impl RefusalCode {
    /// The wire refusal code.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PromptVariableMissing => "PROMPT_VARIABLE_MISSING",
            Self::PromptVariableUnknown => "PROMPT_VARIABLE_UNKNOWN",
            Self::PromptPinUnresolved => "PROMPT_PIN_UNRESOLVED",
            Self::PromptOutputTooLarge => "PROMPT_OUTPUT_TOO_LARGE",
        }
    }
}

/// Canonical JSON: object keys sorted at every level, no whitespace.
fn canonical(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            out.push(b'{');
            for (index, (key, item)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                canonical(&Value::String(key.clone()), out);
                out.push(b':');
                canonical(item, out);
            }
            out.push(b'}');
        }
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                canonical(item, out);
            }
            out.push(b']');
        }
        scalar => out.extend_from_slice(scalar.to_string().as_bytes()),
    }
}

fn canonical_bytes(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    canonical(value, &mut out);
    out
}

/// Render one validated prompt revision with its variable values.
pub fn render_prompt(
    revision: &ValidatedPrompt,
    vars: &BTreeMap<String, Value>,
    policy: &RenderPolicy,
) -> Result<RenderedPrompt, RefusalCode> {
    if policy.rendering_version != RENDERING_VERSION {
        return Err(RefusalCode::PromptPinUnresolved);
    }
    let identity = revision.identity();
    let body = canonical_bytes(revision.blueprint());
    if Digest256::sha256(&body).to_hex() != identity.body_sha256 {
        return Err(RefusalCode::PromptPinUnresolved);
    }
    let declared = revision.declared_variables();
    if vars.keys().any(|name| !declared.contains(name.as_str())) {
        return Err(RefusalCode::PromptVariableUnknown);
    }
    if declared.iter().any(|name| !vars.contains_key(*name)) {
        return Err(RefusalCode::PromptVariableMissing);
    }
    let rendered = serde_json::json!({
        "rendering_version": policy.rendering_version,
        "pins": Digest256::sha256(&frame_pins(identity)).to_hex(),
        "blueprint": revision.blueprint(),
        "variables": vars,
    });
    let bytes = canonical_bytes(&rendered);
    if bytes.len() > policy.max_render_bytes as usize {
        return Err(RefusalCode::PromptOutputTooLarge);
    }
    let render_digest = Digest256::sha256(&bytes);
    Ok(RenderedPrompt {
        bytes,
        render_digest,
    })
}

/// The prompt `select_system_prompt` picked and why (MEDULLA-DECISION-ENGINE-R814.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedPrompt {
    pub component_id: String,
    pub digest: String,
    /// `match_ratio * confidence`, Q32 (`1.0` is `1 << 32`).
    pub score_q32: i64,
}

/// Pick the system prompt for one slot (MEDULLA-DECISION-ENGINE-R814.2).
///
/// The match ratio is the share of `required` that a candidate `serves`. A
/// candidate is legal when its ratio is at least `min_match_micros` (the
/// `prompt_candidate.min_match_ratio` knob, millionths), compared exactly in
/// integers. The score is `match_ratio * confidence` in Q32; the highest score
/// wins and ties go to the lowest `component_id`. Only system-role candidates
/// are considered. When none is legal the answer is
/// `UncoveredCapability` naming the first required capability, in sorted
/// order, that no considered candidate serves (or the first required one when
/// every capability is served somewhere but no single prompt is legal). The
/// function never invents a prompt.
pub fn select_system_prompt(
    required: &BTreeSet<String>,
    candidates: &[PromptCandidate],
    min_match_micros: i64,
) -> Result<SelectedPrompt, AbstainReason> {
    let total = required.len() as u128;
    let mut best: Option<(i64, &PromptCandidate)> = None;
    for candidate in candidates.iter().filter(|c| c.is_system_candidate()) {
        let matched = required.intersection(&candidate.serves).count() as u128;
        let legal = if total == 0 {
            true
        } else {
            matched * 1_000_000 >= (min_match_micros.max(0) as u128) * total
        };
        if !legal || candidate.confidence_q32 < 0 {
            continue;
        }
        let ratio_q32 = if total == 0 {
            1u128 << 32
        } else {
            (matched << 32) / total
        };
        let score = ((ratio_q32 * candidate.confidence_q32 as u128) >> 32) as i64;
        let better = match best {
            None => true,
            Some((top, held)) => {
                score > top || (score == top && candidate.component_id < held.component_id)
            }
        };
        if better {
            best = Some((score, candidate));
        }
    }
    if let Some((score_q32, candidate)) = best {
        return Ok(SelectedPrompt {
            component_id: candidate.component_id.clone(),
            digest: candidate.digest.clone(),
            score_q32,
        });
    }
    let served: BTreeSet<&String> = candidates
        .iter()
        .filter(|c| c.is_system_candidate())
        .flat_map(|c| c.serves.iter())
        .collect();
    let iri = required
        .iter()
        .find(|iri| !served.contains(iri))
        .or_else(|| required.iter().next())
        .cloned()
        .unwrap_or_default();
    Err(AbstainReason::UncoveredCapability { iri })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_agent_types::agent_component::facts::{
        validate_prompt_blueprint, PromptLimits, PROMPT_SCHEMA_REVISION,
    };
    use medulla_agent_types::agent_component::{ComponentProvenance, PromptBlueprintIdentity};

    fn validated(blueprint: Value) -> ValidatedPrompt {
        let identity = PromptBlueprintIdentity {
            tenant_id: "tenant-a".into(),
            component_id: "prompt:a".into(),
            entry_revision: 2,
            schema_revision: PROMPT_SCHEMA_REVISION,
            body_sha256: Digest256::sha256(&canonical_bytes(&blueprint)).to_hex(),
            provenance: ComponentProvenance::SourcePackage {
                package_id: "pkg-a".into(),
                package_version: "1.0.0".into(),
            },
        };
        validate_prompt_blueprint(&blueprint, &identity, &PromptLimits::default())
            .expect("valid blueprint")
    }

    fn vars(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    // spec: MEDULLA-TYPED-PACKS-R001.8
    #[test]
    fn prompt_render_vectors() {
        let prompt = validated(serde_json::json!({
            "task": "summarize",
            "schema_revision": 1,
            "variables": ["audience", "topic"],
            "x-ext": {"b": 1, "a": [true, null]},
        }));
        let policy = RenderPolicy::default();
        let inputs = vars(&[("topic", "graphs".into()), ("audience", "ops".into())]);

        let first = render_prompt(&prompt, &inputs, &policy).expect("renders");
        let second = render_prompt(&prompt, &inputs, &policy).expect("renders");
        assert_eq!(first, second, "equal inputs give equal bytes");
        assert_eq!(first.render_digest, Digest256::sha256(&first.bytes));

        let changed = vars(&[("topic", "trees".into()), ("audience", "ops".into())]);
        let other = render_prompt(&prompt, &changed, &policy).expect("renders");
        assert_ne!(first.render_digest, other.render_digest);

        assert_eq!(
            render_prompt(&prompt, &vars(&[("topic", "graphs".into())]), &policy),
            Err(RefusalCode::PromptVariableMissing)
        );
        let mut extra = inputs.clone();
        extra.insert("undeclared".into(), Value::from(1));
        assert_eq!(
            render_prompt(&prompt, &extra, &policy),
            Err(RefusalCode::PromptVariableUnknown)
        );
        assert_eq!(
            render_prompt(
                &prompt,
                &inputs,
                &RenderPolicy {
                    max_render_bytes: 16,
                    ..policy
                }
            ),
            Err(RefusalCode::PromptOutputTooLarge)
        );
        assert_eq!(
            render_prompt(
                &prompt,
                &inputs,
                &RenderPolicy {
                    rendering_version: 2,
                    ..policy
                }
            ),
            Err(RefusalCode::PromptPinUnresolved)
        );

        let mut unpinned_identity = prompt.identity().clone();
        unpinned_identity.body_sha256 = "0".repeat(64);
        let unpinned = validate_prompt_blueprint(
            prompt.blueprint(),
            &unpinned_identity,
            &PromptLimits::default(),
        )
        .expect("valid blueprint");
        assert_eq!(
            render_prompt(&unpinned, &inputs, &policy),
            Err(RefusalCode::PromptPinUnresolved)
        );
        assert!(MAX_RENDER_BYTES_RANGE.contains(&DEFAULT_MAX_RENDER_BYTES));
        assert_eq!(
            RefusalCode::PromptPinUnresolved.as_str(),
            "PROMPT_PIN_UNRESOLVED"
        );
    }
}
