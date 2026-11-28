#[cfg(feature = "cost")]
use super::request_boundary::dispatch_resource_stats;
use super::request_boundary::{
    append_native_capacity_ops, append_native_resource_ops, append_native_work_item_ops,
    dispatch_boxed,
};
use crate::dispatch::*;
use medulla_server_api::handlers::{DispatchCtx, DispatchHeader};
use medulla_server_graph::dispatch::change_envelope::dispatch_change_envelopes;
use medulla_server_graph::dispatch::graph_pipeline::dispatch_graph_op;
use medulla_server_graph::dispatch::request_boundary::screen::decode_screen_observation;
#[cfg(feature = "ast")]
use medulla_server_graph::{
    dispatch::request_boundary::preflight::ast_input_limits,
    dispatch::request_boundary::preflight::decode_ast_files,
    dispatch::request_boundary::preflight::validate_ast_logical_path,
};

#[cfg(all(feature = "ann-redb", feature = "query"))]
pub mod bookmark;
#[cfg(all(feature = "ann-redb", feature = "query"))]
pub mod bookmark_binding;
pub mod branch_merge;
pub mod branch_write;
pub mod catalog_emit;
#[cfg(all(feature = "redb", feature = "blob", feature = "ast"))]
pub mod changeset_index;
pub mod channels;
pub mod data_plane_arms;
pub mod git_export_pack;
pub mod git_refs_list;
#[cfg(all(feature = "ann-redb", feature = "query"))]
pub mod graph_source;
pub mod import_git_history;
pub mod incident;
pub mod knowledge_bundle;
pub mod lifecycle;
pub mod list_path_grants;
pub mod memory_change;
pub mod memory_events;
pub mod quality;
pub mod read_blame;
pub mod read_path_history;
pub mod read_tree;
pub mod resource_cost;
pub mod revert_changeset;
pub mod schema_drift;
pub mod sdlc;
pub mod service_control;
pub mod set_path_grant;
#[cfg(feature = "security")]
pub mod share;
pub mod source_bisect;
pub mod source_ingest;
pub mod submit_changeset;
pub mod submit_symbol_edit;
pub mod telemetry;

#[cfg(feature = "security")]
pub use lifecycle::apply_rbac_admin;
#[cfg(feature = "redb")]
pub use lifecycle::mark_graph_ready;
pub use lifecycle::{create_graph, GraphLifecycleRequest};
pub use lifecycle::{delete_graph, dispatch_get_identity, index_kind_label, index_validity_label};
