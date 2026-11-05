//! One differential oracle harness for incremental views
//! (MEDULLA-INCREMENTAL-VIEWS-R003).
//!
//! An [`OracleSubject`] maintains a result incrementally and can also recompute it from
//! scratch. [`assert_incremental_equals_recompute`] feeds it steps and, after every step,
//! compares the two result multisets (each sorted first). A mismatch names the step index
//! and the first differing row. The plan oracle and the SQL oracle share this one loop.

use std::cmp::Ordering;

/// A system under differential test.
pub trait OracleSubject {
    /// One input step (a mutation, a commit, a delta batch).
    type Step;
    /// Apply one step to the maintained state and to whatever the recompute reads.
    fn apply_step(&mut self, step: &Self::Step);
    /// The incrementally maintained result as `(row key, weight)` pairs.
    fn maintained(&self) -> Vec<(String, i64)>;
    /// The from-scratch result as `(row key, weight)` pairs.
    fn recomputed(&self) -> Vec<(String, i64)>;
}

/// The first disagreement between the maintained and the recomputed result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OracleMismatch {
    /// Zero-based index of the step after which the results differed.
    pub step: usize,
    /// First differing row (sorted order). `None` on a side means the row is absent there.
    pub row: String,
    pub maintained: Option<i64>,
    pub recomputed: Option<i64>,
}

impl std::fmt::Display for OracleMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "incremental result diverged from recompute after step {}: row `{}` maintained={:?} recomputed={:?}",
            self.step, self.row, self.maintained, self.recomputed
        )
    }
}

impl std::error::Error for OracleMismatch {}

/// Apply `steps` in order and compare the maintained and recomputed multisets after each.
pub fn assert_incremental_equals_recompute<S: OracleSubject>(
    subject: &mut S,
    steps: &[S::Step],
) -> Result<(), OracleMismatch> {
    for (step, s) in steps.iter().enumerate() {
        subject.apply_step(s);
        let mut left = subject.maintained();
        let mut right = subject.recomputed();
        left.sort();
        right.sort();
        if let Some(m) = first_difference(step, &left, &right) {
            return Err(m);
        }
    }
    Ok(())
}

fn first_difference(
    step: usize,
    left: &[(String, i64)],
    right: &[(String, i64)],
) -> Option<OracleMismatch> {
    let (mut i, mut j) = (0, 0);
    while i < left.len() || j < right.len() {
        let order = match (left.get(i), right.get(j)) {
            (Some(l), Some(r)) => l.cmp(r),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => break,
        };
        match order {
            Ordering::Equal => {
                i += 1;
                j += 1;
            }
            Ordering::Less => {
                let l = &left[i];
                return Some(OracleMismatch {
                    step,
                    row: l.0.clone(),
                    maintained: Some(l.1),
                    recomputed: right.iter().find(|r| r.0 == l.0).map(|r| r.1),
                });
            }
            Ordering::Greater => {
                let r = &right[j];
                return Some(OracleMismatch {
                    step,
                    row: r.0.clone(),
                    maintained: left.iter().find(|l| l.0 == r.0).map(|l| l.1),
                    recomputed: Some(r.1),
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Maintains a counter per key; `broken_at` makes the maintained side drift.
    struct Counter {
        truth: Vec<String>,
        maintained: Vec<(String, i64)>,
        applied: usize,
        broken_at: Option<usize>,
    }

    impl OracleSubject for Counter {
        type Step = String;
        fn apply_step(&mut self, step: &String) {
            self.truth.push(step.clone());
            if self.broken_at == Some(self.applied) {
                self.maintained.push(("drift".into(), 1));
            } else if let Some(e) = self.maintained.iter_mut().find(|e| e.0 == *step) {
                e.1 += 1;
            } else {
                self.maintained.push((step.clone(), 1));
            }
            self.applied += 1;
        }
        fn maintained(&self) -> Vec<(String, i64)> {
            self.maintained.clone()
        }
        fn recomputed(&self) -> Vec<(String, i64)> {
            let mut out: Vec<(String, i64)> = Vec::new();
            for k in &self.truth {
                match out.iter_mut().find(|e| e.0 == *k) {
                    Some(e) => e.1 += 1,
                    None => out.push((k.clone(), 1)),
                }
            }
            out
        }
    }

    fn steps(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|s| s.to_string()).collect()
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R003
    #[test]
    fn mismatch_names_step_and_row() {
        let mut ok = Counter {
            truth: vec![],
            maintained: vec![],
            applied: 0,
            broken_at: None,
        };
        assert_eq!(
            assert_incremental_equals_recompute(&mut ok, &steps(&["b", "a", "b"])),
            Ok(())
        );

        let mut bad = Counter {
            truth: vec![],
            maintained: vec![],
            applied: 0,
            broken_at: Some(2),
        };
        let err = assert_incremental_equals_recompute(&mut bad, &steps(&["a", "b", "c", "d"]))
            .unwrap_err();
        assert_eq!(err.step, 2);
        // Sorted order puts `c` (recompute only) before `drift` (maintained only).
        assert_eq!(err.row, "c");
        assert_eq!((err.maintained, err.recomputed), (None, Some(1)));
        assert!(err.to_string().contains("step 2"));
    }
}
