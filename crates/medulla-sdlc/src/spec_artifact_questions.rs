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
    answer_context(question, subject, view, &mut out)?;
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
        | SpecQuestion::ChangesetsBetweenDeployments
        | SpecQuestion::OpenTasksOfBundle
        // The context questions are answered by `answer_context` above.
        | SpecQuestion::TaskContext
        | SpecQuestion::PlanSection
        | SpecQuestion::Contract
        | SpecQuestion::SeamRouting
        | SpecQuestion::PlanToGoalPhase => {}
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
        SpecQuestion::OpenTasksOfBundle => open_tasks_of_bundle(subject, view, out)?,
        _ => {}
    }
    Ok(())
}

/// The ids joined with commas, in the order given with repeats dropped.
fn joined<'a>(ids: impl IntoIterator<Item = &'a str>) -> String {
    let mut seen = BTreeSet::new();
    ids.into_iter()
        .filter(|id| seen.insert(*id))
        .collect::<Vec<_>>()
        .join(",")
}

/// The context questions of MEDULLA-SDLC-R014.15, over the same pre-read view. Each takes a
/// node id and refuses `COMPETENCY_SUBJECT_UNKNOWN` when no record of the right type has it.
fn answer_context(
    question: SpecQuestion,
    subject: Option<&str>,
    view: &SpecGraphView,
    out: &mut Collector<'_>,
) -> Result<(), LifecycleRefusal> {
    let subject = subject.unwrap_or_default();
    match question {
        SpecQuestion::TaskContext => task_context(subject, view, out),
        SpecQuestion::PlanSection => plan_section(subject, view, out),
        SpecQuestion::Contract => contract(subject, view, out),
        SpecQuestion::SeamRouting => seam_routing(subject, view, out),
        SpecQuestion::PlanToGoalPhase => plan_to_goal_phase(subject, view, out),
        _ => Ok(()),
    }
}

/// One row for one task: the row it serves, the plan sections that cover that row, the
/// contracts it crosses, its code items, the rows it depends on and what blocks it.
fn task_context(
    task: &str,
    view: &SpecGraphView,
    out: &mut Collector<'_>,
) -> Result<(), LifecycleRefusal> {
    require_node_type(view, task, "TaskLine")?;
    let row = view.out(task, "forRow").next().unwrap_or_default();
    let sections: Vec<&str> = view.inbound(row, "covers").collect();
    let contracts: Vec<&str> = view.out(row, "crossesSeam").collect();
    let code: Vec<&str> = view.out(row, "implementedBy").collect();
    let depends: Vec<&str> = view.out(row, "dependsOnRow").collect();
    let open_dependencies: Vec<&str> = depends
        .iter()
        .copied()
        .filter(|dependency| view.unverified(dependency))
        .collect();
    let providers: Vec<String> = blocked_tasks(view)
        .into_iter()
        .filter(|(blocked, ..)| blocked == task)
        .map(|(_, _, _, provider)| provider)
        .collect();
    let blocked_by = joined(providers.iter().map(String::as_str));
    let blocked = !blocked_by.is_empty() || !open_dependencies.is_empty();
    let mut read = vec![task, row];
    read.extend(&sections);
    read.extend(&contracts);
    read.extend(&depends);
    out.row(
        &read,
        &[
            ("task", task),
            ("task_id", view.prop(task, "taskId")),
            ("phase", view.prop(task, "phase")),
            ("ticked", view.prop(task, "ticked")),
            ("row", row),
            ("row_id", view.prop(row, "rowId")),
            ("row_status", view.prop(row, "rowStatus")),
            ("plan_sections", joined(sections.iter().copied()).as_str()),
            ("contracts", joined(contracts.iter().copied()).as_str()),
            ("code_items", joined(code.iter().copied()).as_str()),
            ("depends_on", joined(depends.iter().copied()).as_str()),
            (
                "open_dependencies",
                joined(open_dependencies.iter().copied()).as_str(),
            ),
            ("blocked_by_providers", blocked_by.as_str()),
            ("blocked", if blocked { "true" } else { "false" }),
        ],
    );
    Ok(())
}

/// One row for one plan section: its span, phase, digest, text and covered rows.
fn plan_section(
    section: &str,
    view: &SpecGraphView,
    out: &mut Collector<'_>,
) -> Result<(), LifecycleRefusal> {
    require_node_type(view, section, "PlanSection")?;
    let covered: Vec<&str> = view.out(section, "covers").collect();
    let mut read = vec![section];
    read.extend(&covered);
    out.row(
        &read,
        &[
            ("section", section),
            ("repo", view.prop(section, "repo")),
            ("file", view.prop(section, "file")),
            ("title", view.prop(section, "title")),
            ("phase", view.prop(section, "phase")),
            ("start_line", view.prop(section, "start_line")),
            ("end_line", view.prop(section, "end_line")),
            ("section_digest", view.prop(section, "digest")),
            ("text", view.prop(section, "text")),
            ("requirements", joined(covered.iter().copied()).as_str()),
        ],
    );
    Ok(())
}

/// One row for one contract: its declared parts, the rows that cross it and the providers
/// that are not yet verified.
fn contract(
    contract: &str,
    view: &SpecGraphView,
    out: &mut Collector<'_>,
) -> Result<(), LifecycleRefusal> {
    require_node_type(view, contract, "Contract")?;
    let crossing: Vec<&str> = view.inbound(contract, "crossesSeam").collect();
    let providers = provider_nodes(view, contract);
    let unverified: Vec<&str> = providers
        .iter()
        .map(String::as_str)
        .filter(|provider| view.unverified(provider))
        .collect();
    let mut read = vec![contract];
    read.extend(&crossing);
    read.extend(providers.iter().map(String::as_str));
    out.row(
        &read,
        &[
            ("contract", contract),
            ("repo", view.prop(contract, "repo")),
            ("seam", view.prop(contract, "seam")),
            ("declared_in", view.prop(contract, "declaredIn")),
            ("provider_repos", view.prop(contract, "provider_repos")),
            ("provider_rows", view.prop(contract, "provider_rows")),
            ("consumer_repos", view.prop(contract, "consumer_repos")),
            ("wire_shape", view.prop(contract, "wire_shape")),
            ("refusal_codes", view.prop(contract, "refusal_codes")),
            ("types", view.prop(contract, "types")),
            ("vectors", view.prop(contract, "vectors")),
            ("crossing_rows", joined(crossing.iter().copied()).as_str()),
            (
                "unverified_providers",
                joined(unverified.iter().copied()).as_str(),
            ),
        ],
    );
    Ok(())
}

/// One row per provider of each contract the requirement crosses: which repository and row
/// provides the seam, whether this row is itself a provider, and the provider's status.
fn seam_routing(
    requirement: &str,
    view: &SpecGraphView,
    out: &mut Collector<'_>,
) -> Result<(), LifecycleRefusal> {
    require_node_type(view, requirement, "Requirement")?;
    out.read(requirement);
    for contract in view.out(requirement, "crossesSeam") {
        let providers = provider_nodes(view, contract);
        let direction = if providers.iter().any(|provider| provider == requirement) {
            "provider"
        } else {
            "consumer"
        };
        for provider in &providers {
            let (repo, row) = provider
                .strip_prefix("requirement:")
                .and_then(|pair| pair.split_once(':'))
                .unwrap_or_default();
            let status = if view.nodes.contains_key(provider) {
                view.prop(provider, "rowStatus")
            } else {
                "unknown"
            };
            out.row(
                &[contract, provider.as_str()],
                &[
                    ("contract", contract),
                    ("seam", view.prop(contract, "seam")),
                    ("direction", direction),
                    ("provider", provider.as_str()),
                    ("provider_repo", repo),
                    ("provider_row", row),
                    ("provider_status", status),
                    ("consumer_repos", view.prop(contract, "consumer_repos")),
                ],
            );
        }
    }
    Ok(())
}

/// One row per requirement a plan section covers: the section's phase and the goals that
/// requirement targets (empty when it targets none).
fn plan_to_goal_phase(
    section: &str,
    view: &SpecGraphView,
    out: &mut Collector<'_>,
) -> Result<(), LifecycleRefusal> {
    require_node_type(view, section, "PlanSection")?;
    out.read(section);
    let phase = view.prop(section, "phase");
    for requirement in view.out(section, "covers") {
        let goals: Vec<&str> = view.out(requirement, "targetsGoal").collect();
        let mut read = vec![requirement];
        read.extend(&goals);
        out.row(
            &read,
            &[
                ("section", section),
                ("phase", phase),
                ("requirement", requirement),
                ("row", view.prop(requirement, "rowId")),
                ("goals", joined(goals.iter().copied()).as_str()),
            ],
        );
    }
    Ok(())
}

/// The open tasks of one bundle (MEDULLA-SDLC-R018.2). A bundle is a `ReleaseBundle` node
/// whose `train_order` lists repository names in release order and whose `open_tasks` lists
/// row ids; the `inBundle` edges say which requirement node of which repository each row is.
/// The rows come out ordered by the position of their repository in the train order (a
/// repository the order does not name comes last), then by row id in natural order, and each
/// row names its unticked tasks.
fn open_tasks_of_bundle(
    bundle: &str,
    view: &SpecGraphView,
    out: &mut Collector<'_>,
) -> Result<(), LifecycleRefusal> {
    require_node_type(view, bundle, "ReleaseBundle")?;
    out.read(bundle);
    let train: Vec<&str> = view.prop(bundle, "train_order").split(',').collect();
    let open: BTreeSet<&str> = view
        .prop(bundle, "open_tasks")
        .split(',')
        .filter(|row| !row.is_empty())
        .collect();
    let mut rows: Vec<(usize, Vec<(u8, u64, String)>, &str, &str)> = view
        .inbound(bundle, "inBundle")
        .filter(|requirement| open.contains(view.prop(requirement, "rowId")))
        .map(|requirement| {
            let repo = view.prop(requirement, "repo");
            let position = train
                .iter()
                .position(|name| *name == repo)
                .unwrap_or(train.len());
            (
                position,
                natural_key(view.prop(requirement, "rowId")),
                requirement,
                repo,
            )
        })
        .collect();
    rows.sort();
    for (position, _, requirement, repo) in rows {
        let mut tasks: Vec<&str> = view
            .of_type("TaskLine")
            .filter(|task| view.prop(task, "ticked") == "false")
            .filter(|task| view.out(task, "forRow").any(|row| row == requirement))
            .collect();
        tasks.sort_by_key(|task| natural_key(view.prop(task, "taskId")));
        let position = position.to_string();
        let joined = tasks.join(",");
        let mut read = vec![requirement];
        read.extend(&tasks);
        out.row(
            &read,
            &[
                ("position", position.as_str()),
                ("repo", repo),
                ("row", view.prop(requirement, "rowId")),
                ("requirement", requirement),
                ("tasks", joined.as_str()),
            ],
        );
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

    // spec: MEDULLA-SDLC-R018.2
    #[test]
    fn open_tasks_of_a_bundle_come_out_in_train_order() {
        let mut view = SpecGraphView::default();
        view.nodes.insert(
            "bundle:b".into(),
            node(
                "ReleaseBundle",
                &[("train_order", "other,demo"), ("open_tasks", "D-R2,O-R9,D-R1")],
            ),
        );
        for (id, repo, row) in [
            ("requirement:demo:D-R1", "demo", "D-R1"),
            ("requirement:demo:D-R2", "demo", "D-R2"),
            ("requirement:demo:D-R3", "demo", "D-R3"),
            ("requirement:other:O-R9", "other", "O-R9"),
        ] {
            view.nodes
                .insert(id.into(), node("Requirement", &[("repo", repo), ("rowId", row)]));
            edge(&mut view, id, "inBundle", "bundle:b");
        }
        for (id, task_id, row, ticked) in [
            ("task:demo:t#T2", "T2", "requirement:demo:D-R2", "false"),
            ("task:demo:t#T10", "T10", "requirement:demo:D-R2", "false"),
            ("task:demo:t#T1", "T1", "requirement:demo:D-R1", "true"),
            ("task:other:t#T1", "T1", "requirement:other:O-R9", "false"),
        ] {
            view.nodes.insert(
                id.into(),
                node("TaskLine", &[("taskId", task_id), ("ticked", ticked)]),
            );
            edge(&mut view, id, "forRow", row);
        }
        let answered = answer(SpecQuestion::OpenTasksOfBundle, Some("bundle:b"), &view).unwrap();
        let rows: Vec<&str> = answered.rows.iter().map(|r| r["row"].as_str()).collect();
        assert_eq!(rows, ["O-R9", "D-R1", "D-R2"], "other first, D-R3 is not open");
        assert_eq!(answered.rows[2]["tasks"], "task:demo:t#T2,task:demo:t#T10");
        assert_eq!(answered.rows[1]["tasks"], "");
        let unknown = answer(SpecQuestion::OpenTasksOfBundle, Some("bundle:none"), &view);
        assert_eq!(unknown.unwrap_err().code(), "COMPETENCY_SUBJECT_UNKNOWN");
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

    // spec: MEDULLA-SDLC-R014.15
    #[test]
    fn context_questions_answer_task_section_contract_seam_and_goal_phase() {
        let mut view = SpecGraphView::default();
        for (id, status, repo, row) in [
            ("requirement:r:A-R1", "SPECIFIED", "r", "A-R1"),
            ("requirement:r:A-R2", "VERIFIED", "r", "A-R2"),
            ("requirement:p:P-R7", "SPECIFIED", "p", "P-R7"),
        ] {
            view.nodes.insert(
                id.into(),
                node(
                    "Requirement",
                    &[("rowStatus", status), ("repo", repo), ("rowId", row)],
                ),
            );
        }
        view.nodes.insert(
            "contract:r:seam".into(),
            node(
                "Contract",
                &[
                    ("seam", "seam"),
                    ("provider_rows", "p:P-R7"),
                    ("consumer_repos", "r"),
                    ("wire_shape", "{a}"),
                ],
            ),
        );
        view.nodes.insert(
            "task:r:t#T1".into(),
            node("TaskLine", &[("taskId", "T1"), ("phase", "2"), ("ticked", "false")]),
        );
        view.nodes.insert(
            "plansection:r:p.md#L3".into(),
            node(
                "PlanSection",
                &[("title", "Phase 2"), ("phase", "2"), ("start_line", "3"), ("digest", "sha256:x")],
            ),
        );
        view.nodes
            .insert("goal:g".into(), node("Goal", &[("statement", "ship")]));
        for (from, rel, to) in [
            ("task:r:t#T1", "forRow", "requirement:r:A-R1"),
            ("requirement:r:A-R1", "crossesSeam", "contract:r:seam"),
            ("requirement:p:P-R7", "crossesSeam", "contract:r:seam"),
            ("requirement:r:A-R1", "dependsOnRow", "requirement:r:A-R2"),
            ("plansection:r:p.md#L3", "covers", "requirement:r:A-R1"),
            ("plansection:r:p.md#L3", "covers", "requirement:r:A-R2"),
            ("requirement:r:A-R1", "targetsGoal", "goal:g"),
        ] {
            edge(&mut view, from, rel, to);
        }

        let task = answer(SpecQuestion::TaskContext, Some("task:r:t#T1"), &view).unwrap();
        assert_eq!(task.rows.len(), 1);
        let row = &task.rows[0];
        assert_eq!(row["row"], "requirement:r:A-R1");
        assert_eq!(row["plan_sections"], "plansection:r:p.md#L3");
        assert_eq!(row["contracts"], "contract:r:seam");
        assert_eq!(row["depends_on"], "requirement:r:A-R2");
        assert_eq!(row["open_dependencies"], "", "A-R2 is verified");
        assert_eq!(row["blocked_by_providers"], "requirement:p:P-R7");
        assert_eq!(row["blocked"], "true");
        assert!(!task.digests.is_empty());

        let section = answer(SpecQuestion::PlanSection, Some("plansection:r:p.md#L3"), &view).unwrap();
        assert_eq!(section.rows[0]["phase"], "2");
        assert_eq!(section.rows[0]["section_digest"], "sha256:x");
        assert_eq!(
            section.rows[0]["requirements"],
            "requirement:r:A-R1,requirement:r:A-R2"
        );

        let seam = answer(SpecQuestion::Contract, Some("contract:r:seam"), &view).unwrap();
        assert_eq!(seam.rows[0]["wire_shape"], "{a}");
        assert_eq!(
            seam.rows[0]["crossing_rows"],
            "requirement:p:P-R7,requirement:r:A-R1"
        );
        assert_eq!(seam.rows[0]["unverified_providers"], "requirement:p:P-R7");

        let routed = answer(SpecQuestion::SeamRouting, Some("requirement:r:A-R1"), &view).unwrap();
        assert_eq!(routed.rows.len(), 1);
        assert_eq!(routed.rows[0]["direction"], "consumer");
        assert_eq!(routed.rows[0]["provider_repo"], "p");
        assert_eq!(routed.rows[0]["provider_row"], "P-R7");
        assert_eq!(routed.rows[0]["provider_status"], "SPECIFIED");
        let provider = answer(SpecQuestion::SeamRouting, Some("requirement:p:P-R7"), &view).unwrap();
        assert_eq!(provider.rows[0]["direction"], "provider");

        let goals = answer(
            SpecQuestion::PlanToGoalPhase,
            Some("plansection:r:p.md#L3"),
            &view,
        )
        .unwrap();
        assert_eq!(goals.rows.len(), 2);
        assert_eq!(goals.rows[0]["goals"], "goal:g");
        assert_eq!(goals.rows[0]["phase"], "2");
        assert_eq!(goals.rows[1]["goals"], "");

        for (question, subject) in [
            (SpecQuestion::TaskContext, "task:none"),
            (SpecQuestion::PlanSection, "requirement:r:A-R1"),
            (SpecQuestion::Contract, "plansection:r:p.md#L3"),
            (SpecQuestion::SeamRouting, "contract:r:seam"),
            (SpecQuestion::PlanToGoalPhase, "goal:g"),
        ] {
            let refused = answer(question, Some(subject), &view).unwrap_err();
            assert_eq!(refused.code(), "COMPETENCY_SUBJECT_UNKNOWN", "{question:?}");
            let missing = answer(question, None, &view).unwrap_err();
            assert_eq!(missing.code(), "PARAMETER_INVALID", "{question:?}");
        }
    }
}
