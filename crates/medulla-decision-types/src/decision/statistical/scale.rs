//! Replica-count scale intents and their verdict (MEDULLA-DECISION-ENGINE-R833).
//!
//! An intent asks to move one scale unit to `desired_replicas`. The verdict is
//! `allow` or `refuse` with exactly one [`ScaleRefusalCode`]. The engine, never
//! the caller, supplies the [`ScaleObservation`]: the stored `Scale*` authority
//! nodes of the graph the question names. The checks run in the order the codes
//! are declared, so an intent with several faults reports the first.

use serde::{Deserialize, Serialize};

use medulla_types::refusal_code::RefusalCode;

/// Text parameter: the graph whose `Scale*` nodes hold the authority.
pub const PARAM_GRAPH: &str = "graph";
/// JSON text parameter: the [`ScaleIntentRecord`].
pub const PARAM_INTENT: &str = "intent";
/// The one option a scale intent question declares.
pub const VERDICT_OPTION: &str = "verdict";
/// The answer that allows the intent.
pub const ALLOW: &str = "allow";
/// Prefix of the answer that refuses it, followed by the code.
pub const REFUSE_PREFIX: &str = "refuse:";

/// The authority fence an intent carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScaleFenceRecord {
    pub lease_id: String,
    pub lease_epoch: u64,
    pub fence_token: u64,
    #[serde(default)]
    pub expires_at: String,
}

/// The complete target identity of an intent. `workload_id` names the scale unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScaleTargetRecord {
    pub cluster_id: String,
    pub runtime_id: String,
    pub workload_id: String,
    pub target_uid: String,
    pub target_resource_version: String,
}

/// One desired replica change, as the scale reconciler records it. Fields the
/// verdict does not read (reason, retry policy, creation time) are tolerated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScaleIntentRecord {
    pub intent_id: String,
    pub intent_revision: u64,
    pub expected_unit_revision: u64,
    pub desired_replicas: u64,
    pub min_replicas: u64,
    pub max_replicas: u64,
    pub controller_mode: String,
    pub replica_writer_id: String,
    pub execution_key: String,
    pub fence: ScaleFenceRecord,
    pub target: ScaleTargetRecord,
}

/// The stored scale unit the intent targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaleUnitFacts {
    pub revision: u64,
    pub min_replicas: u64,
    pub max_replicas: u64,
    pub controller_mode: String,
    pub replica_writer_id: String,
}

/// The stored resource pool of the unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalePoolFacts {
    pub capacity_units: u64,
    pub reserved_headroom_units: u64,
}

/// What the stored `Scale*` nodes report for the intent's unit now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaleObservation {
    /// How many `ScaleAuthorityRevision` nodes the graph holds; exactly one is a single authority.
    pub authority_revisions: usize,
    pub unit: Option<ScaleUnitFacts>,
    pub pool: Option<ScalePoolFacts>,
    /// The status of the unit's failure domain (`healthy`, `degraded`, `offline`).
    pub failure_domain_status: Option<String>,
    /// The `(lease_epoch, fence_token)` of the unit's controller registration.
    pub registration_fence: Option<(u64, u64)>,
}

/// Why an intent is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleRefusalCode {
    AuthorityMissing,
    UnitUnknown,
    UnitRevisionStale,
    BoundsInvalid,
    DesiredOutOfBounds,
    WriterMismatch,
    ControllerUnregistered,
    FenceMismatch,
    FailureDomainUnhealthy,
    PoolHeadroomExceeded,
}

impl ScaleRefusalCode {
    /// Every code, in the order the checks run.
    pub const ALL: [Self; 10] = [
        Self::AuthorityMissing,
        Self::UnitUnknown,
        Self::UnitRevisionStale,
        Self::BoundsInvalid,
        Self::DesiredOutOfBounds,
        Self::WriterMismatch,
        Self::ControllerUnregistered,
        Self::FenceMismatch,
        Self::FailureDomainUnhealthy,
        Self::PoolHeadroomExceeded,
    ];

    /// The registered refusal code.
    pub const fn code(self) -> RefusalCode {
        RefusalCode::new(match self {
            Self::AuthorityMissing => "SCALE_AUTHORITY_MISSING",
            Self::UnitUnknown => "SCALE_UNIT_UNKNOWN",
            Self::UnitRevisionStale => "SCALE_UNIT_REVISION_STALE",
            Self::BoundsInvalid => "SCALE_BOUNDS_INVALID",
            Self::DesiredOutOfBounds => "SCALE_DESIRED_OUT_OF_BOUNDS",
            Self::WriterMismatch => "SCALE_WRITER_MISMATCH",
            Self::ControllerUnregistered => "SCALE_CONTROLLER_UNREGISTERED",
            Self::FenceMismatch => "SCALE_FENCE_MISMATCH",
            Self::FailureDomainUnhealthy => "SCALE_FAILURE_DOMAIN_UNHEALTHY",
            Self::PoolHeadroomExceeded => "SCALE_POOL_HEADROOM_EXCEEDED",
        })
    }
}

/// The family's refusal table, registered in `code_registry::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[
    ScaleRefusalCode::AuthorityMissing.code(),
    ScaleRefusalCode::UnitUnknown.code(),
    ScaleRefusalCode::UnitRevisionStale.code(),
    ScaleRefusalCode::BoundsInvalid.code(),
    ScaleRefusalCode::DesiredOutOfBounds.code(),
    ScaleRefusalCode::WriterMismatch.code(),
    ScaleRefusalCode::ControllerUnregistered.code(),
    ScaleRefusalCode::FenceMismatch.code(),
    ScaleRefusalCode::FailureDomainUnhealthy.code(),
    ScaleRefusalCode::PoolHeadroomExceeded.code(),
];

/// The verdict on one intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleVerdict {
    Allow,
    Refuse(ScaleRefusalCode),
}

impl ScaleVerdict {
    /// The option id the verdict is answered as.
    pub fn option_id(self) -> String {
        match self {
            Self::Allow => ALLOW.to_string(),
            Self::Refuse(code) => format!("{REFUSE_PREFIX}{}", code.code().as_str()),
        }
    }
}

/// Judge `intent` against what the stored authority nodes report. The first
/// failing check names the refusal. One replica occupies one pool unit, so the
/// pool admits `desired_replicas` only up to its capacity less its reserved
/// headroom; what other units hold of the pool is the reservation ledger's
/// concern, not this verdict's.
pub fn judge(intent: &ScaleIntentRecord, observation: &ScaleObservation) -> ScaleVerdict {
    use ScaleRefusalCode as Code;
    use ScaleVerdict::{Allow, Refuse};
    if observation.authority_revisions != 1 {
        return Refuse(Code::AuthorityMissing);
    }
    let Some(unit) = &observation.unit else {
        return Refuse(Code::UnitUnknown);
    };
    if intent.expected_unit_revision != unit.revision {
        return Refuse(Code::UnitRevisionStale);
    }
    if intent.min_replicas > intent.max_replicas
        || intent.min_replicas < unit.min_replicas
        || intent.max_replicas > unit.max_replicas
    {
        return Refuse(Code::BoundsInvalid);
    }
    if intent.desired_replicas < intent.min_replicas
        || intent.desired_replicas > intent.max_replicas
    {
        return Refuse(Code::DesiredOutOfBounds);
    }
    if intent.controller_mode != unit.controller_mode
        || intent.replica_writer_id != unit.replica_writer_id
    {
        return Refuse(Code::WriterMismatch);
    }
    let Some((epoch, token)) = observation.registration_fence else {
        return Refuse(Code::ControllerUnregistered);
    };
    if intent.fence.lease_epoch != epoch || intent.fence.fence_token != token {
        return Refuse(Code::FenceMismatch);
    }
    if observation.failure_domain_status.as_deref() != Some("healthy") {
        return Refuse(Code::FailureDomainUnhealthy);
    }
    let admits = observation.pool.as_ref().is_some_and(|pool| {
        intent.desired_replicas
            <= pool
                .capacity_units
                .saturating_sub(pool.reserved_headroom_units)
    });
    if !admits {
        return Refuse(Code::PoolHeadroomExceeded);
    }
    Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent() -> ScaleIntentRecord {
        ScaleIntentRecord {
            intent_id: "i1".into(),
            intent_revision: 1,
            expected_unit_revision: 3,
            desired_replicas: 4,
            min_replicas: 1,
            max_replicas: 8,
            controller_mode: "native".into(),
            replica_writer_id: "w1".into(),
            execution_key: "x1".into(),
            fence: ScaleFenceRecord {
                lease_id: "l1".into(),
                lease_epoch: 2,
                fence_token: 9,
                expires_at: String::new(),
            },
            target: ScaleTargetRecord {
                cluster_id: "c".into(),
                runtime_id: "r".into(),
                workload_id: "u1".into(),
                target_uid: "uid".into(),
                target_resource_version: "7".into(),
            },
        }
    }

    fn observation() -> ScaleObservation {
        ScaleObservation {
            authority_revisions: 1,
            unit: Some(ScaleUnitFacts {
                revision: 3,
                min_replicas: 0,
                max_replicas: 10,
                controller_mode: "native".into(),
                replica_writer_id: "w1".into(),
            }),
            pool: Some(ScalePoolFacts {
                capacity_units: 10,
                reserved_headroom_units: 2,
            }),
            failure_domain_status: Some("healthy".into()),
            registration_fence: Some((2, 9)),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R833
    #[test]
    fn a_consistent_intent_is_allowed() {
        assert_eq!(judge(&intent(), &observation()), ScaleVerdict::Allow);
        assert_eq!(ScaleVerdict::Allow.option_id(), "allow");
    }

    // spec: MEDULLA-DECISION-ENGINE-R833
    #[test]
    fn each_fault_refuses_with_its_own_code_in_check_order() {
        type Mutation = Box<dyn Fn(&mut ScaleIntentRecord, &mut ScaleObservation)>;
        let cases: Vec<(ScaleRefusalCode, Mutation)> = vec![
            (
                ScaleRefusalCode::AuthorityMissing,
                Box::new(|_, o| o.authority_revisions = 2),
            ),
            (
                ScaleRefusalCode::UnitUnknown,
                Box::new(|_, o| o.unit = None),
            ),
            (
                ScaleRefusalCode::UnitRevisionStale,
                Box::new(|i, _| i.expected_unit_revision = 2),
            ),
            (
                ScaleRefusalCode::BoundsInvalid,
                Box::new(|i, _| i.max_replicas = 11),
            ),
            (
                ScaleRefusalCode::DesiredOutOfBounds,
                Box::new(|i, _| i.desired_replicas = 9),
            ),
            (
                ScaleRefusalCode::WriterMismatch,
                Box::new(|i, _| i.replica_writer_id = "w2".into()),
            ),
            (
                ScaleRefusalCode::ControllerUnregistered,
                Box::new(|_, o| o.registration_fence = None),
            ),
            (
                ScaleRefusalCode::FenceMismatch,
                Box::new(|i, _| i.fence.fence_token = 8),
            ),
            (
                ScaleRefusalCode::FailureDomainUnhealthy,
                Box::new(|_, o| o.failure_domain_status = Some("degraded".into())),
            ),
            (
                ScaleRefusalCode::PoolHeadroomExceeded,
                Box::new(|i, _| {
                    i.max_replicas = 10;
                    i.desired_replicas = 9;
                }),
            ),
        ];
        assert_eq!(cases.len(), ScaleRefusalCode::ALL.len());
        for ((code, fault), expected) in cases.iter().zip(ScaleRefusalCode::ALL) {
            assert_eq!(*code, expected, "cases follow the check order");
            let (mut ask, mut seen) = (intent(), observation());
            fault(&mut ask, &mut seen);
            assert_eq!(judge(&ask, &seen), ScaleVerdict::Refuse(*code), "{code:?}");
        }
        assert_eq!(REFUSALS.len(), ScaleRefusalCode::ALL.len());
    }
}
