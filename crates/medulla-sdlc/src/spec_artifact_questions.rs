//! The competency questions over the spec artifact graph (MEDULLA-SDLC-R014.6).
//!
//! UQL has no negation across rows, so each question is a pure function over a pre-read
//! [`SpecGraphView`]: the router reads the records and edges of the caller's graph once, and
//! [`answer`] decides. The answer cites the digests of every record it read.

use std::collections::{BTreeMap, BTreeSet};

use medulla_source_types::lifecycle::LifecycleRefusal;
use medulla_source_types::spec_question::{SpecQuestion, SpecQuestionAnswer};

/// One stored record of the view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewNode {
    /// The node `type`.
    pub node_type: String,
    /// The string properties of the node.
    pub props: BTreeMap<String, String>,
    /// The SHA-256 of the stored record.
    pub digest: String,
}

/// The records and typed edges one question reads.
#[derive(Clone, Debug, Default)]
pub struct SpecGraphView {
    /// Nodes by id.
    pub nodes: BTreeMap<String, ViewNode>,
    /// Edges as `(source, relationship, target)`.
    pub edges: BTreeSet<(String, String, String)>,
}

impl SpecGraphView {
    fn prop<'a>(&'a self, id: &str, key: &str) -> &'a str {
        self.nodes
            .get(id)
            .and_then(|node| node.props.get(key))
            .map_or("", String::as_str)
    }

    fn of_type<'a>(&'a self, node_type: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.nodes
            .iter()
            .filter(move |(_, node)| node.node_type == node_type)
            .map(|(id, _)| id.as_str())
    }

    fn out<'a>(
        &'a self,
        source: &'a str,
        relationship: &'a str,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.edges
            .iter()
            .filter(move |(from, rel, _)| from == source && rel == relationship)
            .map(|(_, _, to)| to.as_str())
    }

    fn inbound<'a>(
        &'a self,
        target: &'a str,
        relationship: &'a str,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.edges
            .iter()
            .filter(move |(_, rel, to)| to == target && rel == relationship)
            .map(|(from, _, _)| from.as_str())
    }

    fn unverified(&self, requirement: &str) -> bool {
        self.nodes.contains_key(requirement) && self.prop(requirement, "rowStatus") != "VERIFIED"
    }
}

/// The ids of a task in natural order: digit runs compare as numbers.
fn natural_key(text: &str) -> Vec<(u8, u64, String)> {
    let mut key = Vec::new();
    let mut digits = String::new();
    let mut letters = String::new();
    let flush = |key: &mut Vec<(u8, u64, String)>, digits: &mut String, letters: &mut String| {
        if !letters.is_empty() {
            key.push((0, 0, std::mem::take(letters)));
        }
        if !digits.is_empty() {
            key.push((1, digits.parse().unwrap_or(u64::MAX), String::new()));
            digits.clear();
        }
    };
    for c in text.chars() {
        if c.is_ascii_digit() {
            if !letters.is_empty() {
                flush(&mut key, &mut digits, &mut letters);
            }
            digits.push(c);
        } else {
            if !digits.is_empty() {
                flush(&mut key, &mut digits, &mut letters);
            }
            letters.push(c);
        }
    }
    flush(&mut key, &mut digits, &mut letters);
    key
}

/// Collects rows and the digests of the records an answer read.
struct Collector<'a> {
    view: &'a SpecGraphView,
    rows: Vec<BTreeMap<String, String>>,
    digests: BTreeSet<String>,
}

impl<'a> Collector<'a> {
    fn new(view: &'a SpecGraphView) -> Self {
        Self {
            view,
            rows: Vec::new(),
            digests: BTreeSet::new(),
        }
    }

    fn read(&mut self, id: &str) {
        if let Some(node) = self.view.nodes.get(id) {
            if !node.digest.is_empty() {
                self.digests.insert(node.digest.clone());
            }
        }
    }

    fn row(&mut self, read: &[&str], pairs: &[(&str, &str)]) {
        for id in read {
            self.read(id);
        }
        self.rows.push(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        );
    }

    fn finish(self) -> SpecQuestionAnswer {
        SpecQuestionAnswer {
            rows: self.rows,
            digests: self.digests.into_iter().collect(),
        }
    }
}

/// The provider requirement nodes of a contract: `provider_rows` holds `<repo>:<ROW-ID>` pairs.
fn provider_nodes(view: &SpecGraphView, contract: &str) -> Vec<String> {
    view.prop(contract, "provider_rows")
        .split(',')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .map(|pair| format!("requirement:{pair}"))
        .collect()
}

/// The `(task, row, contract, provider)` quadruples of question 3, in task id order.
fn blocked_tasks(view: &SpecGraphView) -> Vec<(String, String, String, String)> {
    let mut blocked = Vec::new();
    for task in view.of_type("TaskLine") {
        for row in view.out(task, "forRow") {
            for contract in view.out(row, "crossesSeam") {
                for provider in provider_nodes(view, contract) {
                    if view.unverified(&provider) {
                        blocked.push((
                            task.to_owned(),
                            row.to_owned(),
                            contract.to_owned(),
                            provider,
                        ));
                    }
                }
            }
        }
    }
    blocked
}

fn require_subject<'a>(
    question: SpecQuestion,
    subject: Option<&'a str>,
) -> Result<Option<&'a str>, LifecycleRefusal> {
    match (question.takes_subject(), subject) {
        (true, Some(value)) if !value.trim().is_empty() => Ok(Some(value)),
        (false, None) => Ok(None),
        (true, _) => Err(LifecycleRefusal::ParameterInvalid(format!(
            "{question:?} needs a subject"
        ))),
        (false, Some(_)) => Err(LifecycleRefusal::ParameterInvalid(format!(
            "{question:?} takes no subject"
        ))),
    }
}

/// Answer `question` over `view`. `subject` is the phase (`NextUnblockedTask`), the goal node id
/// (`ChangesAdvancingGoal`) or the changeset id (`PrinciplesAtRisk`).
pub fn answer(
    question: SpecQuestion,
    subject: Option<&str>,
    view: &SpecGraphView,
) -> Result<SpecQuestionAnswer, LifecycleRefusal> {
    let subject = require_subject(question, subject)?;
    let mut out = Collector::new(view);
    answer_lifecycle(question, subject, view, &mut out)?;
    match question {
        SpecQuestion::UnreachableRequirements => {
            for requirement in view.of_type("Requirement") {
                if view
                    .out(requirement, "implementedByCommit")
                    .next()
                    .is_none()
                    && view.out(requirement, "verifiedBy").next().is_none()
                {
                    out.row(
                        &[requirement],
                        &[
                            ("requirement", requirement),
                            ("row", view.prop(requirement, "rowId")),
                        ],
                    );
                }
            }
        }
        SpecQuestion::UncoveredCodeItems => {
            for symbol in view.of_type("SYMBOL") {
                let covered = view.edges.iter().any(|(from, _, to)| {
                    to == symbol
                        && view
                            .nodes
                            .get(from)
                            .is_some_and(|n| n.node_type == "Requirement")
                });
                if !covered {
                    out.row(&[symbol], &[("symbol", symbol)]);
                }
            }
        }
        SpecQuestion::TasksBlockedByContract => {
            for (task, row, contract, provider) in blocked_tasks(view) {
                let (task, row, contract, provider) = (
                    task.as_str(),
                    row.as_str(),
                    contract.as_str(),
                    provider.as_str(),
                );
                out.row(
                    &[task, row, contract, provider],
                    &[
                        ("task", task),
                        ("row", row),
                        ("contract", contract),
                        ("provider", provider),
                    ],
                );
            }
        }
        SpecQuestion::PrinciplesAtRisk => {
            let changeset = format!("commit:{}", subject.unwrap_or_default());
            let mut seen = BTreeSet::new();
            for requirement in view
                .out(&changeset, "citesRequirement")
                .filter(|r| view.unverified(r))
            {
                for principle in view.out(requirement, "governedBy") {
                    if seen.insert((requirement.to_owned(), principle.to_owned())) {
                        out.row(
                            &[changeset.as_str(), requirement, principle],
                            &[
                                ("principle", principle),
                                ("key", view.prop(principle, "key")),
                                ("title", view.prop(principle, "title")),
                                ("requirement", requirement),
                            ],
                        );
                    }
                }
            }
        }
        SpecQuestion::ChangesAdvancingGoal => {
            let goal = subject.unwrap_or_default();
            if !view.nodes.get(goal).is_some_and(|n| n.node_type == "Goal") {
                return Err(LifecycleRefusal::CompetencySubjectUnknown(goal.to_owned()));
            }
            out.read(goal);
            for requirement in view.inbound(goal, "targetsGoal") {
                for changeset in view.inbound(requirement, "citesRequirement") {
                    out.row(
                        &[requirement, changeset],
                        &[("changeset", changeset), ("requirement", requirement)],
                    );
                }
            }
        }
        SpecQuestion::NextUnblockedTask => {
            let phase = subject.unwrap_or_default();
            let phase_tasks: Vec<&str> = view
                .of_type("TaskLine")
                .filter(|task| view.prop(task, "phase") == phase)
                .collect();
            if phase_tasks.is_empty() {
                return Err(LifecycleRefusal::CompetencySubjectUnknown(phase.to_owned()));
            }
            let blocked: BTreeSet<String> = blocked_tasks(view)
                .into_iter()
                .map(|(task, ..)| task)
                .collect();
            let next = phase_tasks
                .into_iter()
                .filter(|task| view.prop(task, "ticked") == "false" && !blocked.contains(*task))
                .filter(|task| {
                    view.out(task, "forRow").all(|row| {
                        view.out(row, "dependsOnRow")
                            .all(|dep| !view.unverified(dep))
                    })
                })
                .min_by_key(|task| natural_key(view.prop(task, "taskId")));
            if let Some(task) = next {
                let row = view.out(task, "forRow").next().unwrap_or_default();
                let sections: Vec<&str> = view.inbound(row, "covers").collect();
                let contracts: Vec<&str> = view.out(row, "crossesSeam").collect();
                let code: Vec<&str> = view.out(row, "implementedBy").collect();
                let (sections, contracts, code) =
                    (sections.join(","), contracts.join(","), code.join(","));
                out.row(
                    &[task, row],
                    &[
                        ("task", task),
                        ("task_id", view.prop(task, "taskId")),
                        ("row", row),
                        ("plan_sections", sections.as_str()),
                        ("contracts", contracts.as_str()),
                        ("code_items", code.as_str()),
                    ],
                );
            }
        }
        // The lifecycle questions are answered by `answer_lifecycle` above.
        SpecQuestion::UnboundRequirements
        | SpecQuestion::DeploymentOfRequirement
        | SpecQuestion::OpenDefectsInRelease
        | SpecQuestion::ChangesetsBetweenDeployments => {}
    }
    Ok(out.finish())
}

/// The statuses that close a defect for question `OpenDefectsInRelease`.
const SETTLED_DEFECT_STATUSES: [&str; 3] = ["verified", "closed", "rejected"];

fn require_node_type(
    view: &SpecGraphView,
    id: &str,
    node_type: &str,
) -> Result<(), LifecycleRefusal> {
    if view.nodes.get(id).is_some_and(|n| n.node_type == node_type) {
        Ok(())
    } else {
        Err(LifecycleRefusal::CompetencySubjectUnknown(id.to_owned()))
    }
}

/// The lifecycle questions of MEDULLA-SDLC-R006.3, over the same pre-read view.
fn answer_lifecycle(
    question: SpecQuestion,
    subject: Option<&str>,
    view: &SpecGraphView,
    out: &mut Collector<'_>,
) -> Result<(), LifecycleRefusal> {
    let subject = subject.unwrap_or_default();
    match question {
        SpecQuestion::UnboundRequirements => {
            for requirement in view.of_type("Requirement") {
                if view.out(requirement, "verifiedBy").next().is_none() {
                    out.row(
                        &[requirement],
                        &[
                            ("requirement", requirement),
                            ("row", view.prop(requirement, "rowId")),
                        ],
                    );
                }
            }
        }
        SpecQuestion::DeploymentOfRequirement => {
            require_node_type(view, subject, "Requirement")?;
            let mut seen = BTreeSet::new();
            for symbol in view.inbound(subject, "implements") {
                for changeset in view.inbound(symbol, "changes") {
                    for release in view.inbound(changeset, "contains") {
                        for deployment in view.inbound(release, "deploys") {
                            if seen.insert((deployment, release, changeset, symbol)) {
                                out.row(
                                    &[subject, symbol, changeset, release, deployment],
                                    &[
                                        ("deployment", deployment),
                                        ("release", release),
                                        ("changeset", changeset),
                                        ("symbol", symbol),
                                    ],
                                );
                            }
                        }
                    }
                }
            }
        }
        SpecQuestion::OpenDefectsInRelease => {
            require_node_type(view, subject, "Release")?;
            out.read(subject);
            let contained: BTreeSet<&str> = view.out(subject, "contains").collect();
            let mut seen = BTreeSet::new();
            for defect in view.of_type("Defect") {
                if SETTLED_DEFECT_STATUSES.contains(&view.prop(defect, "status")) {
                    continue;
                }
                for symbol in view.out(defect, "affects") {
                    for changeset in view.inbound(symbol, "changes") {
                        if contained.contains(changeset) && seen.insert((defect, symbol, changeset))
                        {
                            out.row(
                                &[defect, symbol, changeset],
                                &[
                                    ("defect", defect),
                                    ("symbol", symbol),
                                    ("changeset", changeset),
                                ],
                            );
                        }
                    }
                }
            }
        }
        SpecQuestion::ChangesetsBetweenDeployments => {
            let Some((first, second)) = subject.split_once(',') else {
                return Err(LifecycleRefusal::ParameterInvalid(
                    "the subject names two deployments, `<first>,<second>`".to_owned(),
                ));
            };
            let (first, second) = (first.trim(), second.trim());
            require_node_type(view, first, "Deployment")?;
            require_node_type(view, second, "Deployment")?;
            let contained = |deployment: &str| -> BTreeSet<&str> {
                view.out(deployment, "deploys")
                    .flat_map(|release| view.out(release, "contains"))
                    .collect()
            };
            let other = contained(second);
            out.read(first);
            out.read(second);
            for changeset in contained(first).difference(&other).copied() {
                out.row(&[changeset], &[("changeset", changeset)]);
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(node_type: &str, props: &[(&str, &str)]) -> ViewNode {
        ViewNode {
            node_type: node_type.into(),
            props: props
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
            digest: format!("d-{node_type}-{}", props.len()),
        }
    }

    fn edge(view: &mut SpecGraphView, from: &str, rel: &str, to: &str) {
        view.edges.insert((from.into(), rel.into(), to.into()));
    }

    // spec: MEDULLA-SDLC-R006.3
    #[test]
    fn lifecycle_questions_follow_the_derived_edges() {
        let mut view = SpecGraphView::default();
        for (id, kind, props) in [
            ("requirement:r:A-R1", "Requirement", vec![("rowId", "A-R1")]),
            ("requirement:r:A-R2", "Requirement", vec![("rowId", "A-R2")]),
            ("symbol:s", "SYMBOL", vec![]),
            ("commit:c1", "Changeset", vec![]),
            ("commit:c2", "Changeset", vec![]),
            ("release:r:1", "Release", vec![]),
            ("release:r:2", "Release", vec![]),
            ("deployment:d1", "Deployment", vec![]),
            ("deployment:d2", "Deployment", vec![]),
            ("defect:r/1", "Defect", vec![("status", "triaged")]),
            ("defect:r/2", "Defect", vec![("status", "closed")]),
            ("testfile:r:t", "TestFile", vec![]),
        ] {
            view.nodes.insert(id.into(), node(kind, &props));
        }
        for (from, rel, to) in [
            ("requirement:r:A-R1", "verifiedBy", "testfile:r:t"),
            ("symbol:s", "implements", "requirement:r:A-R1"),
            ("commit:c1", "changes", "symbol:s"),
            ("release:r:1", "contains", "commit:c1"),
            ("release:r:2", "contains", "commit:c1"),
            ("release:r:2", "contains", "commit:c2"),
            ("deployment:d1", "deploys", "release:r:1"),
            ("deployment:d2", "deploys", "release:r:2"),
            ("defect:r/1", "affects", "symbol:s"),
            ("defect:r/2", "affects", "symbol:s"),
        ] {
            edge(&mut view, from, rel, to);
        }
        let unbound = answer(SpecQuestion::UnboundRequirements, None, &view).unwrap();
        assert_eq!(unbound.rows.len(), 1);
        assert_eq!(unbound.rows[0]["row"], "A-R2");
        let deployed = answer(
            SpecQuestion::DeploymentOfRequirement,
            Some("requirement:r:A-R1"),
            &view,
        )
        .unwrap();
        let deployments: Vec<&str> = deployed.rows.iter().map(|r| r["deployment"].as_str()).collect();
        assert_eq!(deployments, ["deployment:d1", "deployment:d2"]);
        let open = answer(SpecQuestion::OpenDefectsInRelease, Some("release:r:1"), &view).unwrap();
        assert_eq!(open.rows.len(), 1);
        assert_eq!(open.rows[0]["defect"], "defect:r/1");
        let between = answer(
            SpecQuestion::ChangesetsBetweenDeployments,
            Some("deployment:d2,deployment:d1"),
            &view,
        )
        .unwrap();
        assert_eq!(between.rows.len(), 1);
        assert_eq!(between.rows[0]["changeset"], "commit:c2");
        for (question, subject) in [
            (SpecQuestion::DeploymentOfRequirement, "requirement:r:none"),
            (SpecQuestion::OpenDefectsInRelease, "release:r:none"),
        ] {
            let refused = answer(question, Some(subject), &view).unwrap_err();
            assert_eq!(refused.code(), "COMPETENCY_SUBJECT_UNKNOWN");
        }
        let malformed = answer(SpecQuestion::ChangesetsBetweenDeployments, Some("one"), &view);
        assert_eq!(malformed.unwrap_err().code(), "PARAMETER_INVALID");
    }

    // spec: MEDULLA-SDLC-R014.6
    #[test]
    fn next_unblocked_task_skips_ticked_blocked_and_open_dependencies() {
        let mut view = SpecGraphView::default();
        for (id, status) in [
            ("requirement:r:A-R1", "VERIFIED"),
            ("requirement:r:A-R2", "SPECIFIED"),
            ("requirement:r:A-R3", "SPECIFIED"),
        ] {
            view.nodes.insert(
                id.into(),
                node("Requirement", &[("rowStatus", status), ("rowId", id)]),
            );
        }
        view.nodes.insert(
            "contract:r:c".into(),
            node("Contract", &[("provider_rows", "r:A-R2")]),
        );
        for (id, task_id, row, ticked) in [
            ("task:r:t#T002", "T002", "requirement:r:A-R1", "true"),
            ("task:r:t#T003", "T003", "requirement:r:A-R3", "false"),
            ("task:r:t#T010", "T010", "requirement:r:A-R1", "false"),
            ("task:r:t#T009", "T009", "requirement:r:A-R1", "false"),
        ] {
            view.nodes.insert(
                id.into(),
                node(
                    "TaskLine",
                    &[("taskId", task_id), ("phase", "1"), ("ticked", ticked)],
                ),
            );
            edge(&mut view, id, "forRow", row);
        }
        edge(
            &mut view,
            "requirement:r:A-R3",
            "crossesSeam",
            "contract:r:c",
        );
        let blocked = answer(SpecQuestion::TasksBlockedByContract, None, &view).unwrap();
        assert_eq!(blocked.rows.len(), 1);
        assert_eq!(blocked.rows[0]["task"], "task:r:t#T003");
        let next = answer(SpecQuestion::NextUnblockedTask, Some("1"), &view).unwrap();
        assert_eq!(
            next.rows[0]["task_id"], "T009",
            "T003 is blocked, T002 ticked, T009 < T010 naturally"
        );
        assert!(!next.digests.is_empty());
        let unknown = answer(SpecQuestion::NextUnblockedTask, Some("9"), &view);
        assert!(matches!(
            unknown,
            Err(LifecycleRefusal::CompetencySubjectUnknown(_))
        ));
        assert!(matches!(
            answer(SpecQuestion::UncoveredCodeItems, Some("x"), &view),
            Err(LifecycleRefusal::ParameterInvalid(_))
        ));
    }
}
