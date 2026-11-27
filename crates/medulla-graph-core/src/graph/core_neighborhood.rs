use super::*;

use crate::neighborhood::{NeighborLink, NeighborhoodSource};

/// The graph core is the adjacency the bounded neighborhood walk reads
/// (MEDULLA-DURABLE-KERNEL-R876.2). Every call takes one topology read guard; the walk
/// holds none between calls.
impl NeighborhoodSource for GraphCore {
    fn has_node(&self, id: &str) -> bool {
        self.topo.read().node_map.contains_key(id)
    }

    fn links(&self, id: &str) -> Vec<NeighborLink> {
        let topo = self.topo.read();
        let Some(&idx) = topo.node_map.get(id) else {
            return Vec::new();
        };
        let mut links: std::collections::BTreeMap<(String, bool), ()> = Default::default();
        for (direction, outgoing) in [
            (petgraph::Direction::Outgoing, true),
            (petgraph::Direction::Incoming, false),
        ] {
            for edge in topo.graph.edges_directed(idx, direction) {
                let far = if outgoing {
                    edge.target()
                } else {
                    edge.source()
                };
                links.insert((topo.graph[far].clone(), outgoing), ());
            }
        }
        links
            .into_keys()
            .map(|(neighbor, outgoing)| {
                let key = if outgoing {
                    (id.to_string(), neighbor.clone())
                } else {
                    (neighbor.clone(), id.to_string())
                };
                let types = self
                    .edge_properties
                    .get(&key)
                    .map(|blobs| {
                        crate::index::EdgeRels::from_blobs(blobs.iter().map(|b| b.as_slice())).types
                    })
                    .unwrap_or_default();
                NeighborLink {
                    neighbor,
                    outgoing,
                    types,
                }
            })
            .collect()
    }

    fn degree(&self, id: &str) -> usize {
        let topo = self.topo.read();
        topo.node_map.get(id).map_or(0, |idx| {
            topo.graph
                .edges_directed(*idx, petgraph::Direction::Outgoing)
                .count()
                + topo
                    .graph
                    .edges_directed(*idx, petgraph::Direction::Incoming)
                    .count()
        })
    }
}
