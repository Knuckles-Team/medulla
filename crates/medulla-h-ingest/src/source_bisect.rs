//! `BisectStep` (MEDULLA-SCM-R009): the chain window a bisect searches and the answer.
//!
//! The handler reads the window from the history index of the branch, the served caller reads
//! the recorded verdicts of the window from the graph, and `medulla_compute::source_bisect::next_step`
//! decides. Nothing is written.

use std::collections::BTreeMap;

use medulla_compute::source_bisect::next_step;
use medulla_compute::source_history::{decode_seq, id_key, seq_key, HistoryIndex};
use medulla_source_types::source_bisect::{BisectLink, BisectRefusal, BisectStepReceipt, BisectStepRequest};
use medulla_work_types::step_verdict::VerificationVerdict;

/// The most changesets one bisect window holds. A longer range refuses `INVALID_ARGUMENT`
/// naming a nearer `good`; with no `good` the window is the last `MAX_BISECT_WINDOW` changesets
/// below `bad`.
pub const MAX_BISECT_WINDOW: u64 = 4096;

/// The window of the chain a bisect searches: oldest first, ending at `bad`, plus the index of
/// `good` inside it when the request names one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BisectWindow {
    pub links: Vec<BisectLink>,
    pub good: Option<usize>,
}

fn seq_of(index: &dyn HistoryIndex, rev: &str) -> Result<Result<u64, BisectRefusal>, String> {
    match index.row(&seq_key(rev))? {
        Some(bytes) => Ok(Ok(decode_seq(&bytes)?)),
        None => Ok(Err(BisectRefusal::RevisionNotOnChain(format!(
            "{rev} is not a changeset of the branch"
        )))),
    }
}

/// Read the window of `request` from the history index.
pub fn read_window(
    index: &dyn HistoryIndex,
    request: &BisectStepRequest,
) -> Result<Result<BisectWindow, BisectRefusal>, String> {
    let bad_seq = match seq_of(index, &request.bad)? {
        Ok(seq) => seq,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let first = match request.good.as_deref() {
        Some(good) => match seq_of(index, good)? {
            Ok(seq) if seq < bad_seq => seq,
            Ok(_) => {
                return Ok(Err(BisectRefusal::InvalidArgument(
                    "good is not below bad on the chain".to_string(),
                )))
            }
            Err(refusal) => return Ok(Err(refusal)),
        },
        None => bad_seq.saturating_sub(MAX_BISECT_WINDOW).max(1),
    };
    if bad_seq - first + 1 > MAX_BISECT_WINDOW + 1 {
        return Ok(Err(BisectRefusal::InvalidArgument(format!(
            "the range holds more than {MAX_BISECT_WINDOW} changesets; name a nearer good"
        ))));
    }
    let mut links = Vec::new();
    for seq in first..=bad_seq {
        let Some(bytes) = index.row(&id_key(seq))? else {
            return Err(format!("history index has no commit row at seq {seq}"));
        };
        let commit = String::from_utf8(bytes).map_err(|error| error.to_string())?;
        let changeset = index
            .changeset_at(&commit)?
            .ok_or_else(|| format!("commit {commit} carries no changeset"))?;
        links.push(BisectLink {
            commit,
            changeset: changeset.id,
        });
    }
    let good = request.good.as_ref().map(|_| 0);
    Ok(Ok(BisectWindow { links, good }))
}

/// The next step over a window and the recorded verdicts of the bisected gate.
pub fn step(
    window: &BisectWindow,
    verdicts: &BTreeMap<String, VerificationVerdict>,
) -> Result<BisectStepReceipt, BisectRefusal> {
    next_step(&window.links, verdicts, window.good)
}
