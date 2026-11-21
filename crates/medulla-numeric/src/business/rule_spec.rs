//! Rule records (MEDULLA-BUSINESS-OPS-R002.1).
//!
//! A `:RuleSpec` is `{kind, version, parameters, canonical digest}`, immutable by id and
//! version. [`load`] is the one entry that turns a stored record into a [`RuleSpec`]: a
//! version the engine does not know refuses with [`RULE_VERSION_UNKNOWN`] and nothing is
//! evaluated. The digest is taken over the kind, the version and the parameters sorted
//! by name, so the same record always has the same digest.

use std::collections::BTreeMap;

use medulla_types::contract::Digest256;

/// The refusal code for a rule version this engine does not know.
pub const RULE_VERSION_UNKNOWN: &str = "RULE_VERSION_UNKNOWN";

/// The refusal code for a rule kind this engine does not know.
pub const RULE_KIND_UNKNOWN: &str = "RULE_KIND_UNKNOWN";

/// The rule versions this engine evaluates.
pub const SUPPORTED_VERSIONS: &[u32] = &[1];

/// The domain of the canonical rule digest.
const RULE_DIGEST_DOMAIN: &[u8] = b"medulla.business.rule_spec";

/// The five rule kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuleKind {
    /// Next due dates of a recurrence rule.
    Recurrence,
    /// Applicability of a threshold expression.
    Threshold,
    /// Matching of orders, payments and payouts.
    Reconcile,
    /// Reorder point and quantity.
    Reorder,
    /// Claim-text policy check.
    PhrasePolicy,
}

impl RuleKind {
    /// Every kind, in registry order.
    pub const ALL: [RuleKind; 5] = [
        RuleKind::Recurrence,
        RuleKind::Threshold,
        RuleKind::Reconcile,
        RuleKind::Reorder,
        RuleKind::PhrasePolicy,
    ];

    /// The stored name of the kind.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recurrence => "recurrence",
            Self::Threshold => "threshold",
            Self::Reconcile => "reconcile",
            Self::Reorder => "reorder",
            Self::PhrasePolicy => "phrase_policy",
        }
    }

    /// The kind with the stored name `name`, if the engine knows one.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }
}

/// A refusal with one stable code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleRefusal {
    /// The stable code, such as [`RULE_VERSION_UNKNOWN`].
    pub code: &'static str,
    /// What was refused.
    pub detail: String,
}

impl RuleRefusal {
    /// A refusal with `code` and `detail`.
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for RuleRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

/// One loaded rule record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleSpec {
    /// The rule kind.
    pub kind: RuleKind,
    /// The rule version.
    pub version: u32,
    /// The rule parameters, sorted by name.
    pub parameters: BTreeMap<String, String>,
    /// The canonical digest of kind, version and parameters, as 64 lowercase hex characters.
    pub digest: String,
}

/// The canonical digest of one rule record.
pub fn canonical_digest(
    kind: RuleKind,
    version: u32,
    parameters: &BTreeMap<String, String>,
) -> String {
    let version_bytes = version.to_be_bytes();
    let mut fields: Vec<&[u8]> = vec![kind.as_str().as_bytes(), version_bytes.as_slice()];
    for (name, value) in parameters {
        fields.push(name.as_bytes());
        fields.push(value.as_bytes());
    }
    Digest256::framed(RULE_DIGEST_DOMAIN, &fields)
        .expect("a rule record always fits digest framing")
        .to_hex()
}

/// Load a rule record. A version the engine does not know refuses with
/// [`RULE_VERSION_UNKNOWN`]; a kind it does not know refuses with [`RULE_KIND_UNKNOWN`].
/// Nothing is evaluated on a refusal.
pub fn load(
    kind: &str,
    version: u32,
    parameters: BTreeMap<String, String>,
) -> Result<RuleSpec, RuleRefusal> {
    let kind = RuleKind::parse(kind)
        .ok_or_else(|| RuleRefusal::new(RULE_KIND_UNKNOWN, format!("no rule kind `{kind}`")))?;
    if !SUPPORTED_VERSIONS.contains(&version) {
        return Err(RuleRefusal::new(
            RULE_VERSION_UNKNOWN,
            format!("rule kind `{}` has no version {version}", kind.as_str()),
        ));
    }
    let digest = canonical_digest(kind, version, &parameters);
    Ok(RuleSpec {
        kind,
        version,
        parameters,
        digest,
    })
}

/// One input node of an evaluation, with the revision that was read.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct InputRef {
    /// The node id.
    pub node_id: String,
    /// The revision of the node that was read.
    pub revision: u64,
}

impl InputRef {
    /// An input node reference.
    pub fn new(node_id: impl Into<String>, revision: u64) -> Self {
        Self {
            node_id: node_id.into(),
            revision,
        }
    }
}

/// What every rule result carries: the rule digest, the input nodes with their revisions
/// and the steps taken.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuleTrace {
    /// The digest of the rule that produced the result; empty until a rule is bound.
    pub rule_digest: String,
    /// The input nodes with their revisions, sorted.
    pub inputs: Vec<InputRef>,
    /// The steps taken, in order.
    pub steps: Vec<String>,
}

impl RuleTrace {
    /// A trace for the rule `spec` over `inputs`; the inputs are sorted so the same set of
    /// nodes always gives the same trace.
    pub fn new(spec: &RuleSpec, mut inputs: Vec<InputRef>) -> Self {
        inputs.sort();
        inputs.dedup();
        Self {
            rule_digest: spec.digest.clone(),
            inputs,
            steps: Vec::new(),
        }
    }

    /// Record one step.
    pub fn step(&mut self, text: impl Into<String>) {
        self.steps.push(text.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parameters(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    // spec: MEDULLA-BUSINESS-OPS-R002.1
    #[test]
    fn unknown_version_refuses_and_digest_is_stable() {
        // An unknown version refuses with the stable code and loads nothing.
        let refused = load("recurrence", 2, parameters(&[("frequency", "monthly")])).unwrap_err();
        assert_eq!(refused.code, RULE_VERSION_UNKNOWN);
        let refused = load("recurrence", 0, BTreeMap::new()).unwrap_err();
        assert_eq!(refused.code, RULE_VERSION_UNKNOWN);
        let refused = load("unknown_kind", 1, BTreeMap::new()).unwrap_err();
        assert_eq!(refused.code, RULE_KIND_UNKNOWN);

        // The digest is stable: the same record always has the same digest, whatever the
        // order the parameters were given in.
        let one = load(
            "recurrence",
            1,
            parameters(&[("frequency", "monthly"), ("interval", "1")]),
        )
        .unwrap();
        let two = load(
            "recurrence",
            1,
            parameters(&[("interval", "1"), ("frequency", "monthly")]),
        )
        .unwrap();
        assert_eq!(one.digest, two.digest);
        assert_eq!(one.digest.len(), 64);
        assert!(one
            .digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        assert_eq!(one.digest, canonical_digest(one.kind, 1, &one.parameters));

        // Any change of kind, version-bound parameters or value changes the digest.
        let other_value = load(
            "recurrence",
            1,
            parameters(&[("frequency", "weekly"), ("interval", "1")]),
        )
        .unwrap();
        let other_kind = load(
            "threshold",
            1,
            parameters(&[("frequency", "monthly"), ("interval", "1")]),
        )
        .unwrap();
        assert_ne!(one.digest, other_value.digest);
        assert_ne!(one.digest, other_kind.digest);
        // Field framing: moving a byte between a name and a value changes the digest.
        let moved = load(
            "recurrence",
            1,
            parameters(&[("frequencymonthly", ""), ("interval", "1")]),
        )
        .unwrap();
        assert_ne!(one.digest, moved.digest);

        // Every kind round-trips through its stored name.
        for kind in RuleKind::ALL {
            assert_eq!(RuleKind::parse(kind.as_str()), Some(kind));
        }

        // A trace carries the digest and the sorted, de-duplicated inputs.
        let mut trace = RuleTrace::new(
            &one,
            vec![
                InputRef::new("node:b", 2),
                InputRef::new("node:a", 1),
                InputRef::new("node:a", 1),
            ],
        );
        trace.step("loaded");
        assert_eq!(trace.rule_digest, one.digest);
        assert_eq!(
            trace.inputs,
            vec![InputRef::new("node:a", 1), InputRef::new("node:b", 2)]
        );
        assert_eq!(trace.steps, vec!["loaded".to_string()]);
    }
}
