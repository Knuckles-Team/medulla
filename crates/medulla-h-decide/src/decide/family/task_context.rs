//! Run-context assembly for a `task_plan` question (MEDULLA-SDLC-R015.2).
//!
//! A `TaskPlan` question that names a requirement row (`task_context_row`) and its
//! repository (`task_context_repo`) gets the context an agent run needs: the row text, the
//! plan sections that cover it, the contracts that name it, the data model and the code
//! neighbourhood. The graph is read behind the ordinary read ACL, so a node the caller
//! cannot see is absent and never counted. The digest of the context rides in the answer's
//! premises as `task_context.digest`; the answer wire does not change.

use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, TypedParam, TypedValue,
};
use medulla_decision_types::decision::task_plan::context::{ContextSection, TaskContext};
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance, PremiseRef};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_types::contract::BoundedVec;
use std::collections::BTreeSet;

use crate::decide::stat_support::invalid_parameter;
use crate::decide::SharedState;

/// The text param holding the row id.
pub const ROW_PARAM: &str = "task_context_row";
/// The text param holding the repository.
pub const REPO_PARAM: &str = "task_context_repo";
/// The text param naming the graph that holds the records. It is required: a question
/// that names a row without it is refused, and no graph is assumed.
pub const GRAPH_PARAM: &str = "graph";
/// The text param the engine fills with the digest of the assembled context.
pub const DIGEST_PARAM: &str = "task_context_digest";
/// The text param the engine fills with the assembled context itself (canonical JSON). It
/// is sealed into the plan the answer carries (MEDULLA-SDLC-R015.3).
pub const BODY_PARAM: &str = "task_context_body";

const PLAN_MISSING: &str = "TASK_CONTEXT_PLAN_MISSING";
const ROW_UNKNOWN: &str = "TASK_CONTEXT_ROW_UNKNOWN";
const MAX_NODES: usize = 1_000_000;

/// Cut `text` to at most `room` bytes at a UTF-8 boundary.
fn cut(text: &str, room: usize) -> &str {
    let mut end = room.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Add `section` within the remaining `room`; a section that passes it is cut and the rest
/// is dropped (`truncated`).
fn fit(section: ContextSection, room: &mut usize, truncated: &mut bool) -> Option<ContextSection> {
    if *truncated {
        return None;
    }
    if section.text.len() > *room {
        let text = cut(&section.text, *room).to_string();
        *truncated = true;
        *room = 0;
        return Some(ContextSection { text, ..section });
    }
    *room -= section.text.len();
    Some(section)
}

type Props = serde_json::Map<String, serde_json::Value>;

fn text_of(props: &Props, key: &str) -> String {
    props
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Assemble the run context of `row_id` in `repo`.
pub(in crate::decide) async fn assemble_task_context(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    repo: &str,
    row_id: &str,
    max_bytes: usize,
) -> Result<TaskContext, String> {
    let core = crate::decide::topology_read::graph_core(state, graph, verified).await?;
    let authority = {
        let guard = state.read().await;
        GraphReadAuthority::from_verified(verified, &guard.isolation)?
    };
    let (repo, row_id) = (repo.to_string(), row_id.to_string());
    tokio::task::spawn_blocking(move || {
        read_task_context(
            &core,
            &|blob, is_schema| authority.can_see_node(blob, is_schema),
            &repo,
            &row_id,
            max_bytes,
        )
    })
    .await
    .map_err(|error| format!("task context read task failed: {error}"))?
}

/// Read the run context of `row_id` in `repo` from `core`. `can_see` is the read ACL of the
/// caller over a node's property blob and whether the node is a schema node: a node it
/// refuses is absent and never counted.
pub fn read_task_context(
    core: &medulla_graph_core::graph::GraphCore,
    can_see: &dyn Fn(&[u8], bool) -> bool,
    repo: &str,
    row_id: &str,
    max_bytes: usize,
) -> Result<TaskContext, String> {
    let (repo, row_id) = (repo.to_string(), row_id.to_string());
    let visible = |id: &str| -> Option<Props> {
        let blob = core.get_node_properties(id)?;
        if !can_see(&blob, core.is_schema_node(id)) {
            return None;
        }
        medulla_types::msgpack::decode_property_object(&blob).ok()
    };
    let requirement = format!("requirement:{repo}:{row_id}");
    let row = visible(&requirement)
        .ok_or_else(|| format!("{ROW_UNKNOWN}: {row_id} is no requirement of {repo}"))?;
    let relation = |source: &str, target: &str, name: &str| {
        core.typed_edges_between(source, target)
            .iter()
            .any(|(relation, _)| relation == name)
    };
    let line_of = |props: &Props| {
        text_of(props, "start_line")
            .parse::<u64>()
            .unwrap_or(u64::MAX)
    };
    let section_of = |id: String, props: &Props| ContextSection {
        title: text_of(props, "title"),
        text: text_of(props, "text"),
        node_id: id,
    };
    // The plan sections that cover the row, in start-line order.
    let mut plans: Vec<(u64, String, Props)> = core
        .get_predecessors(&requirement)
        .unwrap_or_default()
        .into_iter()
        .filter(|id| relation(id, &requirement, "covers"))
        .filter_map(|id| Some((id.clone(), visible(&id)?)))
        .filter(|(_, props)| props.get("type").and_then(|t| t.as_str()) == Some("PlanSection"))
        .map(|(id, props)| (line_of(&props), id, props))
        .collect();
    if plans.is_empty() {
        return Err(format!(
            "{PLAN_MISSING}: no plan section covers {row_id} in {repo}"
        ));
    }
    plans.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    // The contract sections that name the row and the data model section.
    let mut contracts: Vec<(String, String, Props)> = Vec::new();
    let mut data_model: Option<(u64, String, Props)> = None;
    for (id, blob) in core.get_nodes_by_label("SpecSection", MAX_NODES) {
        if !can_see(&blob, core.is_schema_node(&id)) {
            continue;
        }
        let Ok(props) = medulla_types::msgpack::decode_property_object(&blob) else {
            continue;
        };
        if text_of(&props, "repo") != repo {
            continue;
        }
        let file = text_of(&props, "file");
        if (file.contains("/contracts/") || file.starts_with("contracts/"))
            && text_of(&props, "text").contains(row_id.as_str())
        {
            contracts.push((file.clone(), id.clone(), props.clone()));
        }
        if text_of(&props, "title") == "data-model" || file.ends_with("data-model.md") {
            let key = line_of(&props);
            if data_model
                .as_ref()
                .is_none_or(|(held, held_id, _)| (key, &id) < (*held, held_id))
            {
                data_model = Some((key, id.clone(), props.clone()));
            }
        }
    }
    contracts.sort_by(|a, b| {
        (&a.0, line_of(&a.2), &a.1).cmp(&(&b.0, line_of(&b.2), &b.1))
    });
    // The code neighbourhood: the symbols the row is implemented by and one hop around.
    let mut neighbourhood: BTreeSet<String> = BTreeSet::new();
    for symbol in core.get_successors(&requirement).unwrap_or_default() {
        if !relation(&requirement, &symbol, "implementedBy") || visible(&symbol).is_none() {
            continue;
        }
        neighbourhood.insert(symbol.clone());
        for near in core.get_neighbors(&symbol).unwrap_or_default() {
            if !near.starts_with("requirement:") && visible(&near).is_some() {
                neighbourhood.insert(near);
            }
        }
    }
    let mut room = max_bytes;
    let mut truncated = false;
    let row_text = {
        let text = text_of(&row, "text");
        if text.len() > room {
            truncated = true;
            let kept = cut(&text, room).to_string();
            room = 0;
            kept
        } else {
            room -= text.len();
            text
        }
    };
    let plan_sections = plans
        .into_iter()
        .filter_map(|(_, id, props)| fit(section_of(id, &props), &mut room, &mut truncated))
        .collect();
    let contracts = contracts
        .into_iter()
        .filter_map(|(_, id, props)| fit(section_of(id, &props), &mut room, &mut truncated))
        .collect();
    let data_model =
        data_model.and_then(|(_, id, props)| fit(section_of(id, &props), &mut room, &mut truncated));
    Ok(TaskContext {
        row_id,
        row_text,
        plan_sections,
        contracts,
        data_model,
        neighbourhood: neighbourhood.into_iter().collect(),
        truncated,
    })
}

fn text_param<'a>(request: &'a DecideRequest, name: &str) -> Option<&'a str> {
    request
        .params
        .iter()
        .find(|param| param.name == name)
        .and_then(|param| match &param.value {
            TypedValue::Text(text) if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        })
}

/// Read the run context of a `TaskPlan` question that names a row and records its digest in
/// the `task_context_digest` param. A question without the row param is returned as it came;
/// a caller-supplied digest is refused, since the digest is the engine's reading.
pub(in crate::decide) async fn with_task_context(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    mut request: DecideRequest,
) -> Result<DecideRequest, String> {
    if request.question.kind != QuestionKind::TaskPlan {
        return Ok(request);
    }
    if text_param(&request, DIGEST_PARAM).is_some() || text_param(&request, BODY_PARAM).is_some() {
        return Err(invalid_parameter(
            "the task context digest and body are filled by the engine and cannot be supplied",
        ));
    }
    let Some(row) = text_param(&request, ROW_PARAM).map(str::to_string) else {
        return Ok(request);
    };
    let repo = text_param(&request, REPO_PARAM)
        .map(str::to_string)
        .ok_or_else(|| {
            invalid_parameter(format!("{ROW_PARAM} needs the text param {REPO_PARAM}"))
        })?;
    let max_bytes = usize::try_from(medulla_server_api::knob_config::knobs().sdlc_task_context_max_bytes)
        .unwrap_or(usize::MAX);
    let graph = text_param(&request, GRAPH_PARAM)
        .map(str::to_string)
        .ok_or_else(|| {
            invalid_parameter(format!("{ROW_PARAM} needs the text param {GRAPH_PARAM}"))
        })?;
    let context =
        assemble_task_context(state, &graph, verified, &repo, &row, max_bytes).await?;
    let mut params = request.params.as_slice().to_vec();
    params.push(TypedParam {
        name: DIGEST_PARAM.to_string(),
        value: TypedValue::Text(context.digest()),
    });
    params.push(TypedParam {
        name: BODY_PARAM.to_string(),
        value: TypedValue::Text(
            serde_json::to_string(&context)
                .map_err(|error| format!("task context body did not encode: {error}"))?,
        ),
    });
    request.params = BoundedVec::new(params).map_err(invalid_parameter)?;
    Ok(request)
}

/// The context body the engine recorded on a question, read back for sealing into the plan.
pub(in crate::decide) fn body_of(request: &DecideRequest) -> Result<Option<TaskContext>, String> {
    text_param(request, BODY_PARAM)
        .map(|body| {
            serde_json::from_str(body)
                .map_err(|error| format!("task context body did not decode: {error}"))
        })
        .transpose()
}

/// The premise a recorded context digest gives: one observation, `task_context.digest`.
pub(super) fn premises(request: &DecideRequest) -> Vec<PremiseRef> {
    let (Some(digest), Some(row)) = (
        text_param(request, DIGEST_PARAM),
        text_param(request, ROW_PARAM),
    ) else {
        return Vec::new();
    };
    vec![PremiseRef {
        subject: row.to_string(),
        fact: format!("task_context.digest={digest}"),
        class: PremiseClass::Observation,
        provenance: PremiseProvenance::Request {
            field: format!("params.{DIGEST_PARAM}"),
        },
        trust_tier: None,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(id: &str, text: &str) -> ContextSection {
        ContextSection {
            node_id: id.into(),
            title: id.into(),
            text: text.into(),
        }
    }

    // spec: MEDULLA-SDLC-R015.2
    #[test]
    fn sections_are_cut_at_a_utf8_boundary_and_the_rest_dropped() {
        let (mut room, mut truncated) = (5usize, false);
        let first = fit(section("a", "ab"), &mut room, &mut truncated).expect("fits");
        assert_eq!((first.text.as_str(), room, truncated), ("ab", 3, false));
        // "é" is two bytes: the cut at three bytes falls inside the second one.
        let second = fit(section("b", "éé"), &mut room, &mut truncated).expect("cut");
        assert_eq!(second.text, "é");
        assert!(truncated);
        assert_eq!(room, 0);
        assert!(fit(section("c", "z"), &mut room, &mut truncated).is_none());
    }

    // spec: MEDULLA-SDLC-R015.2
    #[test]
    fn the_digest_follows_the_context() {
        let context = |text: &str| TaskContext {
            row_id: "DEMO-R001".into(),
            row_text: text.into(),
            plan_sections: vec![section("p", "plan")],
            contracts: Vec::new(),
            data_model: None,
            neighbourhood: vec!["symbol:a".into()],
            truncated: false,
        };
        assert_eq!(context("x").digest(), context("x").digest());
        assert_ne!(context("x").digest(), context("y").digest());
        assert_eq!(context("x").digest().len(), 64);
    }
}
