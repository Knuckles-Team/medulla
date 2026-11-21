//! The plan scope's allow set at every scan and vector leaf
//! (MEDULLA-FEDERATED-QUERY-R078.6).
//!
//! A plan that carries a `Plan.scope` reads a row only when the scope's closed predicate
//! set admits it: the row's `tenant` property, when it has one, equals the scope tenant,
//! and every clearance label the row requires is among the scope's labels. The check runs
//! at the leaf itself, on top of graph isolation, never as a post-filter of a later
//! stage. A scope that is not verified admits no row (fail closed).

use medulla_query_types::wire::PlanScopeLeaf;
use medulla_types::contract::ServerErrorCode;

use super::PlanCtx;
use crate::rowset::RowSet;

/// The node property naming the tenant a row belongs to.
const TENANT_PROPERTY: &str = "tenant";

/// Whether the plan scope admits node `id` at a leaf of class `leaf`. A plan with no
/// scope, or a scope that does not cover `leaf`, admits every row; an unverified scope
/// admits none.
pub(crate) fn allows(ctx: &PlanCtx, leaf: PlanScopeLeaf, id: &str) -> bool {
    let Some(scope) = &ctx.plan_scope else {
        return true;
    };
    if !scope.leaves.contains(&leaf) {
        return true;
    }
    if !scope.verified {
        return false;
    }
    let Some(props) = ctx.view.node_row_object(id) else {
        return true;
    };
    if let Some(tenant) = props.get(TENANT_PROPERTY).and_then(serde_json::Value::as_str) {
        if tenant != scope.tenant {
            return false;
        }
    }
    super::company_context::node_clearance(&props)
        .iter()
        .all(|label| scope.clearance_labels.contains(label))
}

/// Narrow the rows a scan leaf of class `leaf` produced to the scope's allow set. An
/// unverified scope refuses the leaf before any row is returned.
pub(super) fn scope_leaf(
    ctx: &PlanCtx,
    leaf: PlanScopeLeaf,
    rows: RowSet,
) -> Result<RowSet, String> {
    let Some(scope) = &ctx.plan_scope else {
        return Ok(rows);
    };
    if scope.leaves.contains(&leaf) && !scope.verified {
        return Err(format!(
            "{}: the plan scope tenant is not verified",
            ServerErrorCode::PlanScopeUnverified
        ));
    }
    let keep: std::collections::HashSet<&str> = rows
        .rows()
        .iter()
        .map(|row| row.id.as_str())
        .filter(|id| allows(ctx, leaf, id))
        .collect();
    if keep.len() == rows.len() {
        return Ok(rows);
    }
    Ok(rows.intersect_keep_order(&keep))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_graph_core::graph::GraphCore;
    use medulla_query_types::wire::PlanScope;
    use medulla_semantic_index::semantic::SemanticStore;

    fn blob(value: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&value).expect("encodes")
    }

    fn scope(verified: bool) -> PlanScope {
        PlanScope {
            tenant: "tenant-a".into(),
            clearance_labels: vec!["internal".into()],
            verified,
            leaves: vec![PlanScopeLeaf::Graph, PlanScopeLeaf::Vector],
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R078.6
    #[test]
    fn scope_narrows_rows_within_one_tenant() {
        let core = GraphCore::new();
        core.add_node("open".into(), blob(serde_json::json!({ "type": "Doc" })));
        core.add_node(
            "mine".into(),
            blob(serde_json::json!({ "type": "Doc", "tenant": "tenant-a", "clearance": "internal" })),
        );
        core.add_node(
            "secret".into(),
            blob(serde_json::json!({ "type": "Doc", "clearance": ["internal", "finance"] })),
        );
        core.add_node(
            "foreign".into(),
            blob(serde_json::json!({ "type": "Doc", "tenant": "tenant-b" })),
        );
        let view = core.analysis_snapshot();
        let semantic = SemanticStore::new();
        let ctx = PlanCtx::new(&view, &semantic).with_plan_scope(scope(true));
        let all = || RowSet::from_ids(["open", "mine", "secret", "foreign"].map(String::from));

        let graph = scope_leaf(&ctx, PlanScopeLeaf::Graph, all()).expect("scoped scan");
        assert_eq!(graph.ids(), vec!["open", "mine"]);
        // The edge leaf is not stamped on this scope, so it is not narrowed.
        let edge = scope_leaf(&ctx, PlanScopeLeaf::Edge, all()).expect("edge leaf");
        assert_eq!(edge.len(), 4);
        // The vector leaf's candidate set is the same allow set.
        let input = all();
        let eligible = super::super::vector_rank::eligible_ids(&ctx, &input);
        let mut ids: Vec<&str> = eligible.into_iter().collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["mine", "open"]);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R078.6
    #[test]
    fn an_unverified_scope_fails_closed_at_every_leaf() {
        let core = GraphCore::new();
        core.add_node("open".into(), blob(serde_json::json!({ "type": "Doc" })));
        let view = core.analysis_snapshot();
        let semantic = SemanticStore::new();
        let ctx = PlanCtx::new(&view, &semantic).with_plan_scope(scope(false));
        let rows = RowSet::from_ids(["open".to_string()]);
        let error = scope_leaf(&ctx, PlanScopeLeaf::Graph, rows.clone()).unwrap_err();
        assert!(error.starts_with("PLAN_SCOPE_UNVERIFIED"), "{error}");
        assert!(super::super::vector_rank::eligible_ids(&ctx, &rows).is_empty());
    }
}
