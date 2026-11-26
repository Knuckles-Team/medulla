//! The served completion barrier (MEDULLA-DURABLE-KERNEL-R878): one answer to "has this
//! asynchronous work settled", so a caller waits on the engine instead of sleeping and polling.
//!
//! A `resource` names the work: `graph:<name>` is the committed-change watermark of a graph (it
//! rises with every durable change the graph records) and `catalog:<source>` is the catalog
//! probe of one source the caller registered (0 until the probe has settled, 1 after).

use serde::{Deserialize, Serialize};

/// The resource prefix of a graph's committed-change watermark.
pub const WATERMARK_GRAPH_PREFIX: &str = "graph:";
/// The resource prefix of a registered source's catalog probe.
pub const WATERMARK_CATALOG_PREFIX: &str = "catalog:";

/// A resource a watermark can be awaited on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatermarkResource {
    /// The committed-change watermark of the named graph.
    Graph(String),
    /// The catalog probe of the named source of the verified caller.
    Catalog(String),
}

impl WatermarkResource {
    /// Parse a resource name, or `None` when it names neither kind or names nothing.
    pub fn parse(resource: &str) -> Option<Self> {
        let named = |prefix: &str| {
            resource
                .strip_prefix(prefix)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        };
        named(WATERMARK_GRAPH_PREFIX)
            .map(Self::Graph)
            .or_else(|| named(WATERMARK_CATALOG_PREFIX).map(Self::Catalog))
    }
}

/// The answer of `Method::AwaitWatermark`: the watermark the resource held when the wait ended
/// and whether it reached the requested one before the deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WatermarkAwaited {
    /// The watermark when the wait ended.
    pub watermark: u64,
    /// Whether `watermark` is at least the requested one.
    pub reached: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DURABLE-KERNEL-R878
    #[test]
    fn a_resource_names_a_graph_or_a_source_and_nothing_else() {
        assert_eq!(
            WatermarkResource::parse("graph:pack"),
            Some(WatermarkResource::Graph("pack".to_string()))
        );
        assert_eq!(
            WatermarkResource::parse("catalog:gramps"),
            Some(WatermarkResource::Catalog("gramps".to_string()))
        );
        assert_eq!(WatermarkResource::parse("graph:"), None);
        assert_eq!(WatermarkResource::parse("queue:x"), None);
    }
}
