//! Graph intake model (MEDULLA-DECISION-ENGINE-R163.1).
//!
//! An intake source is either a file with its digest or a reference to a graph
//! query result. The graph reference names the authority, the query, the
//! revision read and the digest of the content returned.

use serde::{Deserialize, Serialize};

use medulla_types::contract::Digest256;

/// A graph a source was read from, and its content type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GraphSource {
    pub graph_ref: GraphReference,
    pub content_type: String,
}

/// A pinned reference into a graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GraphReference {
    pub authority: String,
    pub query: String,
    pub revision: u64,
    pub content_digest: Digest256,
}

/// One intake source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum IntakeSource {
    Filesystem {
        path: String,
        content_digest: Digest256,
    },
    Graph {
        graph_ref: GraphReference,
    },
}

/// One intake source after resolution (MEDULLA-DECISION-ENGINE-R163.2).
///
/// It keeps where the content came from (authority, query, exact revision) and
/// the digest of what was read, never the content body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ResolvedSource {
    /// A file, passed through with the digest its caller supplied.
    Filesystem {
        path: String,
        content_digest: Digest256,
    },
    /// A graph node resolved to its exact revision and content digest.
    Graph {
        authority: String,
        query: String,
        revision: u64,
        content_digest: Digest256,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference() -> GraphReference {
        GraphReference {
            authority: "graph:main".into(),
            query: "MATCH (n) RETURN n".into(),
            revision: 12,
            content_digest: Digest256::sha256(b"rows"),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R163.1
    #[test]
    fn intake_types_round_trip() {
        let sources = vec![
            IntakeSource::Filesystem {
                path: "docs/a.md".into(),
                content_digest: Digest256::sha256(b"a"),
            },
            IntakeSource::Graph {
                graph_ref: reference(),
            },
        ];
        for source in &sources {
            let text = serde_json::to_string(source).unwrap();
            assert_eq!(
                &serde_json::from_str::<IntakeSource>(&text).unwrap(),
                source
            );
        }
        assert!(serde_json::to_string(&sources[1])
            .unwrap()
            .contains("\"kind\":\"graph\""));
        let graph = GraphSource {
            graph_ref: reference(),
            content_type: "application/json".into(),
        };
        let text = serde_json::to_string(&graph).unwrap();
        assert_eq!(serde_json::from_str::<GraphSource>(&text).unwrap(), graph);
        assert!(serde_json::from_str::<IntakeSource>(r#"{"kind":"ftp","path":"x"}"#).is_err());
        let bounded: Result<medulla_types::contract::BoundedVec<IntakeSource, 64>, String> =
            medulla_types::contract::BoundedVec::new(vec![sources[0].clone(); 65]);
        assert!(bounded.is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R163.2
    #[test]
    fn resolved_source_keeps_authority_not_body() {
        let graph = ResolvedSource::Graph {
            authority: "graph:main".into(),
            query: "paper:1".into(),
            revision: 12,
            content_digest: Digest256::sha256(b"body"),
        };
        let file = ResolvedSource::Filesystem {
            path: "docs/a.md".into(),
            content_digest: Digest256::sha256(b"a"),
        };
        let value = serde_json::to_value(&graph).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["authority", "content_digest", "kind", "query", "revision"],
            "authority, revision and digest are kept; no field can carry a body"
        );
        let value = serde_json::to_value(&file).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["content_digest", "kind", "path"]);
        for source in [&graph, &file] {
            let text = serde_json::to_string(source).unwrap();
            assert_eq!(
                &serde_json::from_str::<ResolvedSource>(&text).unwrap(),
                source
            );
            assert!(!text.contains("body"));
        }
        // A body field is refused rather than carried.
        assert!(serde_json::from_str::<ResolvedSource>(
            r#"{"kind":"filesystem","path":"x","content_digest":"00","body":"text"}"#
        )
        .is_err());
    }
}
