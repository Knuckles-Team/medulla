//! `EVALUATE kernel [OF c [AGAINST c2]] [KEY value ...]` (MEDULLA-FEDERATED-QUERY-R726):
//! the report stage that runs one numeric evaluation kernel over the named value channels
//! of each series. Kernel names, channel arity and parameter keys are the one table
//! `medulla_query_types::series_expr::EVALUATE_KERNELS`; the printer is
//! `medulla_query_types::wire::uql_op`.

use medulla_query_types::wire::Op;

use super::Parser;
use crate::diag::UqlError;

impl Parser<'_> {
    /// `EVALUATE RULE ...` (MEDULLA-BUSINESS-OPS-R002.7) or `EVALUATE kernel ...`
    /// (MEDULLA-FEDERATED-QUERY-R726): the word after `EVALUATE` picks the form. `RULE` is
    /// not a kernel name, so the two never collide.
    pub(super) fn evaluate_stage(&mut self) -> Result<Op, UqlError> {
        if self.peek_kw("RULE") {
            self.bump();
            return self.evaluate_rule();
        }
        self.evaluate()
    }

    /// `EVALUATE RULE id [ASOF int] [COUNT int] [TEXT string [LOCALE string]]`: the clauses
    /// come in this order, each at most once; `LOCALE` needs a `TEXT`. A missing `ASOF` is
    /// not a parse error: the executor refuses it with `ASOF_REQUIRED`, so the same text
    /// answers the same typed refusal everywhere.
    pub(super) fn evaluate_rule(&mut self) -> Result<Op, UqlError> {
        use medulla_query_types::rule_expr::EvaluateRuleOp;

        let rule = self.id("a rule node id")?;
        let as_of_ms = if self.eat_kw("ASOF") {
            Some(self.parse_number::<i64>("an ASOF time in epoch milliseconds")?)
        } else {
            None
        };
        let count = if self.eat_kw("COUNT") {
            Some(self.parse_number::<u32>("a COUNT")?)
        } else {
            None
        };
        let (text, locale) = if self.eat_kw("TEXT") {
            let text = self.string("a TEXT string")?;
            let locale = if self.eat_kw("LOCALE") {
                Some(self.string("a LOCALE string")?)
            } else {
                None
            };
            (Some(text), locale)
        } else {
            (None, None)
        };
        Ok(Op::EvaluateRule {
            spec: EvaluateRuleOp { rule, as_of_ms, count, text, locale },
        })
    }

    gated! { "timeseries",
        /// `EVALUATE kernel [OF c [AGAINST c2]] {KEY number}`: a kernel outside the table
        /// is refused with `UQL_E_EVALUATE_KERNEL` at parse time; a parameter key is one
        /// the kernel lists, written at most once.
        fn evaluate(&mut self) -> Result<Op, UqlError> {
            use medulla_query_types::series_expr::{evaluate_kernel, EvaluateOp, EVALUATE_KERNELS};

            let span = self.cur_span();
            let word = self.name("an EVALUATE kernel")?;
            let Some(kernel) = evaluate_kernel(&word) else {
                return Err(UqlError::new(
                    medulla_types::contract::UqlCode::EvaluateKernel,
                    format!("`{word}` is not an EVALUATE kernel"),
                    span,
                )
                .expecting(EVALUATE_KERNELS.iter().map(|k| format!("`{}`", k.name)).collect()));
            };
            let mut channels = Vec::new();
            if kernel.channels >= 1 {
                self.expect_kw("OF")?;
                channels.push(self.name("a value channel")?);
            }
            if kernel.channels >= 2 {
                self.expect_kw("AGAINST")?;
                channels.push(self.name("a value channel")?);
            }
            let mut params: Vec<(String, f64)> = Vec::new();
            while let Some(key) = kernel.params.iter().find(|k| self.peek_kw(k)) {
                let at = self.cur_span().0;
                self.bump();
                if params.iter().any(|(seen, _)| seen == key) {
                    return Err(self.err_at(at, &format!("`{key}` is given twice")));
                }
                params.push(((*key).to_string(), self.number("a number")?));
            }
            Ok(Op::Evaluate {
                spec: EvaluateOp { kernel: kernel.name.to_string(), channels, params },
            })
        }
    }
}

#[cfg(all(test, feature = "timeseries"))]
mod tests {
    use medulla_query_types::series_expr::{EvaluateOp, EVALUATE_KERNELS};
    use medulla_query_types::wire::{uql_op, Op};

    use crate::parse;
    use medulla_types::contract::UqlCode;

    fn spelling(kernel: &str) -> String {
        let k = EVALUATE_KERNELS.iter().find(|k| k.name == kernel).unwrap();
        let mut text = format!("TSSCAN ['s'] FROM 0 TO 10 |> EVALUATE {kernel}");
        if k.channels >= 1 {
            text.push_str(" OF v0");
        }
        if k.channels >= 2 {
            text.push_str(" AGAINST v1");
        }
        for (i, key) in k.params.iter().enumerate() {
            text.push_str(&format!(" {key} {}", i + 2));
        }
        text
    }

    // proves R726: UQL EVALUATE grammar
    // spec: MEDULLA-FEDERATED-QUERY-R726
    #[test]
    fn parses_every_kernel_and_refuses_unknown() {
        for k in EVALUATE_KERNELS {
            let text = spelling(k.name);
            let plan = parse(&text).unwrap_or_else(|e| panic!("{text}: {}", e.render(&text)));
            let Some(Op::Evaluate { spec }) = plan.ops.last() else {
                panic!("{text}: last stage is not Evaluate");
            };
            assert_eq!(spec.kernel, k.name);
            assert_eq!(spec.channels.len(), k.channels);
            let keys: Vec<&str> = spec.params.iter().map(|(key, _)| key.as_str()).collect();
            assert_eq!(keys, k.params);
            // The printed stage parses back to the same op.
            let printed = uql_op(plan.ops.last().unwrap()).unwrap();
            let again = parse(&format!("TSSCAN ['s'] FROM 0 TO 10 |> {printed}")).unwrap();
            assert_eq!(again.ops.last(), plan.ops.last());
        }

        // Parameters are optional and may come in any order; a channel may be derived.
        let plan = parse("TSSCAN ['s'] FROM 0 TO 10 |> EVALUATE regime_hmm OF v0 TOL 0.5 STATES 3").unwrap();
        assert_eq!(
            plan.ops.last(),
            Some(&Op::Evaluate {
                spec: EvaluateOp {
                    kernel: "regime_hmm".into(),
                    channels: vec!["v0".into()],
                    params: vec![("TOL".into(), 0.5), ("STATES".into(), 3.0)],
                },
            })
        );

        let unknown = parse("TSSCAN ['s'] FROM 0 TO 10 |> EVALUATE no_such_kernel OF v0").unwrap_err();
        assert_eq!(unknown.code, UqlCode::EvaluateKernel);
        assert_eq!(unknown.code.as_str(), "UQL_E_EVALUATE_KERNEL");
        assert!(unknown.expected.iter().any(|e| e == "`brier`"));

        // A kernel missing its channels, or repeating a key, is a syntax refusal.
        assert!(parse("TSSCAN ['s'] FROM 0 TO 10 |> EVALUATE brier OF v0").is_err());
        assert!(parse("TSSCAN ['s'] FROM 0 TO 10 |> EVALUATE adf OF v0 MAXLAG 2 MAXLAG 3").is_err());
    }
}
