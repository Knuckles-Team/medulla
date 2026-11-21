//! `TRAVERSE` and `PROPAGATE`: edge patterns (direction, relationship, edge predicates),
//! hop ranges and the impact-model clause.

use medulla_query_types::wire::{EdgeDir, LayerWeight, Op, PropagateModel, RiskHistorySpec};

use super::Parser;
use crate::diag::{UqlCode, UqlError};
use crate::lexer::Tok;

/// `PROPAGATE` without `HOPS`.
const PROPAGATE_DEFAULT_HOPS: usize = 8;
/// The most layers a `RISK HISTORY` tail may name (MEDULLA-FEDERATED-QUERY-R032.100).
const RISK_HISTORY_MAX_LAYERS: usize = 16;
/// `CASCADE` without `SAMPLES`.
const PROPAGATE_DEFAULT_SAMPLES: u32 = 2_000;

impl<'a> Parser<'a> {
    /// `TRAVERSE edge [hops]`.
    pub(super) fn traverse(&mut self) -> Result<Op, UqlError> {
        let (dir, rel, edge_preds) = self.edge_pattern()?;
        let (min, max) = self.hop_range()?;
        Ok(match (dir, rel, edge_preds.is_empty()) {
            (EdgeDir::Out, Some(rel), true) => Op::Traverse { rel, min, max },
            (dir, rel, _) => Op::Expand {
                rel,
                dir,
                min,
                max,
                edge_preds,
            },
        })
    }

    /// `PROPAGATE model edge [HOPS n] [DEFAULT p]` (EH-526).
    pub(super) fn propagate(&mut self) -> Result<Op, UqlError> {
        let model = self.propagate_model()?;
        let (dir, rel, edge_preds) = self.edge_pattern()?;
        let hops = if self.eat_kw("HOPS") {
            self.parse_number::<usize>("a hop bound")?
        } else {
            PROPAGATE_DEFAULT_HOPS
        };
        let default_transmission = if self.eat_kw("DEFAULT") {
            self.transmission()?
        } else {
            1.0
        };
        let risk_history = if self.eat_kw("RISK") {
            Some(self.risk_history()?)
        } else {
            None
        };
        Ok(Op::Propagate {
            model,
            rel,
            dir,
            edge_preds,
            hops,
            default_transmission,
            risk_history,
        })
    }

    /// `HISTORY 'dataset' WINDOW from TO to LAYERS ('layer' weight {, 'layer' weight})`
    /// after `RISK` (MEDULLA-FEDERATED-QUERY-R032.100).
    fn risk_history(&mut self) -> Result<RiskHistorySpec, UqlError> {
        let start = self.prev_span();
        self.expect_kw("HISTORY")?;
        let dataset = self.string("a history dataset id")?;
        self.expect_kw("WINDOW")?;
        let from_ms = self.parse_number::<i64>("a window start (Unix ms)")?;
        self.expect_kw("TO")?;
        let to_ms = self.parse_number::<i64>("a window end (Unix ms)")?;
        self.expect_kw("LAYERS")?;
        self.expect(&Tok::LParen, "`(` to open the layer list")?;
        let mut layers: Vec<LayerWeight> = Vec::new();
        loop {
            let at = self.cur_span();
            let layer = self.string("a layer name")?;
            let weight = self.number("a layer weight")?;
            let refuse = |msg: String| {
                UqlError::new(UqlCode::RiskHistory, msg, (at.0, self.prev_span().1))
            };
            if !(0.0..=1.0).contains(&weight) {
                return Err(refuse(format!(
                    "layer `{layer}` weight must be in [0, 1]"
                )));
            }
            if layers.iter().any(|known| known.layer == layer) {
                return Err(refuse(format!("layer `{layer}` is listed twice")));
            }
            layers.push(LayerWeight { layer, weight });
            if layers.len() > RISK_HISTORY_MAX_LAYERS {
                return Err(refuse(format!(
                    "RISK HISTORY names at most {RISK_HISTORY_MAX_LAYERS} layers"
                )));
            }
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen, "`)` to close the layer list")?;
        if to_ms <= from_ms {
            return Err(UqlError::new(
                UqlCode::RiskHistory,
                "the RISK HISTORY window must end after it starts",
                (start.0, self.prev_span().1),
            ));
        }
        Ok(RiskHistorySpec {
            dataset,
            from_ms,
            to_ms,
            layers,
        })
    }

    /// `NOISY_OR` | `CASCADE [SAMPLES n] [SEED s]`.
    fn propagate_model(&mut self) -> Result<PropagateModel, UqlError> {
        if self.eat_kw("NOISY_OR") {
            return Ok(PropagateModel::NoisyOr);
        }
        if !self.eat_kw("CASCADE") {
            return Err(self.err_here("expected `NOISY_OR` or `CASCADE` after PROPAGATE"));
        }
        let samples = if self.eat_kw("SAMPLES") {
            self.parse_number::<u32>("a sample count")?
        } else {
            PROPAGATE_DEFAULT_SAMPLES
        };
        let seed = if self.eat_kw("SEED") {
            self.parse_number::<u64>("a seed")?
        } else {
            0
        };
        Ok(PropagateModel::Cascade { samples, seed })
    }

    /// A default transmission probability in `[0, 1]`.
    fn transmission(&mut self) -> Result<f64, UqlError> {
        let start = self.cur_span();
        let p = self.number("a transmission probability")?;
        if !(0.0..=1.0).contains(&p) {
            return Err(UqlError::new(
                UqlCode::InvalidRange,
                "DEFAULT transmission must be in [0, 1]",
                (start.0, self.prev_span().1),
            ));
        }
        Ok(p)
    }

    /// `-[rel]->` | `-[rel]-` | `<-[rel]-` | bare `name` (outgoing).
    fn edge_pattern(
        &mut self,
    ) -> Result<
        (
            EdgeDir,
            Option<String>,
            Vec<medulla_query_types::wire::Pred>,
        ),
        UqlError,
    > {
        let incoming = self.eat(&Tok::LArrow);
        if !incoming && !self.eat(&Tok::Dash) {
            let rel = self.name("a relationship type (or `-[:REL]->`)")?;
            return Ok((EdgeDir::Out, Some(rel), Vec::new()));
        }
        self.expect(&Tok::LBracket, "`[` in the edge pattern")?;
        let rel = if self.eat(&Tok::Star) {
            None
        } else {
            self.expect(&Tok::Colon, "`:REL` or `*` in the edge pattern")?;
            Some(self.name("a relationship type")?)
        };
        let preds = if self.eat_kw("WHERE") {
            self.pred_list()?
        } else {
            Vec::new()
        };
        self.expect(&Tok::RBracket, "`]` to close the edge pattern")?;
        let dir = self.edge_close(incoming)?;
        Ok((dir, rel, preds))
    }

    /// The closing `->` / `-` of an edge pattern.
    fn edge_close(&mut self, incoming: bool) -> Result<EdgeDir, UqlError> {
        if incoming {
            self.expect(&Tok::Dash, "`-` to close `<-[…]-`")?;
            return Ok(EdgeDir::In);
        }
        if self.eat(&Tok::Arrow) {
            return Ok(EdgeDir::Out);
        }
        self.expect(&Tok::Dash, "`->` (outgoing) or `-` (either direction)")?;
        Ok(EdgeDir::Both)
    }

    /// `hops = { int [ (, | ..) int ] }`; absent ⇒ exactly one hop.
    fn hop_range(&mut self) -> Result<(usize, usize), UqlError> {
        if !self.eat(&Tok::LBrace) {
            return Ok((1, 1));
        }
        let start = self.prev_start();
        let min = self.parse_number::<usize>("a minimum hop count")?;
        let max = if self.eat(&Tok::Comma) || self.eat(&Tok::DotDot) {
            self.parse_number::<usize>("a maximum hop count")?
        } else {
            min
        };
        self.expect(&Tok::RBrace, "`}` to close the hop range")?;
        if max < min {
            return Err(UqlError::new(
                UqlCode::InvalidRange,
                "hop range max must be ≥ min",
                (start, self.prev_span().1),
            ));
        }
        Ok((min, max))
    }
}

#[cfg(test)]
mod tests {
    use medulla_query_types::wire::{uql_op, LayerWeight, Op};

    use crate::{parse, UqlCode};

    const HISTORY: &str = "RISK HISTORY 'deploys' WINDOW 1700000000000 TO 1702592000000 \
                           LAYERS ('service' 1, 'os' 0.5, 'hardware' 0.25)";

    fn text(tail: &str) -> String {
        format!("MATCH (:Service) |> PROPAGATE NOISY_OR -[:dependsOn]- HOPS 4 {tail}")
    }

    // spec: MEDULLA-FEDERATED-QUERY-R032.100
    #[test]
    fn risk_history_tail_parses_prints_and_refuses() {
        let plan = parse(&text(HISTORY)).unwrap();
        let Some(Op::Propagate { risk_history, .. }) = plan.ops.last() else {
            panic!("a PROPAGATE stage");
        };
        let spec = risk_history.clone().expect("the tail parsed");
        assert_eq!(spec.dataset, "deploys");
        assert_eq!((spec.from_ms, spec.to_ms), (1_700_000_000_000, 1_702_592_000_000));
        assert_eq!(
            spec.layers,
            vec![
                LayerWeight { layer: "service".into(), weight: 1.0 },
                LayerWeight { layer: "os".into(), weight: 0.5 },
                LayerWeight { layer: "hardware".into(), weight: 0.25 },
            ]
        );

        // The printer spells the tail and the text parses back to the same plan.
        let printed = plan.to_uql().unwrap();
        assert!(printed.contains("RISK HISTORY 'deploys' WINDOW"), "{printed}");
        assert_eq!(parse(&printed).unwrap(), plan);
        assert!(uql_op(plan.ops.last().unwrap()).unwrap().contains("LAYERS ('service' 1"));

        // Without the tail the stage is unchanged.
        let plain = parse(&text("")).unwrap();
        let Some(Op::Propagate { risk_history, .. }) = plain.ops.last() else {
            panic!("a PROPAGATE stage");
        };
        assert!(risk_history.is_none());

        // Each refusal carries UQL_E_RISK_HISTORY.
        let sixteen = (0..17)
            .map(|i| format!("'l{i}' 0.5"))
            .collect::<Vec<_>>()
            .join(", ");
        for tail in [
            "RISK HISTORY 'd' WINDOW 0 TO 10 LAYERS ('a' 1.5)".to_string(),
            "RISK HISTORY 'd' WINDOW 0 TO 10 LAYERS ('a' -0.1)".to_string(),
            "RISK HISTORY 'd' WINDOW 0 TO 10 LAYERS ('a' 1, 'a' 0.5)".to_string(),
            "RISK HISTORY 'd' WINDOW 10 TO 10 LAYERS ('a' 1)".to_string(),
            "RISK HISTORY 'd' WINDOW 10 TO 5 LAYERS ('a' 1)".to_string(),
            format!("RISK HISTORY 'd' WINDOW 0 TO 10 LAYERS ({sixteen})"),
        ] {
            let error = parse(&text(&tail)).unwrap_err();
            assert_eq!(error.code, UqlCode::RiskHistory, "{tail}");
            assert_eq!(error.code.as_str(), "UQL_E_RISK_HISTORY");
        }
    }
}
