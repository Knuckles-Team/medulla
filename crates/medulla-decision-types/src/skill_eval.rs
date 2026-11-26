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

/// `EVAL_SCOPE_REQUIRED`: the caller lacks the scope the evaluation op needs.
pub const EVAL_SCOPE_REQUIRED: RefusalCode = RefusalCode::new("EVAL_SCOPE_REQUIRED");

/// The scope that adds an evaluation case (MEDULLA-IDENTITY-R736).
pub const SCOPE_AUTHOR: &str = "skills:author";
/// The scope that reads a suite report and records a judge calibration (MEDULLA-IDENTITY-R736).
pub const SCOPE_EVAL: &str = "skills:eval";

/// The `skill_eval` table of registered refusal codes.
pub const REFUSALS: &[RefusalCode] = &[
    JUDGE_NOT_CALIBRATED,
    EVAL_RUN_NOT_CLEAN_ROOM,
    EVAL_CASE_INVALID,
    EVAL_SCOPE_REQUIRED,
];

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

/// The body of `DecisionEval` op `skill_eval_case_add`: one case added to a skill's suite
/// (MEDULLA-DECISION-ENGINE-R711.7). Scope `skills:author`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillEvalCaseAddRequest {
    /// The tenant that owns the suite.
    pub tenant_id: String,
    /// The case to add; its `skill_ref` names the suite.
    pub case: SkillEvalCase,
    /// The tools the skill may call; every mock of the case names one of them.
    pub known_tools: Vec<String>,
}

/// What `skill_eval_case_add` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillEvalCaseAdded {
    /// The case id the caller sent.
    pub case_id: String,
    /// The skill revision the case tests.
    pub skill_ref: String,
    /// The graph node the case is stored as.
    pub node_id: String,
}

/// The body of `DecisionEval` op `skill_eval_report`: read the latest suite report of one skill
/// (MEDULLA-DECISION-ENGINE-R711.7). Scope `skills:eval`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillEvalReportRequest {
    /// The tenant that owns the suite.
    pub tenant_id: String,
    /// The skill revision.
    pub skill_ref: String,
}

/// What `skill_eval_report` answers: both fields are absent for a skill the tenant has no suite
/// for, whether the skill exists or not, so the answer is no existence oracle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillEvalReportAnswer {
    /// The skill revision asked about.
    pub skill_ref: String,
    /// The latest suite report, when the tenant has one.
    pub report: Option<SuiteReport>,
    /// The judge calibration on record, when the tenant has one.
    pub calibration: Option<JudgeCalibration>,
}

/// The body of `DecisionEval` op `skill_eval_calibrate`: record how well a judge agrees with
/// human annotation for one skill's suite (MEDULLA-DECISION-ENGINE-R711.7). Scope `skills:eval`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillEvalCalibrateRequest {
    /// The tenant that owns the suite.
    pub tenant_id: String,
    /// The skill revision whose judged cases the judge grades.
    pub skill_ref: String,
    /// The calibration to record.
    pub calibration: JudgeCalibration,
}

/// What `skill_eval_calibrate` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillEvalCalibrated {
    /// The skill revision.
    pub skill_ref: String,
    /// The calibration now on record.
    pub calibration: JudgeCalibration,
    /// Whether the judge clears the kappa bound and the annotated count, so a judged case runs.
    pub admitted: bool,
}

/// Which side of an ablation one run belongs to (MEDULLA-DECISION-ENGINE-R712.1).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AblationArm {
    /// The skill is loaded.
    With,
    /// The skill is not loaded.
    Without,
    /// The skill is loaded without one named part.
    WithoutPart {
        /// The part left out.
        part: String,
    },
}

/// One run of an ablation. The arm stays with the engine: a judge reads only [`JudgeInput`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AblationRun {
    /// The skill revision under test.
    pub skill_ref: String,
    /// The digest of the prompt both arms ran; no prompt text is stored.
    pub prompt_digest: String,
    /// The arm of the run.
    pub arm: AblationArm,
    /// The id a judge sees in place of the arm.
    pub blind_id: String,
    /// How well the run did, in ppm.
    pub outcome_score_ppm: u64,
    /// The seed of the run; a pair shares prompt and seed.
    pub seed: u64,
}

/// What a judge reads about one run: the blind id and the digests, never the arm.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct JudgeInput {
    /// The blind id of the run.
    pub blind_id: String,
    /// The digest of the prompt.
    pub prompt_digest: String,
    /// The digest of the output to grade.
    pub output_digest: String,
}

impl AblationRun {
    /// The input a judge is handed for this run: the arm is not part of it.
    pub fn judge_input(&self, output_digest: &str) -> JudgeInput {
        JudgeInput {
            blind_id: self.blind_id.clone(),
            prompt_digest: self.prompt_digest.clone(),
            output_digest: output_digest.to_string(),
        }
    }
}

/// How much two identical runs differ by chance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct NoiseBand {
    /// How many identical runs the band was measured on.
    pub identical_runs: u64,
    /// Twice their standard deviation, in ppm.
    pub band_ppm: u64,
}

/// What an ablation shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AblationVerdict {
    /// The arms differ by more than the noise band.
    Effect {
        /// The mean of the first arm minus the mean of the second, in ppm.
        delta_ppm: i64,
    },
    /// The arms differ by no more than the noise band.
    WithinNoise,
}

/// A challenger revision taking a share of the runs of one skill.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillCanary {
    /// The skill.
    pub skill_ref: String,
    /// The revision that serves the rest of the runs.
    pub champion_revision: String,
    /// The revision that serves the canary share.
    pub challenger_revision: String,
    /// The share of runs the challenger takes, in ppm.
    pub traffic_ppm: u64,
    /// How long the canary window lasts, in milliseconds.
    pub window_ms: u64,
}

/// What a canary decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CanaryVerdict {
    /// Raise the challenger's share.
    Ramp,
    /// Keep the share where it is.
    Hold,
    /// Take the challenger off.
    Rollback,
}

impl CanaryVerdict {
    /// The text a premise names the verdict by.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ramp => "ramp",
            Self::Hold => "hold",
            Self::Rollback => "rollback",
        }
    }
}

/// Whether a compiled program still matches its model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CompileState {
    /// The program was compiled against the current model revision.
    Current,
    /// The model revision changed after the program was compiled.
    Stale,
}

/// One program a skill was compiled into for one model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ModelCompile {
    /// The skill.
    pub skill_ref: String,
    /// The model the program was compiled for.
    pub model_ref: String,
    /// The digest of the compiled program.
    pub compiled_digest: String,
    /// Current or stale.
    pub state: CompileState,
}

/// The body of `DecisionLog` `learn` write `record_ablation`: the runs of one skill's ablation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AblationRecord {
    /// The skill revision under test.
    pub skill_ref: String,
    /// The score of each identical run, in ppm: one prompt, one arm, many seeds.
    pub identical_scores_ppm: Vec<u64>,
    /// The paired runs: a `With` run and a `Without` run per `(prompt_digest, seed)`.
    pub runs: Vec<AblationRun>,
}

/// What `record_ablation` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AblationRecorded {
    /// The noise band of the identical runs.
    pub noise: NoiseBand,
    /// With against without, judged against the band.
    pub verdict: AblationVerdict,
    /// The paired runs as rollout evidence; its receipt digest names the stored rollout set.
    pub evidence: crate::decision::held_out::HeldOutEvidence,
}

/// The bounds of knob `canary.traffic_ppm`.
pub const CANARY_TRAFFIC_MIN_PPM: u64 = 1_000;
/// The upper bound of knob `canary.traffic_ppm`.
pub const CANARY_TRAFFIC_MAX_PPM: u64 = 500_000;

/// What `record_canary` answers: the canary now in force, or none after a rollback.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CanaryRecorded {
    /// The canary split of the skill after the write.
    pub canary: Option<SkillCanary>,
}

/// What `record_model_compile` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ModelCompileRecorded {
    /// The row that was recorded, current.
    pub compile: ModelCompile,
    /// The rows a change of the model revision made stale.
    pub stale: Vec<ModelCompile>,
    /// The draft revisions filed for the stale rows, one per row.
    pub proposals: Vec<String>,
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

    // spec: MEDULLA-DECISION-ENGINE-R712.1
    #[test]
    fn ablation_types_round_trip_and_hide_the_arm() {
        let run = AblationRun {
            skill_ref: "skill:triage@3".into(),
            prompt_digest: "p1".into(),
            arm: AblationArm::WithoutPart { part: "intro".into() },
            blind_id: "b-17".into(),
            outcome_score_ppm: 700_000,
            seed: 4,
        };
        let wire = serde_json::to_string(&run).expect("encodes");
        assert_eq!(serde_json::from_str::<AblationRun>(&wire).expect("decodes"), run);
        for arm in [AblationArm::With, AblationArm::Without] {
            let text = serde_json::to_string(&arm).expect("encodes");
            assert_eq!(serde_json::from_str::<AblationArm>(&text).expect("decodes"), arm);
        }
        // A judge reads the blind id and the digests; no field or text of its input names the arm.
        let input = serde_json::to_string(&run.judge_input("out-1")).expect("encodes");
        assert!(input.contains("b-17") && input.contains("p1") && input.contains("out-1"));
        assert!(!input.contains("arm") && !input.contains("without") && !input.contains("intro"));
        for verdict in [AblationVerdict::Effect { delta_ppm: -5 }, AblationVerdict::WithinNoise] {
            let text = serde_json::to_string(&verdict).expect("encodes");
            assert_eq!(
                serde_json::from_str::<AblationVerdict>(&text).expect("decodes"),
                verdict
            );
        }
        let compile = ModelCompile {
            skill_ref: "skill:triage@3".into(),
            model_ref: "model:m1".into(),
            compiled_digest: "d".into(),
            state: CompileState::Stale,
        };
        let text = serde_json::to_string(&compile).expect("encodes");
        assert!(text.contains("\"stale\""));
        assert_eq!(serde_json::from_str::<ModelCompile>(&text).expect("decodes"), compile);
    }
}
