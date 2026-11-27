//! The request-boundary check of path-scoped source access (MEDULLA-SCM-R007).
//!
//! The path-addressed source methods (`SubmitChangeset`, `ReadTree`,
//! `ReadPathHistory`, `ReadBlame`, `GitExportPack`) load the caller's grants
//! from the branch registry and call [`check`] before any blob is read. The
//! caller is `verified.agent_id()`, the same principal `check_graph_access`
//! decides on.

use medulla_source_types::source_path_grant::{
    in_prefix, resolve, PathAccess, PathGrant, PathGrantRefusal,
};

/// Whether `grants` allow `access` on every one of `paths`: each path is
/// decided by its longest matching grant; no match denies.
pub fn check(
    grants: &[PathGrant],
    paths: &[&str],
    access: PathAccess,
) -> Result<(), PathGrantRefusal> {
    for path in paths {
        if !allows(grants, path, access) {
            return Err(access.denied());
        }
    }
    Ok(())
}

/// Whether one path is allowed.
pub fn allows(grants: &[PathGrant], path: &str, access: PathAccess) -> bool {
    resolve(grants, path).is_some_and(|grant| grant.allows(access))
}

/// Whether a `ReadTree` of `prefix` may answer: the grant that governs
/// `prefix` itself allows `access`, or some grant that allows it lies at or
/// below `prefix`. Otherwise the whole request refuses; a request that may
/// answer still drops every entry its own grant denies.
pub fn any_allowed_at_or_below(grants: &[PathGrant], prefix: &str, access: PathAccess) -> bool {
    allows(grants, prefix, access)
        || grants
            .iter()
            .any(|grant| grant.allows(access) && in_prefix(&grant.prefix, prefix))
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
    fn check_refuses_on_the_first_denied_path() {
        let grants = vec![grant("src", true, true), grant("src/secret", false, false)];
        assert_eq!(
            check(&grants, &["src/a", "src/b"], PathAccess::Write),
            Ok(())
        );
        assert_eq!(
            check(&grants, &["src/a", "src/secret/x"], PathAccess::Read),
            Err(PathGrantRefusal::ReadDenied)
        );
        assert_eq!(
            check(&grants, &["docs/x"], PathAccess::Write),
            Err(PathGrantRefusal::WriteDenied)
        );
        assert!(any_allowed_at_or_below(&grants, "", PathAccess::Read));
        assert!(!any_allowed_at_or_below(&grants, "docs", PathAccess::Read));
        // The grant that governs the prefix itself counts: `src/a` reads under `src`.
        assert!(any_allowed_at_or_below(&grants, "src/a", PathAccess::Read));
        assert!(!any_allowed_at_or_below(
            &grants,
            "src/secret",
            PathAccess::Read
        ));
    }
}
