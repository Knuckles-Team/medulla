//! Skill-document step compiler (MEDULLA-REPO-INGEST-R002.1.2).
//!
//! Reads the ordered list under the `## Steps` heading of a skill document.
//! Each item reads `<step_id>: <instruction>`; indented lines read
//! `capability: <id>` and `after: <step_id>[, <step_id>]`. Prose outside the
//! section never becomes a step. The proposal passes the task-plan
//! canonicalizer, so the plan digest matches `TaskPlan`. Time is
//! O(steps + edges) apart from the planner's own ordering.

use medulla_decision_types::decision::task_plan::{
    plan_explicit_tasks, TaskPlanProposal, TaskPlanTask, MAX_CONCURRENCY,
};
use medulla_types::contract::{BoundedVec, Digest256, ServerErrorCode};
use medulla_types::skill_plan::{SkillCompilation, SkillStepBinding, SKILL_GRAMMAR_REVISION};

const STEPS_HEADING: &str = "## Steps";

/// The capability and type registries a compilation is resolved against.
pub trait CapabilityLookup {
    /// Whether the capability registry holds this capability id.
    fn knows(&self, capability_id: &str) -> bool;
    /// Digest of the capability registry the lookup reads.
    fn capability_digest(&self) -> Digest256;
    /// Digest of the type registry the lookup reads.
    fn type_digest(&self) -> Digest256;
}

/// One skill document and the directory it belongs to.
pub struct SkillDocument<'a> {
    pub directory_ref: &'a str,
    pub text: &'a str,
}

/// Why a skill document does not compile.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SkillPlanRefusal {
    #[error("a step has no step id")]
    StepIdMissing,
    #[error("step id {0} appears twice")]
    StepIdDuplicate(String),
    #[error("dependency {0} names no step")]
    DependencyUnresolved(String),
    #[error("the step dependencies form a cycle")]
    Cycle,
    #[error("capability {0} is not in the registry")]
    CapabilityUnresolved(String),
    #[error("unsupported step line: {0}")]
    TypeUnsupported(String),
    #[error("the document has no `## Steps` section with steps")]
    SpanMissing,
}

impl SkillPlanRefusal {
    /// The registered wire code of this refusal.
    pub fn code(&self) -> ServerErrorCode {
        match self {
            Self::StepIdMissing => ServerErrorCode::SkillStepIdMissing,
            Self::StepIdDuplicate(_) => ServerErrorCode::SkillStepIdDuplicate,
            Self::DependencyUnresolved(_) => ServerErrorCode::SkillDependencyUnresolved,
            Self::Cycle => ServerErrorCode::SkillCycle,
            Self::CapabilityUnresolved(_) => ServerErrorCode::SkillCapabilityUnresolved,
            Self::TypeUnsupported(_) => ServerErrorCode::SkillTypeUnsupported,
            Self::SpanMissing => ServerErrorCode::SkillSpanMissing,
        }
    }
}

struct ParsedStep<'a> {
    id: &'a str,
    instruction: &'a str,
    span: (usize, usize),
    capability: Option<&'a str>,
    after: Vec<&'a str>,
}

/// Lines of the `## Steps` section with their byte offsets.
fn section_lines(text: &str) -> Vec<(usize, &str)> {
    let mut offset = 0;
    let mut lines = Vec::new();
    let mut inside = false;
    for raw in text.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']);
        if inside && line.starts_with('#') {
            break;
        }
        if inside && !line.trim().is_empty() {
            lines.push((offset, line));
        }
        inside |= line.trim_end() == STEPS_HEADING;
        offset += raw.len();
    }
    lines
}

fn strip_marker(line: &str) -> &str {
    let line = line.trim_start_matches(['-', '*']).trim_start();
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    match line[digits..].strip_prefix('.') {
        Some(rest) if digits > 0 => rest.trim_start(),
        _ => line,
    }
}

fn open_step(offset: usize, line: &str) -> Result<ParsedStep<'_>, SkillPlanRefusal> {
    let (id, instruction) = strip_marker(line)
        .split_once(':')
        .ok_or(SkillPlanRefusal::StepIdMissing)?;
    let id = id.trim();
    if id.is_empty() {
        return Err(SkillPlanRefusal::StepIdMissing);
    }
    Ok(ParsedStep {
        id,
        instruction: instruction.trim(),
        span: (offset, offset + line.len()),
        capability: None,
        after: Vec::new(),
    })
}

fn attach_attribute<'a>(
    step: &mut ParsedStep<'a>,
    offset: usize,
    line: &'a str,
) -> Result<(), SkillPlanRefusal> {
    let unsupported = || SkillPlanRefusal::TypeUnsupported(line.trim().to_string());
    let (key, value) = line.trim().split_once(':').ok_or_else(unsupported)?;
    match key.trim() {
        "capability" => step.capability = Some(value.trim()),
        "after" => step
            .after
            .extend(value.split(',').map(str::trim).filter(|id| !id.is_empty())),
        _ => return Err(unsupported()),
    }
    step.span.1 = offset + line.len();
    Ok(())
}

fn parse_steps(text: &str) -> Result<Vec<ParsedStep<'_>>, SkillPlanRefusal> {
    let mut steps: Vec<ParsedStep> = Vec::new();
    for (offset, line) in section_lines(text) {
        if line.starts_with(char::is_whitespace) {
            let step = steps
                .last_mut()
                .ok_or_else(|| SkillPlanRefusal::TypeUnsupported(line.trim().to_string()))?;
            attach_attribute(step, offset, line)?;
        } else {
            steps.push(open_step(offset, line)?);
        }
    }
    if steps.is_empty() {
        return Err(SkillPlanRefusal::SpanMissing);
    }
    Ok(steps)
}

fn check_capabilities(
    steps: &[ParsedStep<'_>],
    lookup: &dyn CapabilityLookup,
) -> Result<(), SkillPlanRefusal> {
    match steps
        .iter()
        .filter_map(|step| step.capability)
        .find(|id| !lookup.knows(id))
    {
        Some(id) => Err(SkillPlanRefusal::CapabilityUnresolved(id.to_string())),
        None => Ok(()),
    }
}

fn task_of(step: &ParsedStep<'_>) -> Result<TaskPlanTask, SkillPlanRefusal> {
    let after = step.after.iter().map(|id| id.to_string()).collect();
    Ok(TaskPlanTask {
        task_id: step.id.to_string(),
        prompt: step.instruction.to_string(),
        depends_on: BoundedVec::new(after)
            .map_err(|_| SkillPlanRefusal::TypeUnsupported("more than 64 dependencies".into()))?,
        call: None,
        aggregation: None,
        assignee: step.capability.map(str::to_string),
        branch: None,
        join: None,
        decision: None,
        approval: None,
    })
}

fn proposal_of(steps: &[ParsedStep<'_>]) -> Result<TaskPlanProposal, SkillPlanRefusal> {
    let tasks = steps.iter().map(task_of).collect::<Result<Vec<_>, _>>()?;
    let count = u16::try_from(steps.len()).unwrap_or(u16::MAX);
    Ok(TaskPlanProposal {
        tasks: BoundedVec::new(tasks)
            .map_err(|_| SkillPlanRefusal::TypeUnsupported("more than 64 steps".to_string()))?,
        output_task_id: steps[steps.len() - 1].id.to_string(),
        max_concurrency: count.min(MAX_CONCURRENCY),
        max_steps: count,
    })
}

/// Planner failure codes and the refusal each one is.
fn plan_refusal(message: &str) -> SkillPlanRefusal {
    let code = message.split(':').next().unwrap_or_default();
    let detail = message.split_once(": ").map_or("", |(_, rest)| rest);
    match code {
        "plan_cycle" => SkillPlanRefusal::Cycle,
        "missing_dependency" | "disconnected_output" => {
            SkillPlanRefusal::DependencyUnresolved(detail.to_string())
        }
        "duplicate_task_id" => SkillPlanRefusal::StepIdDuplicate(detail.to_string()),
        "invalid_task_id" => SkillPlanRefusal::StepIdMissing,
        _ => SkillPlanRefusal::TypeUnsupported(message.to_string()),
    }
}

fn bind(step: &ParsedStep<'_>) -> SkillStepBinding {
    SkillStepBinding {
        step_id: step.id.to_string(),
        span_start: step.span.0 as u64,
        span_end: step.span.1 as u64,
        instruction_digest: Digest256::sha256(step.instruction.as_bytes()),
        capability_id: step.capability.map(str::to_string),
    }
}

/// Compile a skill document into a task-plan proposal and its compilation record.
pub fn compile_skill(
    doc: &SkillDocument<'_>,
    lookup: &dyn CapabilityLookup,
) -> Result<(TaskPlanProposal, SkillCompilation), SkillPlanRefusal> {
    let steps = parse_steps(doc.text)?;
    check_capabilities(&steps, lookup)?;
    let proposal = proposal_of(&steps)?;
    let plan = plan_explicit_tasks(&proposal).map_err(|message| plan_refusal(&message))?;
    let compilation = SkillCompilation {
        directory_ref: doc.directory_ref.to_string(),
        document_digest: Digest256::sha256(doc.text.as_bytes()),
        grammar_revision: SKILL_GRAMMAR_REVISION.to_string(),
        plan_digest: Digest256::sha256(plan.plan_digest.as_bytes()),
        capability_digest: lookup.capability_digest(),
        type_digest: lookup.type_digest(),
        steps: steps.iter().map(bind).collect(),
    };
    Ok((proposal, compilation))
}
