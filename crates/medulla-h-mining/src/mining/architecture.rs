//! The served `ArchitectureView` read (MEDULLA-CQ-ORCH-R010.3).
//!
//! The view is derived from the caller's projected graph, so another tenant's
//! repository is unknown to the caller. The revision the caller states is
//! compared with the repository snapshot digest computed from the same
//! projected read view, so the returned view is consistent with the digest it
//! was compared with. A refusal returns its `VIEW_*` code and no partial view.

use std::sync::Arc;

use medulla_core::architecture_view::{
    derive_architecture_view_with, repository_revision, ViewRules,
};
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_protocol_types::result_contract::graph::ArchitectureView;
use medulla_server_api::knob_config::knobs;
use {
    medulla_catalog_source_types::architecture_view::check_revision,
    medulla_catalog_source_types::architecture_view::ArchitectureViewResponse,
    medulla_catalog_source_types::architecture_view::ViewScope,
};

/// Answer one `ArchitectureView` read.
pub(super) fn handle_architecture_view(
    req_id: u64,
    core: &Arc<GraphCore>,
    read_authority: Option<&medulla_server_api::carrier::GraphReadAuthority>,
    scope: &ViewScope,
) -> Response {
    let visible = match read_authority {
        Some(authority) => authority.project_core(core),
        None => core.clone(),
    };
    let revision = repository_revision(&visible, &scope.repository_id);
    if let Err(stale) = check_revision(scope, revision.as_deref()) {
        return Response::err(req_id, stale.to_string());
    }
    let rules = ViewRules {
        max_containers: knobs().architecture_view_max_containers as usize,
        ..ViewRules::default()
    };
    match derive_architecture_view_with(&visible, scope, &rules) {
        Ok(view) => Response::ok(
            req_id,
            ResultPayload::of::<ArchitectureView>(ArchitectureViewResponse::new(view, revision)),
        ),
        Err(abstained) => Response::err(req_id, abstained.to_string()),
    }
}
