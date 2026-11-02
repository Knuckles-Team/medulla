//! medulla-uql: UQL language: parser, binder and lowering to the executor.
//!
//! Level L5 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: federated-query-and-reasoning (UQL).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! UQL — the Unified Query Language front-end (CONCEPT:AU-KG.query.top-nodes-by-degree).
//!
//! A human/agent-writable TEXT surface that parses to the SAME [`medulla_types::wire::Plan`]
//! AST `Method::UnifiedQuery` already executes — so one query expresses filter
//! (relational) + traverse (graph) + rank (vector) + every other modality without a
//! caller hand-building the Op list. A pure FRONT-END: it adds no executor.
//!
//! * [`grammar`] — THE grammar (one table; the docs, the NL prompt and error lists are
//!   generated from it);
//! * [`parse`] / [`parse_with`] / [`parse_statement`] — text → plan, with typed `$name`
//!   parameters bound as values (never spliced into text);
//! * [`medulla_types::wire::Plan::to_uql`] / [`print`] — plan → canonical text; for every
//!   printable plan `parse(print(p)) == canonicalize(p)`;
//! * [`UqlError`] — structured diagnostics (stable code, span, expected set, fix hint).
//!
//! The front-end is DEPENDENCY-FREE (no DataFusion, no regex), so it ships in a
//! default/Pi build — only EXECUTION of the resulting Plan is `query`-gated.
//!
//! ```
//! use medulla_uql as uql;
//! use medulla_types::wire::{Op, Pred};
//!
//! let plan = uql::parse(
//!     "MATCH (:Doc) WHERE year > 2024 |> TRAVERSE -[:CITES]->{1,2} \
//!      |> RANK BY ~[1.0, 0.0] |> LIMIT 10",
//! )
//! .unwrap();
//! assert_eq!(plan.ops[0], Op::Scan { label: "Doc".into() });
//! assert_eq!(
//!     plan.ops[1],
//!     Op::Filter { preds: vec![Pred::GtNum { prop: "year".into(), n: 2024.0 }] }
//! );
//! ```

use std::collections::BTreeMap;

use medulla_types::wire::{Plan, UqlParam};

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod diag;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod grammar;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod lexer;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod parser;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod print;
/// Running a parsed statement: rows with score channels, EXPLAIN, PROFILE (UQL-07/08/09).
pub use diag::render_caret;
pub use diag::{UqlCode, UqlError, UqlWarnCode, UqlWarning, ALL_CODES};
#[cfg(feature = "query")]
pub use parser::program::to_plan_dag;
pub use parser::program::{Annotations, Body, DagNode, Mode, Statement, UQL_VERSION};
pub use print::canonicalize;

/// Typed parameter bindings: `$name` → value.
pub type Params = BTreeMap<String, UqlParam>;

/// Parse a full statement (version pragma, `EXPLAIN`/`PROFILE`, `LET` bindings) with
/// typed parameters. Every bound parameter must be referenced.
pub fn parse_statement(src: &str, params: &Params) -> Result<Statement, UqlError> {
    let tokens = lexer::lex(src)?;
    parser::Parser::new(src, &tokens, params).statement()
}

/// Parse a plain pipeline with typed parameters into the [`Plan`] `UnifiedQuery` runs.
/// `EXPLAIN`/`PROFILE`, DAG programs and `WITH …` annotations are refused
/// (`UQL_STATEMENT_NOT_PIPELINE`) — use [`parse_statement`] for those.
pub fn parse_with(src: &str, params: &Params) -> Result<Plan, UqlError> {
    let stmt = parse_statement(src, params)?;
    #[cfg(feature = "federation")]
    reject_pipeline_federation_budget(&stmt, src)?;
    match (stmt.mode, stmt.body, stmt.annotations.any()) {
        (Mode::Run, Body::Pipeline(plan), false) => Ok(plan),
        _ => Err(UqlError::new(
            UqlCode::StatementNotPipeline,
            "this is an EXPLAIN/PROFILE statement, a LET … FROM/JOIN program or an annotated \
             (`WITH …`) statement, not a plain pipeline",
            (0, src.len().min(1)),
        )
        .with_help("use `medulla_uql::parse_statement`")),
    }
}

#[cfg(feature = "federation")]
fn reject_pipeline_federation_budget(stmt: &Statement, src: &str) -> Result<(), UqlError> {
    if stmt.federation_budget.is_some() {
        return Err(UqlError::new(
            UqlCode::StatementNotPipeline,
            "a federation budget hint requires statement execution",
            (0, src.len().min(1)),
        )
        .with_help("use `medulla_uql::parse_statement`"));
    }
    Ok(())
}

/// Parse one series expression (the `DERIVE` sub-grammar, EH-522) whose channel names are
/// bound by the caller — a materialised derived series' sources (EH-524).
pub fn parse_series_expr(src: &str) -> Result<medulla_types::series_expr::SeriesExpr, UqlError> {
    let tokens = lexer::lex(src)?;
    let params = Params::new();
    parser::Parser::new(src, &tokens, &params).series_definition()
}

/// Parse a plain pipeline (no parameters) into the [`Plan`] AST.
pub fn parse(src: &str) -> Result<Plan, UqlError> {
    parse_with(src, &Params::new())
}
