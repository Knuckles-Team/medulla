//! The source kind a semantic index is built from, and the two identities that carry it: the
//! source revision (`sql-source:<digest>:epoch:<n>` or `graph-source:<digest>:epoch:<n>`) and the
//! source entity id (`semantic-sql-source:<digest>` or `semantic-graph-source:<digest>`).
//!
//! Both identities are kind tagged and every parser takes both kinds, so a graph source never
//! masquerades as SQL and a numerically newer revision of one kind never supersedes the other.

use serde::{Deserialize, Serialize};

use super::digest::SemanticDigest;
use super::state::SemanticIndexError;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SemanticSourceKind {
    #[default]
    Sql,
    Graph,
}

impl SemanticSourceKind {
    pub const ALL: [Self; 2] = [Self::Sql, Self::Graph];

    /// The prefix of a source revision of this kind.
    pub fn revision_tag(self) -> &'static str {
        match self {
            Self::Sql => "sql-source",
            Self::Graph => "graph-source",
        }
    }

    /// The prefix of a source entity id of this kind.
    pub fn entity_tag(self) -> &'static str {
        match self {
            Self::Sql => "semantic-sql-source",
            Self::Graph => "semantic-graph-source",
        }
    }

    /// The source entity id of this kind for one identity digest.
    pub fn entity_id(self, digest: SemanticDigest) -> String {
        format!("{}:{digest}", self.entity_tag())
    }

    /// Parse a source entity id of either kind.
    pub fn parse_entity_id(entity: &str) -> Option<(Self, SemanticDigest)> {
        Self::ALL.into_iter().find_map(|kind| {
            let digest = entity.strip_prefix(kind.entity_tag())?.strip_prefix(':')?;
            Some((kind, SemanticDigest::parse(digest).ok()?))
        })
    }

    /// The canonical tenant-wide source revision of this kind for one authority and epoch.
    pub fn revision_for_epoch(
        self,
        authority_digest: SemanticDigest,
        source_epoch: u64,
    ) -> Result<String, SemanticIndexError> {
        if source_epoch == 0 {
            return Err(SemanticIndexError::InvalidField {
                field: "source_epoch".to_string(),
                reason: "source epoch must be nonzero".to_string(),
            });
        }
        Ok(format!(
            "{}:{authority_digest}:epoch:{source_epoch}",
            self.revision_tag()
        ))
    }
}

/// A parsed canonical source revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticSourceRevision<'a> {
    pub kind: SemanticSourceKind,
    /// The source lineage: the kind tag and the authority digest, `<tag>:sha256:<hex>`. Two
    /// revisions order by epoch only when this is equal, so it carries the kind.
    pub authority: &'a str,
    pub epoch: u64,
}

impl<'a> SemanticSourceRevision<'a> {
    /// Parse a canonical source revision of either kind. The lineage is parsed before the epoch
    /// so a numerically newer foreign source cannot supersede the tenant's current lineage.
    pub fn parse(revision: &'a str) -> Option<Self> {
        let (authority, epoch) = revision.rsplit_once(":epoch:")?;
        let (kind, digest) = SemanticSourceKind::ALL.into_iter().find_map(|kind| {
            let digest = authority
                .strip_prefix(kind.revision_tag())?
                .strip_prefix(':')?;
            Some((kind, digest))
        })?;
        SemanticDigest::parse(digest).ok()?;
        let epoch: u64 = epoch.parse().ok()?;
        if epoch == 0 || epoch.to_string() != epoch_text(revision) {
            return None;
        }
        Some(Self {
            kind,
            authority,
            epoch,
        })
    }
}

fn epoch_text(revision: &str) -> &str {
    revision
        .rsplit_once(":epoch:")
        .map_or("", |(_, epoch)| epoch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> SemanticDigest {
        SemanticDigest::from_bytes([byte; 32])
    }

    #[test]
    fn revisions_round_trip_for_both_kinds() {
        for kind in SemanticSourceKind::ALL {
            let revision = kind.revision_for_epoch(digest(7), 3).unwrap();
            assert!(revision.starts_with(&format!("{}:sha256:", kind.revision_tag())));
            let parsed = SemanticSourceRevision::parse(&revision).unwrap();
            assert_eq!((parsed.kind, parsed.epoch), (kind, 3));
            assert_eq!(
                parsed.authority,
                format!("{}:{}", kind.revision_tag(), digest(7))
            );
        }
    }

    #[test]
    fn the_two_kinds_never_share_a_lineage() {
        let sql = SemanticSourceKind::Sql
            .revision_for_epoch(digest(1), 9)
            .unwrap();
        let graph = SemanticSourceKind::Graph
            .revision_for_epoch(digest(1), 9)
            .unwrap();
        let sql = SemanticSourceRevision::parse(&sql).unwrap();
        let graph = SemanticSourceRevision::parse(&graph).unwrap();
        assert_ne!(sql.authority, graph.authority);
        assert_ne!(sql.kind, graph.kind);
    }

    #[test]
    fn malformed_revisions_are_refused() {
        let good = SemanticSourceKind::Graph
            .revision_for_epoch(digest(2), 1)
            .unwrap();
        assert!(SemanticSourceRevision::parse(&good).is_some());
        let base = format!("{}", digest(2));
        for invalid in [
            String::new(),
            "graph-source".to_string(),
            format!("graph-source:{base}"),
            format!("graph-source:{base}:epoch:0"),
            format!("graph-source:{base}:epoch:01"),
            format!("graph-source:{base}:epoch:+1"),
            format!("graph-source:{base}:epoch:x"),
            format!("graph-source:{}:epoch:1", base.to_uppercase()),
            "graph-source:sha256:abc:epoch:1".to_string(),
            format!("other-source:{base}:epoch:1"),
            format!("{base}:epoch:1"),
        ] {
            assert!(
                SemanticSourceRevision::parse(&invalid).is_none(),
                "{invalid}"
            );
        }
        assert!(SemanticSourceKind::Graph
            .revision_for_epoch(digest(2), 0)
            .is_err());
    }

    #[test]
    fn entity_ids_parse_with_their_kind() {
        for kind in SemanticSourceKind::ALL {
            let id = kind.entity_id(digest(5));
            assert_eq!(
                SemanticSourceKind::parse_entity_id(&id),
                Some((kind, digest(5)))
            );
        }
        for invalid in [
            "semantic-source:sha256:00",
            "semantic-graph-source:nope",
            "semantic-sql-sourcesha256:00",
        ] {
            assert!(
                SemanticSourceKind::parse_entity_id(invalid).is_none(),
                "{invalid}"
            );
        }
    }
}
