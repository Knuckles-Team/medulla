//! Path-scoped access grants of the source methods (MEDULLA-SCM-R007).
//!
//! A grant gives one principal read and/or write access to the paths of one
//! repository under a prefix. The longest prefix that matches a path on a
//! path-segment boundary decides; no match denies. Pure data and decisions: the
//! registry rows live in the branch registry and the request-boundary check is
//! `medulla_server_auth::access::path_grant::check`.

use serde::{Deserialize, Serialize};

/// One grant row: `principal` may read and/or write the paths of `repo` under
/// `prefix`. The empty prefix covers the whole repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PathGrant {
    /// The repository the grant covers.
    pub repo: String,
    /// The principal the grant is given to.
    pub principal: String,
    /// The path prefix the grant covers; empty covers the whole repository.
    pub prefix: String,
    /// Whether the principal may read paths under the prefix.
    pub read: bool,
    /// Whether the principal may write paths under the prefix.
    pub write: bool,
}

/// What a request does to a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathAccess {
    /// The request reads the path.
    Read,
    /// The request writes the path.
    Write,
}

/// Why a path grant check or write refused; each variant serves one stable code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathGrantRefusal {
    /// No grant lets the principal read the path (`PATH_READ_DENIED`).
    ReadDenied,
    /// No grant lets the principal write the path (`PATH_WRITE_DENIED`).
    WriteDenied,
    /// A `SetPathGrant` request broke a bound (`PATH_GRANT_INVALID`).
    Invalid,
}

impl PathGrantRefusal {
    /// The stable refusal code served on the wire.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ReadDenied => "PATH_READ_DENIED",
            Self::WriteDenied => "PATH_WRITE_DENIED",
            Self::Invalid => "PATH_GRANT_INVALID",
        }
    }
}

impl PathAccess {
    /// The refusal a denied path of this access answers with.
    pub fn denied(self) -> PathGrantRefusal {
        match self {
            Self::Read => PathGrantRefusal::ReadDenied,
            Self::Write => PathGrantRefusal::WriteDenied,
        }
    }
}

/// Whether `path` lies under `prefix` on a path-segment boundary. The empty
/// prefix holds every path.
pub fn in_prefix(path: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The grant that decides `path`: the longest `prefix` that matches on a
/// path-segment boundary. `None` means no grant matches, which denies.
pub fn resolve<'a>(grants: &'a [PathGrant], path: &str) -> Option<&'a PathGrant> {
    grants
        .iter()
        .filter(|grant| in_prefix(path, &grant.prefix))
        .max_by_key(|grant| grant.prefix.len())
}

impl PathGrant {
    /// Whether this grant allows `access`.
    pub fn allows(&self, access: PathAccess) -> bool {
        match access {
            PathAccess::Read => self.read,
            PathAccess::Write => self.write,
        }
    }
}

/// The closed request of `Method::SetPathGrant` (MEDULLA-SCM-R007).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SetPathGrantRequest {
    /// The repository the grant covers.
    pub repo: String,
    /// The principal the grant is given to.
    pub principal: String,
    /// The path prefix the grant covers; empty covers the whole repository.
    pub prefix: String,
    /// Whether the principal may read paths under the prefix.
    pub read: bool,
    /// Whether the principal may write paths under the prefix.
    pub write: bool,
}

impl SetPathGrantRequest {
    /// Check every bound; any failure is `PATH_GRANT_INVALID` with the field
    /// named. `repo` has the bounds of `ReadTreeRequest.repo`, `principal` is 1
    /// to 256 bytes and `prefix` is empty or a valid submitted path.
    pub fn validate(&self) -> Result<PathGrant, String> {
        let repo_ok = !self.repo.is_empty()
            && self.repo.chars().count() <= 128
            && self
                .repo
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
            && !self.repo.split('/').any(|segment| segment == "..");
        if !repo_ok {
            return Err("repo".to_string());
        }
        if self.principal.is_empty() || self.principal.len() > 256 {
            return Err("principal".to_string());
        }
        let prefix_ok = self.prefix.is_empty()
            || (self.prefix.len() <= 4096
                && !self.prefix.contains('\0')
                && !self
                    .prefix
                    .split('/')
                    .any(|segment| segment.is_empty() || segment == "." || segment == ".."));
        if !prefix_ok {
            return Err("prefix".to_string());
        }
        Ok(PathGrant {
            repo: self.repo.clone(),
            principal: self.principal.clone(),
            prefix: self.prefix.clone(),
            read: self.read,
            write: self.write,
        })
    }
}

/// The closed request of `Method::ListPathGrants` (MEDULLA-SCM-R007.1): the grants of one
/// principal in the verified tenant, optionally narrowed to one repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ListPathGrantsRequest {
    /// The principal whose grants are read; the verified caller's own grants need only a read
    /// scope, another principal's need `graph:admin`.
    pub principal: String,
    /// Only the grants on this repository; absent reads every repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

impl ListPathGrantsRequest {
    /// Check the bounds `SetPathGrantRequest` holds for the same fields; a failure is
    /// `PATH_GRANT_INVALID` with the field named.
    pub fn validate(&self) -> Result<(), String> {
        if self.principal.is_empty() || self.principal.len() > 256 {
            return Err("principal".to_string());
        }
        if let Some(repo) = &self.repo {
            let repo_ok = !repo.is_empty()
                && repo.chars().count() <= 128
                && repo
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
                && !repo.split('/').any(|segment| segment == "..");
            if !repo_ok {
                return Err("repo".to_string());
            }
        }
        Ok(())
    }
}

/// The served answer of `ListPathGrants`: the principal's grants ordered by repository, then
/// prefix. A principal with no grants, a principal the tenant never granted and a repository
/// with none all answer an empty list. A grant with both bits false is a stored explicit deny
/// and lists like any other row.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PathGrantList {
    /// The grants of the principal, ordered by repository, then prefix.
    pub grants: Vec<PathGrant>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(prefix: &str, read: bool, write: bool) -> PathGrant {
        PathGrant {
            repo: "r".into(),
            principal: "p".into(),
            prefix: prefix.into(),
            read,
            write,
        }
    }

    // spec: MEDULLA-SCM-R007
    #[test]
    fn longest_segment_prefix_wins_and_no_match_denies() {
        let grants = vec![
            grant("src", true, true),
            grant("src/secret", false, false),
            grant("docs/a", true, false),
        ];
        assert_eq!(resolve(&grants, "src/a.rs").unwrap().prefix, "src");
        assert_eq!(resolve(&grants, "src").unwrap().prefix, "src");
        assert_eq!(
            resolve(&grants, "src/secret/key").unwrap().prefix,
            "src/secret"
        );
        // A prefix matches on a segment boundary only.
        assert_eq!(resolve(&grants, "src/secretive").unwrap().prefix, "src");
        assert!(resolve(&grants, "srcx/a").is_none());
        assert!(resolve(&grants, "docs/x").is_none());
        // The empty prefix covers the whole repository but loses to a longer one.
        let mut with_root = grants.clone();
        with_root.push(grant("", true, false));
        assert_eq!(resolve(&with_root, "docs/x").unwrap().prefix, "");
        assert_eq!(resolve(&with_root, "src/a").unwrap().prefix, "src");
        assert!(!resolve(&grants, "src/secret/key")
            .unwrap()
            .allows(PathAccess::Read));
        assert_eq!(PathAccess::Write.denied().code(), "PATH_WRITE_DENIED");
        assert_eq!(PathAccess::Read.denied().code(), "PATH_READ_DENIED");
        assert_eq!(PathGrantRefusal::Invalid.code(), "PATH_GRANT_INVALID");
    }

    // spec: MEDULLA-SCM-R007
    #[test]
    fn a_set_grant_request_is_bounded() {
        let ok = SetPathGrantRequest {
            repo: "repo".into(),
            principal: "agent".into(),
            prefix: "src".into(),
            read: true,
            write: false,
        };
        assert!(ok.validate().is_ok());
        let mut bad = ok.clone();
        bad.prefix = "../x".into();
        assert_eq!(bad.validate(), Err("prefix".to_string()));
        bad = ok.clone();
        bad.principal = String::new();
        assert_eq!(bad.validate(), Err("principal".to_string()));
        bad = ok.clone();
        bad.repo = "a..b/../c".into();
        assert_eq!(bad.validate(), Err("repo".to_string()));
        bad = ok;
        bad.prefix = String::new();
        assert!(bad.validate().is_ok());
    }

    // spec: MEDULLA-SCM-R007.1
    #[test]
    fn list_request_bounds_match_the_set_request() {
        let ok = ListPathGrantsRequest {
            principal: "alice".into(),
            repo: Some("org/repo".into()),
        };
        assert_eq!(ok.validate(), Ok(()));
        let any_repo = ListPathGrantsRequest {
            principal: "alice".into(),
            repo: None,
        };
        assert_eq!(any_repo.validate(), Ok(()));
        let no_principal = ListPathGrantsRequest {
            principal: String::new(),
            repo: None,
        };
        assert_eq!(no_principal.validate(), Err("principal".to_string()));
        let bad_repo = ListPathGrantsRequest {
            principal: "alice".into(),
            repo: Some("a/../b".into()),
        };
        assert_eq!(bad_repo.validate(), Err("repo".to_string()));
        let wire: ListPathGrantsRequest = serde_json::from_str(r#"{"principal":"alice"}"#).unwrap();
        assert_eq!(wire, any_repo);
        assert!(serde_json::from_str::<ListPathGrantsRequest>(
            r#"{"principal":"alice","tenant":"t"}"#
        )
        .is_err());
        assert!(PathGrantList::default().grants.is_empty());
    }
}
