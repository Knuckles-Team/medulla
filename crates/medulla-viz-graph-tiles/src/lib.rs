//! `medulla-viz-graph-tiles` — the binary tile/streaming protocol for graph
//! payloads (VIZ-2 of the million-node-graph-visualization program).
//!
//! See [`contract`] for the shared `clusters(...)`/`expand(...)` types and
//! the [`contract::GraphSource`] trait boundary VIZ-1's real GraphCore-backed
//! clustering plugs into; [`wire`] for the binary encoding + chunk-streaming
//! frame envelope that is this crate's actual deliverable; [`demo`] for a
//! deterministic, seeded, in-memory `GraphSource` proving the whole path end
//! to end before VIZ-1 lands.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them

pub mod contract;
pub mod demo;
pub mod wire;

pub use contract::{
    ChildClusterRef, ClusterExpansion, ClusterLevel, ClusterSummary, GraphSource, InterClusterEdge,
    TileEdge, TileNode,
};
pub use demo::{DemoGraph, DemoParams};
pub use wire::{
    decode_cluster_expansion, decode_cluster_level, encode_cluster_expansion, encode_cluster_level,
    read_frames, write_frame, write_stream_end, StreamFrame, TileKind, WireError,
};
