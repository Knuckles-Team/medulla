// CONCEPT:MEDULLA-KG.domains.finance-compute.golden-fixture — the typed shape a
// finance golden fixture's figure must carry (MEDULLA-FINANCE-PRIMITIVES-R013.1).
//
// Finance golden fixtures (broker statements, corporate actions,
// daylight-saving/session boundaries, leveraged-ETF decay, futures rolls)
// compare a computed figure against a published reference value. Every
// displayed figure must carry its source, as-of time, and session -- this
// module defines that figure shape and refuses one missing provenance,
// before any fixture comparison runs. Money is a fixed-point `i64` tick,
// scaled by `SCALE` (matching `lot_accounting::SCALE`, MEDULLA-FINANCE-PRIMITIVES-
// R005.1): never `f64`, so a replayed comparison is byte-identical across
// hosts.

use std::fmt;

/// Fixed-point scale: one whole currency/quantity unit is `SCALE` ticks.
pub const SCALE: i64 = 100_000_000;

/// A displayed finance figure with full provenance
/// (MEDULLA-FINANCE-PRIMITIVES-R013).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoldenFixtureFigure {
    /// Fixed-point value in ticks; never a floating-point money value.
    pub value_ticks: i64,
    /// Where the figure came from (a broker statement id, corporate-action
    /// notice id, published reference table, etc.). Never empty.
    pub source: String,
    /// The as-of time the figure is valid for, as an opaque ISO-8601-ish
    /// string (bitemporal AS OF, not a wall-clock read time). Never empty.
    pub as_of: String,
    /// The exchange session the figure was computed in (e.g. a session id
    /// or "regular"/"pre-market"/"after-hours"). Never empty.
    pub session: String,
}

/// Why a golden-fixture figure was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FigureProvenanceRefusal {
    pub reason: String,
}

impl fmt::Display for FigureProvenanceRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "golden-fixture figure refused: {}", self.reason)
    }
}

impl std::error::Error for FigureProvenanceRefusal {}

/// The decimal string of `ticks` at [`SCALE`], the wire form of money: eight fraction digits and
/// a leading `-` for a negative value (MEDULLA-FINANCE-PRIMITIVES-R005.3.4).
pub fn ticks_to_decimal(ticks: i64) -> String {
    let magnitude = ticks.unsigned_abs();
    let scale = SCALE as u64;
    format!(
        "{}{}.{:08}",
        if ticks < 0 { "-" } else { "" },
        magnitude / scale,
        magnitude % scale
    )
}

/// Admit `figure` only when every provenance field is present
/// (MEDULLA-FINANCE-PRIMITIVES-R013): a figure missing its source, as-of time, or
/// session is refused rather than displayed with a silent gap.
pub fn check_figure_provenance(
    figure: &GoldenFixtureFigure,
) -> Result<(), FigureProvenanceRefusal> {
    if figure.source.is_empty() {
        return Err(FigureProvenanceRefusal {
            reason: "figure.source is empty".to_string(),
        });
    }
    if figure.as_of.is_empty() {
        return Err(FigureProvenanceRefusal {
            reason: "figure.as_of is empty".to_string(),
        });
    }
    if figure.session.is_empty() {
        return Err(FigureProvenanceRefusal {
            reason: "figure.session is empty".to_string(),
        });
    }
    Ok(())
}

/// Why a golden-fixture comparison was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FigureComparisonRefusal {
    pub reason: String,
}

/// Compare a computed broker-statement figure against its published
/// reference value (MEDULLA-FINANCE-PRIMITIVES-R013.2.1): refuse `computed` when
/// its provenance is incomplete (`check_figure_provenance`), or when its
/// fixed-point `value_ticks` does not equal `published_reference_ticks`.
/// Both are `i64` ticks, never `f64`, so the comparison is byte-identical
/// across replays and hosts.
pub fn compare_broker_statement_figure(
    computed: &GoldenFixtureFigure,
    published_reference_ticks: i64,
) -> Result<(), FigureComparisonRefusal> {
    check_figure_provenance(computed).map_err(|refusal| FigureComparisonRefusal {
        reason: refusal.reason,
    })?;
    if computed.value_ticks != published_reference_ticks {
        return Err(FigureComparisonRefusal {
            reason: format!(
                "computed broker-statement figure {} ticks does not match published reference {} ticks",
                computed.value_ticks, published_reference_ticks
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FINANCE-PRIMITIVES-R005.3.4
    #[test]
    fn performance_card_is_verbatim_and_unavailable_is_never_zero() {
        use crate::returns::{twr_ticks, ReturnError, ReturnFlow, ValuationPoint};
        let at = |ns, whole: i64| ValuationPoint {
            ns,
            value_ticks: whole * SCALE / 10,
        };
        // 100, 110 then 121.1 with a 10 deposit in the second period: TWR 0.111.
        let valuations = [at(0, 1000), at(10, 1100), at(20, 1211)];
        let flows = [ReturnFlow {
            ns: 20,
            amount_ticks: 10 * SCALE,
        }];
        let ticks = twr_ticks(&valuations, &flows).expect("the two-period return");
        assert_eq!(ticks_to_decimal(ticks), "0.11100000");
        let admitted = GoldenFixtureFigure {
            value_ticks: ticks,
            source: "broker-statement:2026-09".to_string(),
            as_of: "2026-09-30T00:00:00Z".to_string(),
            session: "regular".to_string(),
        };
        assert!(check_figure_provenance(&admitted).is_ok());
        assert!(check_figure_provenance(&GoldenFixtureFigure {
            session: String::new(),
            ..admitted
        })
        .is_err());
        assert_eq!(ticks_to_decimal(-250_000_000), "-2.50000000");
        // No valuation at all is an error, never a zero return.
        assert_eq!(twr_ticks(&[], &[]), Err(ReturnError::NoPeriod));
        assert_eq!(ticks_to_decimal(0), "0.00000000");
    }

    fn figure() -> GoldenFixtureFigure {
        GoldenFixtureFigure {
            value_ticks: 12_345 * SCALE,
            source: "broker-statement:2026-09".to_string(),
            as_of: "2026-09-30T00:00:00Z".to_string(),
            session: "regular".to_string(),
        }
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R013.1
    #[test]
    fn a_fully_provenanced_figure_is_admitted() {
        assert!(check_figure_provenance(&figure()).is_ok());
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R013.1
    #[test]
    fn a_figure_missing_its_source_is_refused() {
        let mut bad = figure();
        bad.source = String::new();
        let refusal = check_figure_provenance(&bad).unwrap_err();
        assert!(refusal.reason.contains("source"));
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R013.1
    #[test]
    fn a_figure_missing_its_as_of_time_is_refused() {
        let mut bad = figure();
        bad.as_of = String::new();
        assert!(check_figure_provenance(&bad).is_err());
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R013.1
    #[test]
    fn a_figure_missing_its_session_is_refused() {
        let mut bad = figure();
        bad.session = String::new();
        assert!(check_figure_provenance(&bad).is_err());
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R013.2.1
    #[test]
    fn a_broker_statement_figure_matching_its_published_reference_is_admitted() {
        let computed = figure();
        let published_reference_ticks = computed.value_ticks;
        assert!(compare_broker_statement_figure(&computed, published_reference_ticks).is_ok());
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R013.2.1
    #[test]
    fn a_broker_statement_figure_diverging_from_its_published_reference_is_refused() {
        let computed = figure();
        let published_reference_ticks = computed.value_ticks + SCALE;
        let refusal =
            compare_broker_statement_figure(&computed, published_reference_ticks).unwrap_err();
        assert!(refusal
            .reason
            .contains("does not match published reference"));
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R013.2.1
    #[test]
    fn a_broker_statement_figure_with_incomplete_provenance_is_refused_before_comparison() {
        let mut bad = figure();
        bad.source = String::new();
        let refusal = compare_broker_statement_figure(&bad, bad.value_ticks).unwrap_err();
        assert!(refusal.reason.contains("source"));
    }
}
