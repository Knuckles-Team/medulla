//! The one differential oracle harness (MEDULLA-INCREMENTAL-VIEWS-R003).
//!
//! A subject applies a stream of steps and can report both its maintained
//! rows and a full recompute. The harness compares the two multisets after
//! every step and names the first step and row that differ.

use std::fmt;

/// Something whose incrementally maintained result must equal a recompute.
pub trait OracleSubject {
    type Step;

    /// Apply one step to the maintained state and to the recompute source.
    fn apply_step(&mut self, step: &Self::Step);

    /// The maintained multiset: `(row, weight)` pairs.
    fn maintained(&self) -> Vec<(String, i64)>;

    /// The multiset a full recompute produces.
    fn recomputed(&self) -> Vec<(String, i64)>;
}

/// The first divergence between maintained and recomputed rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleMismatch {
    /// Zero-based index of the step after which the multisets differ.
    pub step: usize,
    /// The first differing row with its maintained and recomputed weights.
    pub row: String,
    pub maintained: i64,
    pub recomputed: i64,
}

impl fmt::Display for OracleMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "step {}: row {:?} maintained weight {} but recompute weight {}",
            self.step, self.row, self.maintained, self.recomputed
        )
    }
}

impl std::error::Error for OracleMismatch {}

fn sorted(mut rows: Vec<(String, i64)>) -> Vec<(String, i64)> {
    rows.sort();
    rows
}

fn weight_of(rows: &[(String, i64)], row: &str) -> i64 {
    rows.iter().filter(|(r, _)| r == row).map(|(_, w)| w).sum()
}

fn first_difference(
    step: usize,
    maintained: &[(String, i64)],
    recomputed: &[(String, i64)],
) -> Option<OracleMismatch> {
    if maintained == recomputed {
        return None;
    }
    let row = maintained
        .iter()
        .chain(recomputed)
        .map(|(r, _)| r)
        .find(|r| weight_of(maintained, r) != weight_of(recomputed, r))
        .cloned()
        .unwrap_or_default();
    Some(OracleMismatch {
        step,
        maintained: weight_of(maintained, &row),
        recomputed: weight_of(recomputed, &row),
        row,
    })
}

/// Apply `steps` in order and compare the sorted maintained and recomputed
/// multisets after each one.
pub fn assert_incremental_equals_recompute<S: OracleSubject>(
    subject: &mut S,
    steps: &[S::Step],
) -> Result<(), OracleMismatch> {
    for (index, step) in steps.iter().enumerate() {
        subject.apply_step(step);
        let maintained = sorted(subject.maintained());
        let recomputed = sorted(subject.recomputed());
        if let Some(mismatch) = first_difference(index, &maintained, &recomputed) {
            return Err(mismatch);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Maintains a count per row but drops every second insert of `"b"`.
    struct Counter {
        log: Vec<String>,
        maintained: Vec<(String, i64)>,
    }

    impl OracleSubject for Counter {
        type Step = String;

        fn apply_step(&mut self, step: &String) {
            self.log.push(step.clone());
            let skip = step == "b" && self.log.iter().filter(|r| *r == "b").count() == 2;
            if skip {
                return;
            }
            match self.maintained.iter_mut().find(|(r, _)| r == step) {
                Some((_, w)) => *w += 1,
                None => self.maintained.push((step.clone(), 1)),
            }
        }

        fn maintained(&self) -> Vec<(String, i64)> {
            self.maintained.clone()
        }

        fn recomputed(&self) -> Vec<(String, i64)> {
            let mut out: Vec<(String, i64)> = Vec::new();
            for row in &self.log {
                match out.iter_mut().find(|(r, _)| r == row) {
                    Some((_, w)) => *w += 1,
                    None => out.push((row.clone(), 1)),
                }
            }
            out
        }
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R003
    #[test]
    fn mismatch_names_step_and_row() {
        let steps: Vec<String> = ["a", "b", "a", "b"].map(String::from).to_vec();
        let fresh = || Counter {
            log: Vec::new(),
            maintained: Vec::new(),
        };
        assert_eq!(
            assert_incremental_equals_recompute(&mut fresh(), &steps[..3]),
            Ok(())
        );
        let mismatch = assert_incremental_equals_recompute(&mut fresh(), &steps).unwrap_err();
        assert_eq!(
            mismatch,
            OracleMismatch {
                step: 3,
                row: "b".to_string(),
                maintained: 1,
                recomputed: 2,
            }
        );
        assert!(mismatch.to_string().starts_with("step 3: row \"b\""));
    }
}
