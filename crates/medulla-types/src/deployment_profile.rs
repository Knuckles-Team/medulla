//! The deployment profile of the engine (MEDULLA-DURABLE-KERNEL-R038.100): `embedded`
//! (in-process library) or `served` (the default: a server process). The profile
//! changes transport and startup only; it never changes the durability class of a
//! namespace or the acknowledgment authority of redb.

use std::fmt;

/// The configured value of `deployment.profile`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DeploymentProfile {
    /// The in-process library.
    Embedded,
    /// A server process. The default.
    #[default]
    Served,
}

impl DeploymentProfile {
    /// The configuration value of this profile.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Embedded => "embedded",
            Self::Served => "served",
        }
    }

    /// Parse a configured value; any other value is refused, never defaulted.
    pub fn parse_declared(name: &str) -> Result<Self, UnknownDeploymentProfile> {
        match name {
            "embedded" => Ok(Self::Embedded),
            "served" => Ok(Self::Served),
            other => Err(UnknownDeploymentProfile(other.to_string())),
        }
    }
}

/// A `deployment.profile` value that is neither `embedded` nor `served`. Displays as
/// the registered `DEPLOYMENT_PROFILE_UNKNOWN` refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownDeploymentProfile(String);

impl fmt::Display for UnknownDeploymentProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: deployment.profile {:?} is not one of embedded, served",
            crate::contract::ServerErrorCode::DeploymentProfileUnknown.as_str(),
            self.0
        )
    }
}

impl std::error::Error for UnknownDeploymentProfile {}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DURABLE-KERNEL-R038.100
    #[test]
    fn profile_parses_its_two_values_and_refuses_the_rest() {
        assert_eq!(DeploymentProfile::default(), DeploymentProfile::Served);
        for profile in [DeploymentProfile::Embedded, DeploymentProfile::Served] {
            assert_eq!(
                DeploymentProfile::parse_declared(profile.as_str()),
                Ok(profile)
            );
        }
        for bad in ["Embedded", "", "cluster", " served"] {
            let refused = DeploymentProfile::parse_declared(bad).unwrap_err();
            assert!(refused
                .to_string()
                .starts_with("DEPLOYMENT_PROFILE_UNKNOWN: "));
        }
    }
}
