//! Agent goals (MEDULLA-DECISION-ENGINE-R901): the durable, tenant-keyed record of a goal an agent
//! pursues, the iterations that pursued it, and the explicit state machine that moves it.
//!
//! A goal is created queued. `run` moves it to running and opens an iteration (a numbered attempt
//! with its own run id); `finish` records how the run ended; `cancel` stops a queued or running
//! goal. [`GOAL_MACHINE`] is the one edge table: [`GoalState::apply`] is the only function that
//! produces a new state, and a refused event is the typed `TRANSITION_REFUSED`. The engine starts
//! no harness: it records that the run began and hands back the run id the launcher uses.
//!
//! Every clock value is the `now_ms` the handler passes in (the engine clock). A goal of another
//! tenant answers `GOAL_NOT_FOUND`, the same as an absent one, so no read is an existence oracle.
//!
//! Pure data and decisions; the served handler reads the stored nodes the caller can see, calls
//! [`handle_goal_control`], [`handle_goal_list`] or [`handle_goal_iterations`] and writes the nodes.

use medulla_types::refusal_code::RefusalCode;
use medulla_types::tenant_cursor::CursorFamily;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::lifecycle::sha256_hex;

/// The graph `type` of a goal node.
pub const AGENT_GOAL_TYPE: &str = "AgentGoal";
/// The graph `type` of a goal iteration node.
pub const GOAL_ITERATION_TYPE: &str = "AgentGoalIteration";

/// No goal of the verified tenant has this id. An absent goal and a foreign one answer alike.
pub const GOAL_NOT_FOUND: &str = "GOAL_NOT_FOUND";
/// A request field is out of bounds, a goal id is taken by another title, or a cursor is unusable.
pub const GOAL_INVALID: &str = "GOAL_INVALID";

/// The stable refusal codes of the goal methods (a refused transition is `TRANSITION_REFUSED`).
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(GOAL_NOT_FOUND),
    RefusalCode::new(GOAL_INVALID),
];

/// Longest goal id, in bytes.
pub const MAX_GOAL_ID_BYTES: usize = 128;
/// Longest title, in bytes.
pub const MAX_GOAL_TITLE_BYTES: usize = 256;
/// Most iterations one goal records.
pub const MAX_GOAL_ITERATIONS: u32 = 1000;
/// Most goals one `GoalList` page carries, and the page size when none is asked for.
pub const MAX_GOAL_LIST_LIMIT: u32 = 100;
/// Longest opaque list cursor a caller may hand back.
pub const MAX_GOAL_CURSOR_BYTES: usize = 1024;
/// Domain-separation prefix of the `GoalList` cursor family.
pub const GOAL_CURSOR_DOMAIN: &[u8] = b"medulla/agent-goal-list-cursor/v1";

const LIST_CURSOR: CursorFamily = CursorFamily {
    domain: GOAL_CURSOR_DOMAIN,
    max_bytes: MAX_GOAL_CURSOR_BYTES,
    noun: "goal list",
};

/// A typed refusal of a goal method.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GoalRefusal {
    /// A field is out of bounds, an id is taken, or a cursor is not usable.
    #[error("GOAL_INVALID: {0}")]
    Invalid(String),
    /// No goal of the verified tenant has the id.
    #[error("GOAL_NOT_FOUND: {0}")]
    NotFound(String),
    /// The goal machine refuses the event in the goal's current state.
    #[error("{0}")]
    Transition(TransitionRefused),
}

impl GoalRefusal {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => GOAL_INVALID,
            Self::NotFound(_) => GOAL_NOT_FOUND,
            Self::Transition(_) => medulla_types::transition_refused::TRANSITION_REFUSED,
        }
    }

    /// The one refusal of a read or a control that cannot find its goal.
    pub fn not_found() -> Self {
        Self::NotFound("no goal with this id".to_owned())
    }
}

/// The state of one goal: the closed state set of `AgentGoalMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum GoalState {
    /// Created and waiting for a run.
    Queued,
    /// A run is pursuing the goal.
    Running,
    /// The last run reached the goal.
    Succeeded,
    /// The last run failed; a new run may retry.
    Failed,
    /// The goal was stopped.
    Cancelled,
}

/// The closed event set of `AgentGoalMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GoalEvent {
    /// A run starts.
    Run,
    /// The run reached the goal.
    Succeed,
    /// The run failed.
    Fail,
    /// The goal is stopped.
    Cancel,
}

impl MachineState for GoalState {
    fn stored(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl MachineEvent for GoalEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Run => "Run",
            Self::Succeed => "Succeed",
            Self::Fail => "Fail",
            Self::Cancel => "Cancel",
        }
    }
}

/// `AgentGoalMachine`: the closed edge table. Every other pair of state and event is refused.
pub static GOAL_MACHINE: MachineDef<GoalState, GoalEvent, &'static str> = MachineDef {
    name: "AgentGoalMachine",
    states: &[
        GoalState::Queued,
        GoalState::Running,
        GoalState::Succeeded,
        GoalState::Failed,
        GoalState::Cancelled,
    ],
    initial: GoalState::Queued,
    terminals: &[GoalState::Succeeded, GoalState::Cancelled],
    events: &[
        GoalEvent::Run,
        GoalEvent::Succeed,
        GoalEvent::Fail,
        GoalEvent::Cancel,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "G01",
            from: GoalState::Queued,
            event: GoalEvent::Run,
            guard: None,
            to: GoalState::Running,
        },
        Edge {
            id: "G02",
            from: GoalState::Running,
            event: GoalEvent::Succeed,
            guard: None,
            to: GoalState::Succeeded,
        },
        Edge {
            id: "G03",
            from: GoalState::Running,
            event: GoalEvent::Fail,
            guard: None,
            to: GoalState::Failed,
        },
        Edge {
            id: "G04",
            from: GoalState::Queued,
            event: GoalEvent::Cancel,
            guard: None,
            to: GoalState::Cancelled,
        },
        Edge {
            id: "G05",
            from: GoalState::Running,
            event: GoalEvent::Cancel,
            guard: None,
            to: GoalState::Cancelled,
        },
        Edge {
            id: "G06",
            from: GoalState::Failed,
            event: GoalEvent::Run,
            guard: None,
            to: GoalState::Running,
        },
    ],
};

impl GoalState {
    /// The one transition function of `AgentGoalMachine`.
    pub fn apply(self, event: GoalEvent) -> Result<Self, TransitionRefused> {
        transition(&GOAL_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The state whose stored text is `stored`.
    pub fn from_stored(stored: &str) -> Option<Self> {
        GOAL_MACHINE.state_from_stored(stored)
    }
}

/// How one iteration stands: running until its run ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum IterationOutcome {
    /// The run is still going.
    Running,
    /// The run reached the goal.
    Succeeded,
    /// The run failed.
    Failed,
    /// The run was cancelled.
    Cancelled,
}

impl IterationOutcome {
    /// Parse the stored wire name.
    pub fn parse(name: &str) -> Option<Self> {
        serde_json::from_value(Value::String(name.to_owned())).ok()
    }

    /// The stored wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One goal of a tenant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AgentGoal {
    /// The caller-chosen id of the goal.
    pub goal_id: String,
    /// The verified tenant that owns the goal.
    pub tenant_id: String,
    /// What the goal is.
    pub title: String,
    /// The run of the latest iteration, once a run has started.
    #[serde(default)]
    pub run_id: Option<String>,
    /// Where the goal stands.
    pub state: GoalState,
    /// When the goal was created, by the engine clock, in milliseconds.
    pub created_ms: u64,
}

/// One numbered attempt at a goal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct GoalIteration {
    /// The goal the iteration belongs to.
    pub goal_id: String,
    /// The iteration number, from 1.
    pub iteration: u32,
    /// The run that pursued the goal in this iteration.
    pub run_id: String,
    /// How the iteration stands.
    pub outcome: IterationOutcome,
    /// When the iteration started, by the engine clock, in milliseconds.
    pub started_ms: u64,
    /// When the iteration ended, by the engine clock; absent while it runs.
    #[serde(default)]
    pub ended_ms: Option<u64>,
}

/// How a run ended, as the launcher reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum GoalFinish {
    /// The run reached the goal.
    Succeeded,
    /// The run failed.
    Failed,
}

/// One control operation on a goal. Every one needs the scope `runs:control`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalControlOp {
    /// Create a queued goal. The same id and title again replays it.
    Create {
        /// The id the caller gives the goal.
        goal_id: String,
        /// What the goal is.
        title: String,
    },
    /// Start a run: queued or failed to running, a new iteration, a new run id.
    Run {
        /// The goal to run.
        goal_id: String,
    },
    /// Stop a queued or running goal.
    Cancel {
        /// The goal to stop.
        goal_id: String,
    },
    /// Record how the running run ended.
    Finish {
        /// The goal whose run ended.
        goal_id: String,
        /// How it ended.
        outcome: GoalFinish,
    },
}

impl GoalControlOp {
    /// The wire name of the operation.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::Run { .. } => "run",
            Self::Cancel { .. } => "cancel",
            Self::Finish { .. } => "finish",
        }
    }

    /// The goal id the operation names.
    pub fn goal_id(&self) -> &str {
        match self {
            Self::Create { goal_id, .. }
            | Self::Run { goal_id }
            | Self::Cancel { goal_id }
            | Self::Finish { goal_id, .. } => goal_id,
        }
    }
}

/// What a goal control answers: the goal's state and, when a run exists, its run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct GoalControlReceipt {
    /// The goal the control applied to.
    pub goal_id: String,
    /// The state the goal is in now.
    pub state: GoalState,
    /// The run of the latest iteration, when a run has started.
    #[serde(default)]
    pub run_id: Option<String>,
    /// The number of the latest iteration, when a run has started.
    #[serde(default)]
    pub iteration: Option<u32>,
    /// True when the goal already existed with this title and nothing was written.
    pub replayed: bool,
}

/// One page of goals, newest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct GoalPage {
    /// The goals of this page.
    pub goals: Vec<AgentGoal>,
    /// The opaque cursor of the next page, absent on the last page.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// The iterations of one goal, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct GoalIterations {
    /// The iterations, by number.
    pub iterations: Vec<GoalIteration>,
}

/// A goal as the graph stores it: the wire record and the count of iterations opened so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredGoal {
    /// The goal record.
    pub goal: AgentGoal,
    /// How many iterations the goal has opened.
    pub iterations: u32,
}

/// The node property holding the count of iterations a goal has opened.
pub const GOAL_ITERATIONS_PROPERTY: &str = "iterations";

impl StoredGoal {
    /// Read the goal a graph node holds. `None` when the node is not a goal of `tenant` or holds
    /// a value the goal model does not allow.
    pub fn from_node(node: &Value, tenant: &str) -> Option<Self> {
        if node.get("type").and_then(Value::as_str) != Some(AGENT_GOAL_TYPE)
            || node.get("tenant_id").and_then(Value::as_str) != Some(tenant)
        {
            return None;
        }
        let text = |key: &str| node.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Self {
            goal: AgentGoal {
                goal_id: text("goal_id")?,
                tenant_id: tenant.to_owned(),
                title: text("title")?,
                run_id: text("run_id"),
                state: GoalState::from_stored(&text("state")?)?,
                created_ms: node.get("created_ms")?.as_u64()?,
            },
            iterations: u32::try_from(node.get(GOAL_ITERATIONS_PROPERTY)?.as_u64()?).ok()?,
        })
    }
}

impl GoalIteration {
    /// Read the iteration a graph node holds. `None` when the node is not an iteration of
    /// `tenant` or holds a value the model does not allow.
    pub fn from_node(node: &Value, tenant: &str) -> Option<Self> {
        if node.get("type").and_then(Value::as_str) != Some(GOAL_ITERATION_TYPE)
            || node.get("tenant_id").and_then(Value::as_str) != Some(tenant)
        {
            return None;
        }
        let text = |key: &str| node.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Self {
            goal_id: text("goal_id")?,
            iteration: u32::try_from(node.get("iteration")?.as_u64()?).ok()?,
            run_id: text("run_id")?,
            outcome: IterationOutcome::parse(&text("outcome")?)?,
            started_ms: node.get("started_ms")?.as_u64()?,
            ended_ms: node.get("ended_ms").and_then(Value::as_u64),
        })
    }
}

fn tenant_key(tenant: &str) -> String {
    sha256_hex(tenant.as_bytes())[..16].to_owned()
}

/// The graph node id of `goal_id` in `tenant`: the tenant is part of the key as a short digest,
/// so no tenant name appears in a node id.
pub fn goal_node_id(tenant: &str, goal_id: &str) -> String {
    format!("agent_goal:{}:{goal_id}", tenant_key(tenant))
}

/// The graph node id of iteration `iteration` of `goal_id` in `tenant`.
pub fn iteration_node_id(tenant: &str, goal_id: &str, iteration: u32) -> String {
    format!(
        "agent_goal_iteration:{}:{goal_id}:{iteration}",
        tenant_key(tenant)
    )
}

/// The run id of iteration `iteration` of `goal_id` in `tenant`: derived, so a retried request
/// names the same run, and no tenant or goal name appears in it.
pub fn run_id_of(tenant: &str, goal_id: &str, iteration: u32) -> String {
    let material = format!("{tenant}\u{1f}{goal_id}\u{1f}{iteration}");
    format!("run:{}", &sha256_hex(material.as_bytes())[..24])
}

fn invalid<T>(what: &str) -> Result<T, GoalRefusal> {
    Err(GoalRefusal::Invalid(format!("{what} is out of bounds")))
}

fn goal_id_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_GOAL_ID_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn title_ok(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_GOAL_TITLE_BYTES
        && !value.chars().any(char::is_control)
}

/// Check the bounds of a goal id.
pub fn validate_goal_id(goal_id: &str) -> Result<(), GoalRefusal> {
    if goal_id_ok(goal_id) {
        Ok(())
    } else {
        invalid("goal_id")
    }
}

/// What [`handle_goal_control`] decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalWrite {
    /// The goal after the control.
    pub goal: AgentGoal,
    /// How many iterations the goal has opened after the control.
    pub iterations: u32,
    /// The iteration to write (opened or closed by this control), when there is one.
    pub iteration: Option<GoalIteration>,
    /// True when the goal already existed with this title, so nothing is written.
    pub replayed: bool,
}

impl GoalWrite {
    /// The receipt of this decision.
    pub fn receipt(&self) -> GoalControlReceipt {
        let latest = (self.iterations > 0).then_some(self.iterations);
        GoalControlReceipt {
            goal_id: self.goal.goal_id.clone(),
            state: self.goal.state,
            run_id: self.goal.run_id.clone(),
            iteration: latest,
            replayed: self.replayed,
        }
    }
}

/// `GoalControl`: decide what one control writes, or refuse before anything is written.
///
/// `stored` is the goal already held for the id of `tenant`; `open_iteration` is the goal's latest
/// iteration when it is still running. `now_ms` is the engine clock.
pub fn handle_goal_control(
    tenant: &str,
    stored: Option<StoredGoal>,
    open_iteration: Option<GoalIteration>,
    op: GoalControlOp,
    now_ms: u64,
) -> Result<GoalWrite, GoalRefusal> {
    validate_goal_id(op.goal_id())?;
    if tenant.trim().is_empty() {
        return invalid("tenant");
    }
    if let GoalControlOp::Create { goal_id, title } = op {
        if !title_ok(&title) {
            return invalid("title");
        }
        return match stored {
            Some(held) if held.goal.title == title => Ok(GoalWrite {
                goal: held.goal,
                iterations: held.iterations,
                iteration: None,
                replayed: true,
            }),
            Some(_) => Err(GoalRefusal::Invalid(format!("goal id {goal_id} is taken"))),
            None => Ok(GoalWrite {
                goal: AgentGoal {
                    goal_id,
                    tenant_id: tenant.to_owned(),
                    title,
                    run_id: None,
                    state: GOAL_MACHINE.initial,
                    created_ms: now_ms,
                },
                iterations: 0,
                iteration: None,
                replayed: false,
            }),
        };
    }
    let held = stored.ok_or_else(GoalRefusal::not_found)?;
    let (event, outcome) = match &op {
        GoalControlOp::Run { .. } => (GoalEvent::Run, IterationOutcome::Running),
        GoalControlOp::Cancel { .. } => (GoalEvent::Cancel, IterationOutcome::Cancelled),
        GoalControlOp::Finish {
            outcome: GoalFinish::Succeeded,
            ..
        } => (GoalEvent::Succeed, IterationOutcome::Succeeded),
        GoalControlOp::Finish {
            outcome: GoalFinish::Failed,
            ..
        } => (GoalEvent::Fail, IterationOutcome::Failed),
        GoalControlOp::Create { .. } => return invalid("operation"),
    };
    let next = held
        .goal
        .state
        .apply(event)
        .map_err(GoalRefusal::Transition)?;
    let mut goal = held.goal;
    goal.state = next;
    if event == GoalEvent::Run {
        let number = held.iterations + 1;
        if number > MAX_GOAL_ITERATIONS {
            return invalid("iterations (the goal has used them all)");
        }
        let run_id = run_id_of(tenant, &goal.goal_id, number);
        goal.run_id = Some(run_id.clone());
        return Ok(GoalWrite {
            iteration: Some(GoalIteration {
                goal_id: goal.goal_id.clone(),
                iteration: number,
                run_id,
                outcome,
                started_ms: now_ms,
                ended_ms: None,
            }),
            goal,
            iterations: number,
            replayed: false,
        });
    }
    // The goal was running or queued: a running goal closes its open iteration.
    let iteration = open_iteration
        .filter(|open| open.outcome == IterationOutcome::Running)
        .map(|open| GoalIteration {
            outcome,
            ended_ms: Some(now_ms),
            ..open
        });
    Ok(GoalWrite {
        goal,
        iterations: held.iterations,
        iteration,
        replayed: false,
    })
}

/// The resume key of a goal in the newest-first order: the inverted creation time, then the id.
fn resume_key(goal: &AgentGoal) -> String {
    format!("{:020}|{}", u64::MAX - goal.created_ms, goal.goal_id)
}

/// `GoalList`: one page of `goals` (every goal of `tenant`), newest first, resuming after
/// `cursor`. `limit` is 1 to [`MAX_GOAL_LIST_LIMIT`] (the maximum when absent). A cursor that is
/// malformed or was minted for another tenant refuses `GOAL_INVALID`.
pub fn handle_goal_list(
    mut goals: Vec<AgentGoal>,
    tenant: &str,
    limit: Option<u32>,
    cursor: Option<&str>,
) -> Result<GoalPage, GoalRefusal> {
    let limit = match limit {
        None => MAX_GOAL_LIST_LIMIT as usize,
        Some(0) => return invalid("limit"),
        Some(asked) if asked > MAX_GOAL_LIST_LIMIT => return invalid("limit"),
        Some(asked) => asked as usize,
    };
    let after = cursor
        .map(|cursor| {
            LIST_CURSOR
                .decode(tenant, cursor, |key| !key.trim().is_empty())
                .map_err(GoalRefusal::Invalid)
        })
        .transpose()?;
    goals.sort_by_cached_key(resume_key);
    let rest: Vec<AgentGoal> = goals
        .into_iter()
        .filter(|goal| after.as_ref().is_none_or(|key| resume_key(goal) > *key))
        .collect();
    let more = rest.len() > limit;
    let page: Vec<AgentGoal> = rest.into_iter().take(limit).collect();
    let next_cursor = if more {
        page.last()
            .map(|last| LIST_CURSOR.encode(tenant, &resume_key(last)))
    } else {
        None
    };
    Ok(GoalPage {
        goals: page,
        next_cursor,
    })
}

/// `GoalIterations`: the iterations of one goal, oldest first, or the one not-found refusal when
/// the goal is absent, foreign or invisible.
pub fn handle_goal_iterations(
    stored: Option<StoredGoal>,
    mut iterations: Vec<GoalIteration>,
) -> Result<GoalIterations, GoalRefusal> {
    stored.ok_or_else(GoalRefusal::not_found)?;
    iterations.sort_by_key(|iteration| iteration.iteration);
    Ok(GoalIterations { iterations })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create(id: &str) -> GoalControlOp {
        GoalControlOp::Create {
            goal_id: id.to_owned(),
            title: "ship the release".to_owned(),
        }
    }

    fn held(write: &GoalWrite) -> StoredGoal {
        StoredGoal {
            goal: write.goal.clone(),
            iterations: write.iterations,
        }
    }

    fn control(
        stored: &GoalWrite,
        open: Option<GoalIteration>,
        op: GoalControlOp,
        now: u64,
    ) -> Result<GoalWrite, GoalRefusal> {
        handle_goal_control("acme", Some(held(stored)), open, op, now)
    }

    // spec: MEDULLA-DECISION-ENGINE-R901
    #[test]
    fn a_goal_runs_finishes_fails_and_retries_through_the_machine() {
        let made = handle_goal_control("acme", None, None, create("g1"), 100).expect("created");
        assert_eq!(made.goal.state, GoalState::Queued);
        assert_eq!(made.goal.created_ms, 100);
        assert!(!made.replayed && made.iteration.is_none());

        let run = |stored: &GoalWrite, now| {
            control(
                stored,
                None,
                GoalControlOp::Run {
                    goal_id: "g1".into(),
                },
                now,
            )
        };
        let first = run(&made, 200).expect("run");
        assert_eq!(first.goal.state, GoalState::Running);
        let one = first.iteration.clone().expect("an iteration opens");
        assert_eq!(
            (one.iteration, one.outcome, one.started_ms),
            (1, IterationOutcome::Running, 200)
        );
        assert_eq!(first.goal.run_id.as_deref(), Some(one.run_id.as_str()));
        assert_eq!(first.receipt().iteration, Some(1));

        // A running goal cannot run again, and a failed run closes its iteration.
        assert!(matches!(run(&first, 250), Err(GoalRefusal::Transition(_))));
        let failed = control(
            &first,
            Some(one.clone()),
            GoalControlOp::Finish {
                goal_id: "g1".into(),
                outcome: GoalFinish::Failed,
            },
            300,
        )
        .expect("finish");
        assert_eq!(failed.goal.state, GoalState::Failed);
        let closed = failed.iteration.clone().expect("the open iteration closes");
        assert_eq!(
            (closed.outcome, closed.ended_ms),
            (IterationOutcome::Failed, Some(300))
        );

        // A failed goal retries: iteration 2 with a new run id.
        let second = run(&failed, 400).expect("retry");
        let two = second.iteration.clone().expect("iteration");
        assert_eq!(two.iteration, 2);
        assert_ne!(two.run_id, one.run_id);
        let done = control(
            &second,
            Some(two),
            GoalControlOp::Finish {
                goal_id: "g1".into(),
                outcome: GoalFinish::Succeeded,
            },
            500,
        )
        .expect("finish");
        assert_eq!(done.goal.state, GoalState::Succeeded);
        // A succeeded goal is terminal: nothing moves it.
        let refused = run(&done, 600).expect_err("terminal");
        assert_eq!(refused.code(), "TRANSITION_REFUSED");
    }

    // spec: MEDULLA-DECISION-ENGINE-R901
    #[test]
    fn cancel_stops_a_queued_or_running_goal_and_closes_the_open_iteration() {
        let made = handle_goal_control("acme", None, None, create("g1"), 1).expect("created");
        let cancel = GoalControlOp::Cancel {
            goal_id: "g1".into(),
        };
        let queued = control(&made, None, cancel.clone(), 2).expect("cancel a queued goal");
        assert_eq!(queued.goal.state, GoalState::Cancelled);
        assert!(queued.iteration.is_none());
        assert!(
            control(&queued, None, cancel.clone(), 3).is_err(),
            "cancelled is terminal"
        );

        let running = control(
            &made,
            None,
            GoalControlOp::Run {
                goal_id: "g1".into(),
            },
            5,
        )
        .expect("run");
        let open = running.iteration.clone();
        let stopped = control(&running, open, cancel, 9).expect("cancel a running goal");
        let closed = stopped.iteration.expect("the iteration closes");
        assert_eq!(
            (closed.outcome, closed.ended_ms),
            (IterationOutcome::Cancelled, Some(9))
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R901
    #[test]
    fn create_replays_the_same_title_refuses_another_and_every_other_control_needs_the_goal() {
        let made = handle_goal_control("acme", None, None, create("g1"), 1).expect("created");
        let again =
            handle_goal_control("acme", Some(held(&made)), None, create("g1"), 99).expect("replay");
        assert!(again.replayed);
        assert_eq!(again.goal.created_ms, 1, "the first time stays");
        let other = GoalControlOp::Create {
            goal_id: "g1".into(),
            title: "something else".into(),
        };
        let taken =
            handle_goal_control("acme", Some(held(&made)), None, other, 99).expect_err("taken");
        assert_eq!(taken.code(), GOAL_INVALID);
        for op in [
            GoalControlOp::Run {
                goal_id: "ghost".into(),
            },
            GoalControlOp::Cancel {
                goal_id: "ghost".into(),
            },
            GoalControlOp::Finish {
                goal_id: "ghost".into(),
                outcome: GoalFinish::Failed,
            },
        ] {
            let refused = handle_goal_control("acme", None, None, op, 5).expect_err("absent goal");
            assert_eq!(refused, GoalRefusal::not_found());
        }
        let too_long = "x".repeat(129);
        for bad in ["", "has space", "a:b", too_long.as_str()] {
            assert_eq!(
                handle_goal_control("acme", None, None, create(bad), 1)
                    .expect_err("bad id")
                    .code(),
                GOAL_INVALID,
                "{bad:?}"
            );
        }
    }

    fn goal(id: &str, created: u64) -> AgentGoal {
        AgentGoal {
            goal_id: id.to_owned(),
            tenant_id: "acme".to_owned(),
            title: "t".to_owned(),
            run_id: None,
            state: GoalState::Queued,
            created_ms: created,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R901
    #[test]
    fn list_pages_newest_first_with_a_tenant_bound_cursor() {
        let goals = vec![goal("a", 1), goal("b", 3), goal("c", 2)];
        let first = handle_goal_list(goals.clone(), "acme", Some(2), None).expect("page");
        assert_eq!(
            first
                .goals
                .iter()
                .map(|g| g.goal_id.as_str())
                .collect::<Vec<_>>(),
            ["b", "c"]
        );
        let cursor = first.next_cursor.expect("more remain");
        let rest = handle_goal_list(goals.clone(), "acme", Some(2), Some(&cursor)).expect("page");
        assert_eq!(rest.goals.len(), 1);
        assert!(rest.next_cursor.is_none());
        let foreign =
            handle_goal_list(goals.clone(), "globex", None, Some(&cursor)).expect_err("foreign");
        assert_eq!(foreign.code(), GOAL_INVALID);
        for limit in [0, MAX_GOAL_LIST_LIMIT + 1] {
            assert!(handle_goal_list(goals.clone(), "acme", Some(limit), None).is_err());
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R901
    #[test]
    fn iterations_read_oldest_first_and_an_absent_goal_is_not_found() {
        let made = handle_goal_control("acme", None, None, create("g1"), 1).expect("created");
        let mk = |n| GoalIteration {
            goal_id: "g1".into(),
            iteration: n,
            run_id: run_id_of("acme", "g1", n),
            outcome: IterationOutcome::Failed,
            started_ms: u64::from(n),
            ended_ms: Some(u64::from(n) + 1),
        };
        let read = handle_goal_iterations(Some(held(&made)), vec![mk(2), mk(1)]).expect("read");
        assert_eq!(
            read.iterations
                .iter()
                .map(|i| i.iteration)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(
            handle_goal_iterations(None, vec![mk(1)]),
            Err(GoalRefusal::not_found())
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R901
    #[test]
    fn a_node_reads_as_a_goal_only_for_its_tenant_and_ids_hide_the_tenant() {
        let node = json!({
            "type": "AgentGoal", "tenant_id": "acme", "goal_id": "g1", "title": "t",
            "state": "running", "run_id": "run:1", "created_ms": 7, "iterations": 2,
        });
        let read = StoredGoal::from_node(&node, "acme").expect("a goal");
        assert_eq!((read.goal.state, read.iterations), (GoalState::Running, 2));
        assert!(StoredGoal::from_node(&node, "globex").is_none());
        let mut broken = node.clone();
        broken["state"] = json!("paused");
        assert!(StoredGoal::from_node(&broken, "acme").is_none());
        assert!(!goal_node_id("acme", "g1").contains("acme"));
        assert!(!iteration_node_id("acme", "g1", 1).contains("acme"));
        assert_ne!(goal_node_id("acme", "g1"), goal_node_id("globex", "g1"));
        assert_ne!(run_id_of("acme", "g1", 1), run_id_of("acme", "g1", 2));
        assert_eq!(run_id_of("acme", "g1", 1), run_id_of("acme", "g1", 1));
    }

    // spec: MEDULLA-DECISION-ENGINE-R901
    #[test]
    fn every_legal_edge_is_taken_and_every_other_pair_is_refused() {
        use medulla_types::transition_refused::TransitionRefusalReason as Reason;
        for &state in GOAL_MACHINE.states {
            for &event in GOAL_MACHINE.events {
                let edge = GOAL_MACHINE
                    .edges
                    .iter()
                    .find(|edge| edge.from == state && edge.event == event);
                match (edge, state.apply(event)) {
                    (Some(edge), result) => assert_eq!(result, Ok(edge.to)),
                    (None, Err(refused)) => {
                        assert_eq!(refused.machine, "AgentGoalMachine");
                        assert_eq!(refused.from, state.stored());
                        assert_eq!(refused.event, event.name());
                        let terminal = GOAL_MACHINE.is_terminal(state);
                        assert_eq!(
                            refused.reason,
                            if terminal {
                                Reason::TerminalState
                            } else {
                                Reason::IllegalTransition
                            }
                        );
                    }
                    (None, Ok(next)) => {
                        panic!("{state:?} {event:?} must be refused, took {next:?}")
                    }
                }
            }
        }
    }
}
