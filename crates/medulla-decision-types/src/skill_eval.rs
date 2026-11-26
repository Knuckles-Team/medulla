//! Skill evaluation types for the Learn stage (MEDULLA-DECISION-ENGINE-R711.1).
//!
//! A skill is evaluated against [`SkillEvalCase`] rows of three kinds: golden, real-prompt and
//! author-judged. Each case yields one [`SkillEvalResult`] with a separate outcome score and
//! trajectory score, and a clean-room [`EnvironmentAttestation`]. A [`SuiteReport`] sums one
//! skill's suite and names its band. No case, result or report holds prompt text: only digests.

use serde::{Deserialize, Serialize};

use medulla_types::refusal_code::RefusalCode;

/// The graph class of a case node (vocabulary R817).
pub const SKILL_EVAL_CASE_CLASS: &str = "SkillEvalCase";
/// Fewest annotated items a judge calibration needs.
pub const JUDGE_MIN_ANNOTATED: u64 = 50;

/// `JUDGE_NOT_CALIBRATED`: the judge agrees too little or was calibrated on too few items.
pub const JUDGE_NOT_CALIBRATED: RefusalCode = RefusalCode::new("JUDGE_NOT_CALIBRATED");
/// `EVAL_RUN_NOT_CLEAN_ROOM`: the run read a path of the host.
pub const EVAL_RUN_NOT_CLEAN_ROOM: RefusalCode = RefusalCode::new("EVAL_RUN_NOT_CLEAN_ROOM");
/// `EVAL_CASE_INVALID`: a case the kind or its mocks make unrunnable.
pub const EVAL_CASE_INVALID: RefusalCode = RefusalCode::new("EVAL_CASE_INVALID");

/// The `skill_eval` table of registered refusal codes.
pub const REFUSALS: &[RefusalCode] = &[JUDGE_NOT_CALIBRATED, EVAL_RUN_NOT_CLEAN_ROOM, EVAL_CASE_INVALID];

/// What kind of case it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EvalCaseKind {
    /// A fixed input with a fixed expected answer.
    Golden,
    /// A prompt a user really sent, with an expected trajectory.
    RealPrompt,
    /// A case a judge grades.
    AuthorJudged,
}

/// Where a case came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EvalCaseSource {
    /// Written by the skill's author.
    Authored,
    /// Taken from a recorded run trace.
    Trace,
    /// Made from a human correction (R702).
    Correction,
    /// Made from a review comment or rejection (R702.7).
    ReviewSignal,
}

/// One recorded tool call a case replays.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ToolMock {
    /// The tool.
    pub tool: String,
    /// The digest of the arguments.
    pub args_digest: String,
    /// The digest of the recorded result.
    pub result_digest: String,
}

/// Whether a trajectory step is a skill or a tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TrajectoryKind {
    /// A skill was used.
    Skill,
    /// A tool was called.
    Tool,
}

/// One step of an expected or an actual trajectory.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TrajectoryStep {
    /// Skill or tool.
    pub kind: TrajectoryKind,
    /// Its name.
    pub name: String,
}

/// One case of a skill's suite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillEvalCase {
    /// The case id.
    pub case_id: String,
    /// The skill revision the case tests: `skill:<name>@<revision>`.
    pub skill_ref: String,
    /// Golden, real-prompt or author-judged.
    pub kind: EvalCaseKind,
    /// The digest of the input; the input text is never stored.
    pub input_digest: String,
    /// The digest of the expected answer.
    #[serde(default)]
    pub expected_digest: Option<String>,
    /// The tool calls the run replays.
    #[serde(default)]
    pub tool_mocks: Vec<ToolMock>,
    /// The trajectory the run should follow.
    #[serde(default)]
    pub expected_trajectory: Vec<TrajectoryStep>,
    /// Where the case came from.
    pub source: EvalCaseSource,
}

/// What a clean-room run attests about its environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct EnvironmentAttestation {
    /// The digest of the sandbox image.
    pub sandbox_digest: String,
    /// The digest of the environment.
    pub env_digest: String,
    /// How many host paths the run read; a clean room reads none.
    pub host_paths_read: u64,
}

/// The result of one case.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillEvalResult {
    /// The case.
    pub case_id: String,
    /// How well the outcome matched, in ppm.
    pub outcome_score_ppm: u64,
    /// How well the trajectory matched, in ppm; absent when the case has no trajectory.
    #[serde(default)]
    pub trajectory_score_ppm: Option<u64>,
    /// Whether the case passed.
    pub passed: bool,
    /// The attestation of the run.
    pub attestation: EnvironmentAttestation,
}

/// Where a suite's pass rate stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SuiteBand {
    /// Between the low and high bounds.
    Healthy,
    /// Below the low bound.
    TooLow,
    /// Above the high bound: suspiciously saturated.
    Saturated,
}

/// The report of one skill's suite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SuiteReport {
    /// The share of golden cases, in ppm.
    pub golden_ppm: u64,
    /// The share of real-prompt cases, in ppm.
    pub real_ppm: u64,
    /// The share of author-judged cases, in ppm.
    pub judged_ppm: u64,
    /// The share of passed cases, in ppm.
    pub pass_rate_ppm: u64,
    /// The band of the pass rate.
    pub band: SuiteBand,
    /// How many cases the suite holds.
    pub case_count: u64,
}

/// How well a judge agrees with human annotation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct JudgeCalibration {
    /// The judge.
    pub judge_ref: String,
    /// The annotated set it was calibrated on.
    pub annotated_set_ref: String,
    /// Cohen's kappa, in ppm.
    pub kappa_ppm: u64,
    /// How many annotated items.
    pub n: u64,
}

/// What a harness observed when it ran one case in a clean room.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillEvalRun {
    /// The case that was run.
    pub case_id: String,
    /// The digest of the answer the skill gave.
    pub actual_digest: String,
    /// The steps the skill took.
    #[serde(default)]
    pub actual_trajectory: Vec<TrajectoryStep>,
    /// The verdict of the judge, for a judged case.
    #[serde(default)]
    pub judged_passed: Option<bool>,
    /// What the run attests about its environment.
    pub attestation: EnvironmentAttestation,
    /// The run trace node of the run, when it has one.
    #[serde(default)]
    pub run_trace_id: Option<String>,
}

/// The body of `DecisionEval` op `skill_eval`: one skill's suite and the runs of its cases.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillEvalRequest {
    /// The tenant that owns the suite.
    pub tenant_id: String,
    /// The skill revision under test.
    pub skill_ref: String,
    /// The cases of the suite; every case names `skill_ref`.
    pub cases: Vec<SkillEvalCase>,
    /// The runs of those cases.
    pub runs: Vec<SkillEvalRun>,
    /// The calibration of the judge that graded the judged cases.
    #[serde(default)]
    pub calibration: Option<JudgeCalibration>,
    /// The tools the skill may call; every mock names one of them.
    pub known_tools: Vec<String>,
}

/// One stored result with the case and the run trace it links to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredSkillEvalResult {
    /// The result.
    pub result: SkillEvalResult,
    /// The run trace the result was observed in.
    #[serde(default)]
    pub run_trace_id: Option<String>,
}

/// A case whose result was refused and not stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RefusedEvalCase {
    /// The case.
    pub case_id: String,
    /// The registered refusal code.
    pub code: String,
}

/// What the `skill_eval` job answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillEvalOutcome {
    /// The skill revision.
    pub skill_ref: String,
    /// The stored results.
    pub results: Vec<StoredSkillEvalResult>,
    /// The cases whose results were refused.
    pub refused: Vec<RefusedEvalCase>,
    /// The report of the stored results.
    pub report: SuiteReport,
    /// The premises of the sealed record (R711.5), in order.
    pub premises: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN_CASE: &str = r#"{"case_id":"c1","skill_ref":"skill:triage@3","kind":"real_prompt","input_digest":"i","expected_digest":null,"tool_mocks":[{"tool":"search","args_digest":"a","result_digest":"r"}],"expected_trajectory":[{"kind":"tool","name":"search"}],"source":"review_signal"}"#;

    // spec: MEDULLA-DECISION-ENGINE-R711.1
    #[test]
    fn eval_types_round_trip_with_golden_vector() {
        let case: SkillEvalCase = serde_json::from_str(GOLDEN_CASE).expect("decode");
        assert_eq!(case.kind, EvalCaseKind::RealPrompt);
        assert_eq!(case.source, EvalCaseSource::ReviewSignal);
        assert_eq!(serde_json::to_string(&case).expect("encode"), GOLDEN_CASE);
        let result = SkillEvalResult {
            case_id: "c1".into(),
            outcome_score_ppm: 1_000_000,
            trajectory_score_ppm: Some(500_000),
            passed: true,
            attestation: EnvironmentAttestation {
                sandbox_digest: "s".into(),
                env_digest: "e".into(),
                host_paths_read: 0,
            },
        };
        let text = serde_json::to_string(&result).expect("encode result");
        assert_eq!(serde_json::from_str::<SkillEvalResult>(&text).unwrap(), result);
        let report = SuiteReport {
            golden_ppm: 50_000,
            real_ppm: 800_000,
            judged_ppm: 150_000,
            pass_rate_ppm: 800_000,
            band: SuiteBand::Healthy,
            case_count: 20,
        };
        let text = serde_json::to_string(&report).expect("encode report");
        assert!(text.contains("\"band\":\"healthy\""));
        assert_eq!(serde_json::from_str::<SuiteReport>(&text).unwrap(), report);
        let unknown = GOLDEN_CASE.replace("\"case_id\":\"c1\"", "\"case_id\":\"c1\",\"x\":1");
        assert!(serde_json::from_str::<SkillEvalCase>(&unknown).is_err());
    }
}
