//! `Method::FrictionReport`: friction reports grouped by skill, and the explicit machine each
//! group moves along (MEDULLA-DECISION-ENGINE-R710.3 to R710.5).
//!
//! The handler is the single writer of friction state. A `Report` writes one `FrictionReport`
//! node, updates the skill's open group and links the report to its skill, its run trace and
//! its group, all in one batch; a recurring report also feeds failure-pattern mining because the
//! `FailureIngest` reader takes `FrictionReport` nodes as failure records (R701). A `Transition`
//! moves the group through `FrictionMachine`, names its evidence and appends one `FrictionStep`
//! node. The handler commits nothing itself: it answers a [`PlannedWrite`] the router commits
//! through the graph gateway. A refusal writes nothing.

use medulla_decision_program_types::friction::FrictionOp;
use medulla_server_api::authority_context::VerifiedRequestContext;

use medulla_h_decide_base::decide::planned_write::PlannedWrite;
use medulla_h_decide_base::decide::SharedState;

/// The scope a `Report` needs.
pub const SCOPE_REPORT: &str = "knowledge:flag";
/// The scope a `Transition` needs, except into `Approved`.
pub const SCOPE_TRANSITION: &str = "knowledge:curate";

/// Plan one `FrictionReport` operation for the verified caller.
pub async fn handle_friction_report(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    op: FrictionOp,
) -> Result<PlannedWrite, String> {
    #[cfg(all(feature = "decide", feature = "redb"))]
    {
        served::plan(state, verified, op).await
    }
    #[cfg(not(all(feature = "decide", feature = "redb")))]
    {
        let _ = (state, verified, op);
        Err("FrictionReport requires the `decide` and `redb` features".to_string())
    }
}

#[cfg(all(feature = "decide", feature = "redb"))]
mod served {
    use std::collections::BTreeSet;

    use medulla_decision_outcome_types::decision::revision_list::RevisionRow;
    use medulla_graph_core::graph::GraphView;
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    use medulla_persistence_records::decision_jobs::{decode_artifact, receipt_key, record_key};
    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::coordination::{
        FrictionReportFile, FrictionReportGet, FrictionReportGroup, FrictionReportMove,
    };
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_types::contract::Digest256;
    use medulla_types::transition_refused::MachineState;
    use serde_json::{json, Map, Value};
    use {
        medulla_decision_program_types::friction::required_evidence,
        medulla_decision_program_types::friction::EvidenceKind,
        medulla_decision_program_types::friction::FrictionFiled,
        medulla_decision_program_types::friction::FrictionGroup,
        medulla_decision_program_types::friction::FrictionGroupView,
        medulla_decision_program_types::friction::FrictionGroups,
        medulla_decision_program_types::friction::FrictionMoved,
        medulla_decision_program_types::friction::FrictionOp,
        medulla_decision_program_types::friction::FrictionReport,
        medulla_decision_program_types::friction::FrictionState,
        medulla_decision_program_types::friction::FrictionStepRow,
        medulla_decision_program_types::friction::FrictionTransition,
        medulla_decision_program_types::friction::FRICTION_APPROVAL_ROLE_REQUIRED,
        medulla_decision_program_types::friction::FRICTION_EVIDENCE_REQUIRED,
        medulla_decision_program_types::friction::FRICTION_EVIDENCE_UNKNOWN,
        medulla_decision_program_types::friction::FRICTION_GROUP_CLASS,
        medulla_decision_program_types::friction::FRICTION_GROUP_LIST_MAX,
        medulla_decision_program_types::friction::FRICTION_GROUP_NOT_FOUND,
        medulla_decision_program_types::friction::FRICTION_REPORT_CLASS,
        medulla_decision_program_types::friction::FRICTION_SELF_APPROVED,
        medulla_decision_program_types::friction::FRICTION_SKILL_UNKNOWN,
        medulla_decision_program_types::friction::FRICTION_STEP_CLASS,
        medulla_decision_program_types::friction::FRICTION_TEXT_TOO_LARGE,
        medulla_decision_program_types::friction::FRICTION_TRANSITION_ILLEGAL,
        medulla_decision_program_types::friction::SCOPE_APPROVE,
    };

    use super::{SCOPE_REPORT, SCOPE_TRANSITION};
    use medulla_h_decide_base::decide::planned_write::PlannedWrite;
    use medulla_h_decide_base::decide::stat_promotion::rollouts_key;
    use medulla_h_decide_base::decide::SharedState;
    use {
        medulla_h_decide_base::decide::stat_support::filtered_view_versioned,
        medulla_h_decide_base::decide::stat_support::invalid_parameter,
    };
    use {
        medulla_h_decide_learning::decide::knowledge_flag::served::edge_operation,
        medulla_h_decide_learning::decide::knowledge_flag::served::run_trace_of,
    };

    const ABOUT: &str = "about";
    const OBSERVED_IN: &str = "observed_in";
    const MEMBER_OF: &str = "member_of";
    const STEP_OF: &str = "step_of";
    const RAISED_BY: &str = "raisedBy";

    fn refuse(
        code: medulla_types::refusal_code::RefusalCode,
        detail: impl std::fmt::Display,
    ) -> String {
        format!("{}: {detail}", code.as_str())
    }

    pub(super) async fn plan(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        op: FrictionOp,
    ) -> Result<PlannedWrite, String> {
        let graph = op.graph().to_string();
        let allowed = match &op {
            FrictionOp::Report { .. } => verified.allows_action(SCOPE_REPORT),
            FrictionOp::Transition { transition, .. }
                if transition.to == FrictionState::Approved =>
            {
                verified.allows_action(SCOPE_APPROVE)
            }
            FrictionOp::Transition { .. } => verified.allows_action(SCOPE_TRANSITION),
            _ => verified.allows_action(SCOPE_REPORT) || verified.allows_action(SCOPE_TRANSITION),
        };
        if !allowed {
            return Err(match &op {
                FrictionOp::Transition { transition, .. }
                    if transition.to == FrictionState::Approved =>
                {
                    refuse(
                        FRICTION_APPROVAL_ROLE_REQUIRED,
                        format!("approving a group needs the scope {SCOPE_APPROVE}"),
                    )
                }
                _ => "ACCESS_DENIED: the friction op needs a knowledge scope".to_string(),
            });
        }
        let (view, _) = filtered_view_versioned(state, verified.agent_id(), &graph).await?;
        let tenant = verified.tenant();
        match op {
            FrictionOp::Report { report, .. } => file(graph, &view, verified, report),
            FrictionOp::Transition {
                transition, at_ms, ..
            } => {
                let store = state.write().await.ensure_agent_library()?;
                let revisions: BTreeSet<String> = store
                    .decision_artifacts_with_prefix(tenant, "revision:", 100_000)?
                    .into_iter()
                    .filter_map(|(_, bytes)| {
                        decode_artifact::<RevisionRow>(&bytes, "revision").ok()
                    })
                    .map(|row| row.id)
                    .collect();
                // The held-out receipt and the regression verdict resolve in this tenant's
                // decision store; a missing and a foreign reference both stay absent here.
                let mut stored: BTreeSet<String> = BTreeSet::new();
                if let Some(reference) = transition.evidence_ref.as_deref() {
                    for key in [
                        receipt_key(reference),
                        rollouts_key(reference),
                        record_key(reference),
                    ] {
                        if store.decision_artifact(tenant, &key)?.is_some() {
                            stored.insert(key);
                        }
                    }
                }
                let known = Known { revisions, stored };
                advance(graph, &view, verified, transition, at_ms, &known)
            }
            FrictionOp::Group {
                skill_ref, limit, ..
            } => {
                let mut groups: Vec<FrictionGroup> = groups_of(&view, tenant)
                    .into_iter()
                    .filter(|group| {
                        skill_ref
                            .as_ref()
                            .is_none_or(|skill| &group.skill_ref == skill)
                    })
                    .collect();
                groups.sort_by(|a, b| {
                    b.report_count
                        .cmp(&a.report_count)
                        .then_with(|| a.skill_ref.cmp(&b.skill_ref))
                        .then_with(|| a.group_ref.cmp(&b.group_ref))
                });
                let cap = limit.map_or(FRICTION_GROUP_LIST_MAX, |n| {
                    (n as usize).min(FRICTION_GROUP_LIST_MAX)
                });
                groups.truncate(cap);
                Ok(read_only(
                    graph,
                    ResultPayload::of::<FrictionReportGroup>(FrictionGroups { groups })?,
                ))
            }
            FrictionOp::Get { group_ref, .. } => {
                let group = group_by_ref(&view, tenant, &group_ref)?;
                let view_of = FrictionGroupView {
                    report_ids: report_rows(&view, tenant, &group.group_ref)
                        .into_iter()
                        .map(|row| string(&row, "report_id"))
                        .collect(),
                    steps: step_rows(&view, tenant, &group.group_ref),
                    group,
                };
                Ok(read_only(
                    graph,
                    ResultPayload::of::<FrictionReportGet>(view_of)?,
                ))
            }
        }
    }

    /// What the decision store holds that a move's evidence may name.
    struct Known {
        revisions: BTreeSet<String>,
        /// The store keys, among the evidence's candidates, that exist for the caller's tenant.
        stored: BTreeSet<String>,
    }

    fn read_only(graph: String, reply: ResultPayload) -> PlannedWrite {
        PlannedWrite {
            graph,
            operations: Vec::new(),
            reply,
        }
    }

    fn string(row: &Map<String, Value>, key: &str) -> String {
        row.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn class_of(row: &Map<String, Value>) -> Option<&str> {
        row.get("type").and_then(Value::as_str)
    }

    fn rows_of_class(
        view: &GraphView,
        tenant: &str,
        class: &str,
    ) -> Vec<(String, Map<String, Value>)> {
        let mut ids: Vec<&String> = view.node_properties.keys().collect();
        ids.sort();
        ids.into_iter()
            .filter_map(|id| Some((id.clone(), view.node_row_object(id)?)))
            .filter(|(_, row)| {
                class_of(row) == Some(class)
                    && row.get("tenant_id").and_then(Value::as_str) == Some(tenant)
            })
            .collect()
    }

    fn group_of_row(row: &Map<String, Value>) -> Option<FrictionGroup> {
        let known = [
            "group_ref",
            "skill_ref",
            "report_count",
            "distinct_runs",
            "first_ms",
            "last_ms",
            "state",
        ];
        let mut picked = Map::new();
        for key in known {
            picked.insert(key.to_string(), row.get(key)?.clone());
        }
        serde_json::from_value(Value::Object(picked)).ok()
    }

    fn groups_of(view: &GraphView, tenant: &str) -> Vec<FrictionGroup> {
        rows_of_class(view, tenant, FRICTION_GROUP_CLASS)
            .into_iter()
            .filter_map(|(_, row)| group_of_row(&row))
            .collect()
    }

    fn group_by_ref(
        view: &GraphView,
        tenant: &str,
        group_ref: &str,
    ) -> Result<FrictionGroup, String> {
        view.node_row_object(group_ref)
            .filter(|row| {
                class_of(row) == Some(FRICTION_GROUP_CLASS)
                    && row.get("tenant_id").and_then(Value::as_str) == Some(tenant)
            })
            .and_then(|row| group_of_row(&row))
            .ok_or_else(|| refuse(FRICTION_GROUP_NOT_FOUND, format!("no group {group_ref}")))
    }

    fn report_rows(view: &GraphView, tenant: &str, group_ref: &str) -> Vec<Map<String, Value>> {
        let mut rows: Vec<Map<String, Value>> = rows_of_class(view, tenant, FRICTION_REPORT_CLASS)
            .into_iter()
            .map(|(_, row)| row)
            .filter(|row| row.get("group_ref").and_then(Value::as_str) == Some(group_ref))
            .collect();
        rows.sort_by_key(|row| string(row, "report_id"));
        rows
    }

    fn step_rows(view: &GraphView, tenant: &str, group_ref: &str) -> Vec<FrictionStepRow> {
        let mut rows: Vec<(u64, FrictionStepRow)> =
            rows_of_class(view, tenant, FRICTION_STEP_CLASS)
                .into_iter()
                .filter(|(_, row)| row.get("group_ref").and_then(Value::as_str) == Some(group_ref))
                .filter_map(|(id, row)| {
                    let to = FrictionState::from_stored(row.get("to")?.as_str()?)?;
                    Some((
                        row.get("ordinal").and_then(Value::as_u64).unwrap_or(0),
                        FrictionStepRow {
                            step_id: id,
                            actor: string(&row, "actor"),
                            to,
                            evidence_ref: row
                                .get("evidence_ref")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            at_ms: row.get("at_ms").and_then(Value::as_u64).unwrap_or(0),
                        },
                    ))
                })
                .collect();
        rows.sort_by_key(|(ordinal, _)| *ordinal);
        rows.into_iter().map(|(_, step)| step).collect()
    }

    fn generation_of(group_ref: &str) -> u64 {
        group_ref
            .rsplit_once('#')
            .and_then(|(_, generation)| generation.parse().ok())
            .unwrap_or(0)
    }

    fn receipt_id(parts: &[&str]) -> String {
        let fields: Vec<&[u8]> = parts.iter().map(|part| part.as_bytes()).collect();
        Digest256::framed(b"medulla.friction.receipt.v1", &fields)
            .expect("a receipt frames short fields")
            .to_hex()
    }

    fn group_operation(group: &FrictionGroup, tenant: &str, receipt_id: &str) -> Value {
        let mut properties = match serde_json::to_value(group) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        };
        properties.insert("type".into(), FRICTION_GROUP_CLASS.into());
        properties.insert("node_type".into(), FRICTION_GROUP_CLASS.into());
        properties.insert("tenant_id".into(), tenant.into());
        properties.insert("receipt_id".into(), receipt_id.into());
        json!({"op": "upsert_node", "id": group.group_ref, "properties": properties})
    }

    fn file(
        graph: String,
        view: &GraphView,
        verified: &VerifiedRequestContext,
        mut report: FrictionReport,
    ) -> Result<PlannedWrite, String> {
        let tenant = verified.tenant();
        if let Some(field) = report.oversized_field() {
            return Err(refuse(
                FRICTION_TEXT_TOO_LARGE,
                format!("{field} holds more than 2 KiB"),
            ));
        }
        if report.report_id.is_empty() || report.run_id.is_empty() {
            return Err(invalid_parameter("a report names its id and its run"));
        }
        let skill_known = view.node_row_object(&report.skill_ref).is_some_and(|row| {
            class_of(&row) == Some("Skill")
                && row
                    .get("tenant_id")
                    .and_then(Value::as_str)
                    .is_none_or(|owner| owner == tenant)
        });
        if !skill_known {
            return Err(refuse(
                FRICTION_SKILL_UNKNOWN,
                format!("{} is not a published skill revision", report.skill_ref),
            ));
        }
        report.tenant_id = tenant.to_string();
        report.reported_by = verified.principal_persistence_id();

        let report_node = format!("friction-report:{}", report.report_id);
        let skill_groups: Vec<FrictionGroup> = groups_of(view, tenant)
            .into_iter()
            .filter(|group| group.skill_ref == report.skill_ref)
            .collect();
        let latest = skill_groups
            .iter()
            .max_by_key(|group| generation_of(&group.group_ref));
        // A repeat of one report id counts once, and a report on a finished group opens the next.
        if let Some(group) = latest {
            if report_rows(view, tenant, &group.group_ref)
                .iter()
                .any(|row| string(row, "report_id") == report.report_id)
            {
                return Ok(read_only(
                    graph,
                    ResultPayload::of::<FrictionReportFile>(FrictionFiled {
                        group: group.clone(),
                        counted: false,
                        receipt_id: receipt_id(&[&group.group_ref, &report.report_id]),
                    })?,
                ));
            }
        }
        let group = match latest.filter(|group| !group.state.is_terminal()) {
            Some(open) => {
                let mut runs: BTreeSet<String> = report_rows(view, tenant, &open.group_ref)
                    .iter()
                    .map(|row| string(row, "run_id"))
                    .collect();
                runs.insert(report.run_id.clone());
                FrictionGroup {
                    report_count: open.report_count + 1,
                    distinct_runs: runs.len() as u64,
                    first_ms: open.first_ms.min(report.at_ms),
                    last_ms: open.last_ms.max(report.at_ms),
                    ..open.clone()
                }
            }
            None => FrictionGroup {
                group_ref: format!(
                    "friction:{}#{}",
                    report.skill_ref,
                    latest.map_or(1, |group| generation_of(&group.group_ref) + 1)
                ),
                skill_ref: report.skill_ref.clone(),
                report_count: 1,
                distinct_runs: 1,
                first_ms: report.at_ms,
                last_ms: report.at_ms,
                state: FrictionState::Reported,
            },
        };
        let receipt = receipt_id(&[&group.group_ref, &report.report_id]);
        let mut properties = match serde_json::to_value(&report) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        };
        properties.insert("type".into(), FRICTION_REPORT_CLASS.into());
        properties.insert("node_type".into(), FRICTION_REPORT_CLASS.into());
        properties.insert("group_ref".into(), group.group_ref.clone().into());
        properties.insert("receipt_id".into(), receipt.clone().into());
        // The reader of failure patterns takes a friction report as one failure record.
        properties.insert("run_trace_id".into(), report.run_id.clone().into());
        let mut operations = vec![
            json!({"op": "upsert_node", "id": report_node, "properties": properties}),
            group_operation(&group, tenant, &receipt),
            edge_operation(&report_node, &report.skill_ref, ABOUT, &receipt),
            edge_operation(&report_node, &group.group_ref, MEMBER_OF, &receipt),
        ];
        if let Some(trace) = run_trace_of(view, tenant, &report.run_id) {
            operations.push(edge_operation(&report_node, &trace, OBSERVED_IN, &receipt));
        }
        Ok(PlannedWrite {
            graph,
            operations,
            reply: ResultPayload::of::<FrictionReportFile>(FrictionFiled {
                group,
                counted: true,
                receipt_id: receipt,
            })?,
        })
    }

    /// Whether `evidence` satisfies what the move into `to` requires: the refusal and its detail.
    fn evidence_error(
        to: FrictionState,
        evidence: Option<&str>,
        view: &GraphView,
        verified: &VerifiedRequestContext,
        known: &Known,
    ) -> Option<(medulla_types::refusal_code::RefusalCode, String)> {
        let kind = required_evidence(to);
        if kind == EvidenceKind::None {
            return None;
        }
        let Some(evidence) = evidence.filter(|text| !text.is_empty()) else {
            return Some((
                FRICTION_EVIDENCE_REQUIRED,
                format!("moving to {} needs an evidence_ref", to.stored()),
            ));
        };
        let resolves = |key: String| known.stored.contains(&key);
        match kind {
            // A receipt or a verdict the tenant does not hold answers one code, whether it is
            // missing or belongs to another tenant, so the refusal is no existence oracle.
            EvidenceKind::HeldOutReceipt => {
                let held = resolves(receipt_key(evidence)) || resolves(rollouts_key(evidence));
                (!held).then(|| {
                    (
                        FRICTION_EVIDENCE_UNKNOWN,
                        "the evidence_ref names no held-out receipt".to_string(),
                    )
                })
            }
            EvidenceKind::RegressionVerdict => (!resolves(record_key(evidence))).then(|| {
                (
                    FRICTION_EVIDENCE_UNKNOWN,
                    "the evidence_ref names no regression verdict".to_string(),
                )
            }),
            _ => {
                let bad = match kind {
                    EvidenceKind::FailurePattern => !view.node_properties.contains_key(evidence),
                    EvidenceKind::Revision => !known.revisions.contains(evidence),
                    EvidenceKind::HumanPrincipal => evidence != verified.principal_persistence_id(),
                    EvidenceKind::MeasureWindows => {
                        let windows: Vec<&str> = evidence.split(',').collect();
                        windows.len() != 2
                            || windows.iter().any(|window| window.is_empty())
                            || windows[0] == windows[1]
                    }
                    EvidenceKind::HeldOutReceipt
                    | EvidenceKind::RegressionVerdict
                    | EvidenceKind::None => false,
                };
                bad.then(|| {
                    (
                        FRICTION_EVIDENCE_REQUIRED,
                        format!("the evidence_ref {evidence} does not satisfy {kind:?}"),
                    )
                })
            }
        }
    }

    fn advance(
        graph: String,
        view: &GraphView,
        verified: &VerifiedRequestContext,
        transition: FrictionTransition,
        at_ms: u64,
        known: &Known,
    ) -> Result<PlannedWrite, String> {
        let tenant = verified.tenant();
        let group = group_by_ref(view, tenant, &transition.group_ref)?;
        let Some(event) = group.state.event_to(transition.to) else {
            return Err(refuse(
                FRICTION_TRANSITION_ILLEGAL,
                format!(
                    "{} cannot move to {}",
                    group.state.stored(),
                    transition.to.stored()
                ),
            ));
        };
        // Two-person approval: the approver is the verified caller, and the caller neither
        // reported the group nor made an earlier move on it.
        if transition.to == FrictionState::Approved {
            let approver = verified.principal_persistence_id();
            let reported = report_rows(view, tenant, &group.group_ref)
                .iter()
                .any(|row| string(row, "reported_by") == approver);
            let moved_earlier = step_rows(view, tenant, &group.group_ref)
                .iter()
                .any(|step| step.actor == approver);
            if reported || moved_earlier {
                return Err(refuse(
                    FRICTION_SELF_APPROVED,
                    "the approver reported this group or moved it before approval",
                ));
            }
        }
        if let Some((code, detail)) = evidence_error(
            transition.to,
            transition.evidence_ref.as_deref(),
            view,
            verified,
            known,
        ) {
            return Err(refuse(code, detail));
        }
        let next = group
            .state
            .apply(event)
            .map_err(|refused| refuse(FRICTION_TRANSITION_ILLEGAL, refused))?;
        let moved = FrictionGroup {
            state: next,
            ..group
        };
        let steps = step_rows(view, tenant, &moved.group_ref);
        let ordinal = steps.len() as u64 + 1;
        let step_id = format!("friction-step:{}:{ordinal}", moved.group_ref);
        let actor = verified.principal_persistence_id();
        let receipt = receipt_id(&[&moved.group_ref, &ordinal.to_string(), next.stored()]);
        let step = FrictionStepRow {
            step_id: step_id.clone(),
            actor: actor.clone(),
            to: next,
            evidence_ref: transition.evidence_ref.clone(),
            at_ms,
        };
        let principal = format!("principal:{actor}");
        let mut operations = vec![
            group_operation(&moved, tenant, &receipt),
            json!({
                "op": "upsert_node",
                "id": step_id,
                "properties": {
                    "type": FRICTION_STEP_CLASS,
                    "node_type": FRICTION_STEP_CLASS,
                    "tenant_id": tenant,
                    "group_ref": moved.group_ref,
                    "ordinal": ordinal,
                    "actor": actor,
                    "to": next.stored(),
                    "evidence_ref": transition.evidence_ref,
                    "at_ms": at_ms,
                    "receipt_id": receipt,
                },
            }),
        ];
        if !view.node_properties.contains_key(&principal) {
            operations.push(json!({
                "op": "upsert_node",
                "id": principal,
                "properties": {"type": "Principal", "node_type": "Principal", "tenant_id": tenant},
            }));
        }
        operations.push(edge_operation(
            &step_id,
            &moved.group_ref,
            STEP_OF,
            &receipt,
        ));
        operations.push(edge_operation(&step_id, &principal, RAISED_BY, &receipt));
        Ok(PlannedWrite {
            graph,
            operations,
            reply: ResultPayload::of::<FrictionReportMove>(FrictionMoved { group: moved, step })?,
        })
    }
}
