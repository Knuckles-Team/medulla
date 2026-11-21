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
    pub repo: String,
    pub principal: String,
    pub prefix: String,
    pub read: bool,
    pub write: bool,
}

/// What a request does to a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathAccess {
    Read,
    Write,
}

/// Why a path grant check or write refused; each variant serves one stable code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathGrantRefusal {
    ReadDenied,
    WriteDenied,
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
    pub repo: String,
    pub principal: String,
    pub prefix: String,
    pub read: bool,
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
}
