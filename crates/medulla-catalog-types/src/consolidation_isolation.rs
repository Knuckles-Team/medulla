//! Typed model for cross-application consolidation safety
//! (MEDULLA-UNIFIED-DATA-PLANE-R031): the quota and upgrade-coupling policy that
//! bound one attached application's blast radius, and the admission record
//! that scopes it to a tenant and principal before it is exposed through
//! any approved mapping. This is the typed-model slice (`.1`): the model
//! plus the refusal of an admission with no isolation identity (empty
//! tenant/application/principal), a quota with every bound at zero (it
//! blocks the application entirely rather than isolating it), and a second
//! admission that reuses the same tenant/application under a different
//! principal (an isolation-identity collision). The real fault-isolation
//! enforcement and quota metering are later children.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The bounds that cap one attached application's blast radius.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedApplicationQuota {
    pub max_requests_per_minute: u32,
    pub max_concurrent_connections: u32,
    pub max_storage_bytes: u64,
}

/// The bounds one registered foreign source may spend per query
/// (MEDULLA-UNIFIED-DATA-PLANE-R031.2). Bounds are checked by [`SourceQuota::validate`]; a
/// request narrows a quota with [`SourceQuota::narrowed`] and never widens it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SourceQuota {
    /// Requests in flight against the source at once.
    pub max_concurrent: u32,
    /// Rows the source may return to one query.
    pub max_rows: u64,
    /// Wall-clock milliseconds one request to the source may take.
    pub max_wall_ms: u64,
}

impl SourceQuota {
    pub const CONCURRENT_BOUNDS: (u32, u32) = (1, 256);
    pub const ROWS_BOUNDS: (u64, u64) = (1, 10_000_000);
    pub const WALL_MS_BOUNDS: (u64, u64) = (100, 600_000);

    /// Refuse a quota with a dimension outside its bounds; the message names the dimension.
    pub fn validate(&self) -> Result<(), String> {
        fn check<T: PartialOrd + std::fmt::Display + Copy>(
            name: &str,
            value: T,
            (lo, hi): (T, T),
        ) -> Result<(), String> {
            if value < lo || value > hi {
                return Err(format!("{name} {value} is outside [{lo}, {hi}]"));
            }
            Ok(())
        }
        check(
            "max_concurrent",
            self.max_concurrent,
            Self::CONCURRENT_BOUNDS,
        )?;
        check("max_rows", self.max_rows, Self::ROWS_BOUNDS)?;
        check("max_wall_ms", self.max_wall_ms, Self::WALL_MS_BOUNDS)
    }

    /// The quota a request runs under: each dimension is the smaller of the source's and the
    /// request's, so a request narrows a quota and never widens it.
    pub fn narrowed(&self, rows: u64, wall_ms: u64) -> Self {
        Self {
            max_concurrent: self.max_concurrent,
            max_rows: self.max_rows.min(rows),
            max_wall_ms: self.max_wall_ms.min(wall_ms),
        }
    }
}

/// How a later upgrade to this application's integration is allowed to
/// couple with other attached applications.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeCouplingPolicy {
    /// May upgrade on its own schedule.
    Independent,
    /// Must upgrade together with a named dependency set.
    LockStep,
    /// Requires a recorded, separate go-ahead before upgrading.
    Manual,
}

/// One attached application's admission: the tenant/principal it is scoped
/// to, its blast-radius quota, and its upgrade-coupling policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationAdmission {
    pub application: String,
    pub tenant: String,
    pub principal: String,
    pub quota: AttachedApplicationQuota,
    pub upgrade_coupling: UpgradeCouplingPolicy,
}

/// An admission record failed validation. Carries the offending
/// application name and the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidApplicationAdmission {
    pub application: String,
    pub reason: String,
}

impl std::fmt::Display for InvalidApplicationAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid admission for application {:?}: {}",
            self.application, self.reason
        )
    }
}

impl std::error::Error for InvalidApplicationAdmission {}

/// Confirm an admission record is well-formed: application, tenant, and
/// principal are all non-empty (isolation needs an explicit identity to
/// isolate), and the quota is not entirely zero (a fully zeroed quota
/// blocks the application rather than bounding its blast radius, and is
/// never a meaningful admission).
pub fn validate_admission(
    admission: &ApplicationAdmission,
) -> Result<(), InvalidApplicationAdmission> {
    if admission.application.is_empty()
        || admission.tenant.is_empty()
        || admission.principal.is_empty()
    {
        return Err(InvalidApplicationAdmission {
            application: admission.application.clone(),
            reason: "application, tenant, and principal must all be non-empty".to_string(),
        });
    }
    let quota = admission.quota;
    if quota.max_requests_per_minute == 0
        && quota.max_concurrent_connections == 0
        && quota.max_storage_bytes == 0
    {
        return Err(InvalidApplicationAdmission {
            application: admission.application.clone(),
            reason: "quota with every bound at zero cannot bound a blast radius".to_string(),
        });
    }
    Ok(())
}

/// The registry of admitted applications, keyed by tenant and application
/// name. `admit` is the one entry point a caller uses instead of mutating
/// `admissions` directly.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsolidationIsolationRegistry {
    admissions: BTreeMap<(String, String), ApplicationAdmission>,
}

impl ConsolidationIsolationRegistry {
    /// Admit an application under its tenant, refusing an invalid record
    /// and refusing to re-admit the same tenant/application pair under a
    /// different principal (an isolation-identity collision — the existing
    /// principal must be the one that re-admits or revokes first).
    pub fn admit(
        &mut self,
        admission: ApplicationAdmission,
    ) -> Result<(), InvalidApplicationAdmission> {
        validate_admission(&admission)?;
        let key = (admission.tenant.clone(), admission.application.clone());
        if let Some(existing) = self.admissions.get(&key) {
            if existing.principal != admission.principal {
                return Err(InvalidApplicationAdmission {
                    application: admission.application.clone(),
                    reason: format!(
                        "already admitted under principal {:?}; refusing re-admission under {:?}",
                        existing.principal, admission.principal
                    ),
                });
            }
        }
        self.admissions.insert(key, admission);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.admissions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.admissions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admission(tenant: &str, application: &str, principal: &str) -> ApplicationAdmission {
        ApplicationAdmission {
            application: application.to_string(),
            tenant: tenant.to_string(),
            principal: principal.to_string(),
            quota: AttachedApplicationQuota {
                max_requests_per_minute: 600,
                max_concurrent_connections: 10,
                max_storage_bytes: 1_000_000,
            },
            upgrade_coupling: UpgradeCouplingPolicy::Independent,
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R031.2
    #[test]
    fn source_quota_is_bounded_and_only_narrows() {
        let quota = SourceQuota {
            max_concurrent: 8,
            max_rows: 250_000,
            max_wall_ms: 60_000,
        };
        assert!(quota.validate().is_ok());
        for bad in [
            SourceQuota {
                max_concurrent: 0,
                ..quota
            },
            SourceQuota {
                max_concurrent: 257,
                ..quota
            },
            SourceQuota {
                max_rows: 0,
                ..quota
            },
            SourceQuota {
                max_rows: 10_000_001,
                ..quota
            },
            SourceQuota {
                max_wall_ms: 99,
                ..quota
            },
            SourceQuota {
                max_wall_ms: 600_001,
                ..quota
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        let narrowed = quota.narrowed(1_000, 5_000_000);
        assert_eq!((narrowed.max_rows, narrowed.max_wall_ms), (1_000, 60_000));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R031.1
    #[test]
    fn valid_admission_round_trips_through_admit() {
        let mut registry = ConsolidationIsolationRegistry::default();
        registry
            .admit(admission("tenant-a", "gramps", "alice"))
            .unwrap();
        assert_eq!(registry.len(), 1);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R031.1
    #[test]
    fn empty_identity_is_refused() {
        let mut bad = admission("tenant-a", "gramps", "alice");
        bad.principal = String::new();
        assert!(validate_admission(&bad).is_err());
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R031.1
    #[test]
    fn all_zero_quota_is_refused() {
        let mut bad = admission("tenant-a", "gramps", "alice");
        bad.quota = AttachedApplicationQuota {
            max_requests_per_minute: 0,
            max_concurrent_connections: 0,
            max_storage_bytes: 0,
        };
        let Err(error) = validate_admission(&bad) else {
            panic!("an all-zero quota must be refused");
        };
        assert!(error.reason.contains("blast radius"), "{error}");
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R031.1
    #[test]
    fn same_principal_re_admission_is_idempotent() {
        let mut registry = ConsolidationIsolationRegistry::default();
        registry
            .admit(admission("tenant-a", "gramps", "alice"))
            .unwrap();
        registry
            .admit(admission("tenant-a", "gramps", "alice"))
            .unwrap();
        assert_eq!(registry.len(), 1);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R031.1
    #[test]
    fn different_principal_re_admission_is_refused() {
        let mut registry = ConsolidationIsolationRegistry::default();
        registry
            .admit(admission("tenant-a", "gramps", "alice"))
            .unwrap();
        let result = registry.admit(admission("tenant-a", "gramps", "mallory"));
        assert!(result.is_err());
        assert_eq!(registry.len(), 1);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R031.1
    #[test]
    fn invalid_admission_is_refused_not_silently_admitted() {
        let mut registry = ConsolidationIsolationRegistry::default();
        let result = registry.admit(admission("tenant-a", "", "alice"));
        assert!(result.is_err());
        assert!(registry.is_empty());
    }
}
