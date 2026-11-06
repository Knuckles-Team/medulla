//! openCypher over a named graph view (MEDULLA-UNIFIED-DATA-PLANE-R038.6.2/.4/.5).
//!
//! `compile_view_query` turns the node and relationship patterns of one read
//! query into a SPARQL query over the view's mapped classes, which the shared
//! virtual-graph evaluator runs. Each relationship hop is one same-source join:
//! it yields one pushed source statement and the digest of its mapping. A write,
//! an unmapped label, an over-deep path or an unsupported clause refuses before
//! any source statement exists.

use medulla_types::contract::ServerErrorCode;
use medulla_types::virtual_graph_view::GraphView;

use super::plan::{CypherQuery, Pattern, ReadStage, Statement, WhereExpr, WriteOp};

mod compiler;
mod render;
mod terms;

use compiler::Compiler;

/// Limits a view read compiles under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewLimits {
    /// Knob `federation.cypher_max_path_depth`.
    pub max_path_depth: usize,
}

/// A view read refused at compile time. No source statement ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewRefusal {
    /// A write clause or write mode against a view.
    ReadOnly(String),
    /// A label or relationship type the view does not map.
    UnmappedLabel(String),
    /// A variable-length path deeper than the knob.
    PathDepthExceeded(String),
    /// A read clause the view compiler does not translate.
    ClauseUnsupported(String),
}

impl ViewRefusal {
    /// The registered refusal code.
    pub fn server_code(&self) -> ServerErrorCode {
        match self {
            Self::ReadOnly(_) => ServerErrorCode::VirtualViewReadOnly,
            Self::UnmappedLabel(_) => ServerErrorCode::UnmappedLabel,
            Self::PathDepthExceeded(_) => ServerErrorCode::PathDepthExceeded,
            Self::ClauseUnsupported(_) => ServerErrorCode::ViewClauseUnsupported,
        }
    }

    /// The refusal code string.
    pub fn code(&self) -> &'static str {
        self.server_code().as_str()
    }

    /// The offending token.
    pub fn token(&self) -> &str {
        match self {
            Self::ReadOnly(t)
            | Self::UnmappedLabel(t)
            | Self::PathDepthExceeded(t)
            | Self::ClauseUnsupported(t) => t,
        }
    }
}

impl std::fmt::Display for ViewRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.token())
    }
}

impl std::error::Error for ViewRefusal {}

/// One compiled view read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewPlan {
    /// The SPARQL the virtual-graph evaluator runs.
    pub sparql: String,
    /// SHA-256 of each fragment's mapping, 64 lowercase hex characters.
    pub mapping_digests: Vec<String>,
    /// The source statement of each fragment, aligned with `mapping_digests`.
    pub pushed_statements: Vec<String>,
}

impl ViewPlan {
    /// Hand each pushed statement to `sink`, in fragment order.
    pub fn dispatch(&self, sink: &mut dyn FnMut(&str)) {
        self.pushed_statements.iter().for_each(|s| sink(s));
    }
}

/// Parse `text` and compile it against `view`; a write statement refuses.
pub fn compile_view_text(
    view: &GraphView,
    text: &str,
    limits: ViewLimits,
) -> Result<ViewPlan, ViewRefusal> {
    match super::parser::parse_statement(text) {
        Ok(Statement::Read(query)) => compile_view_query(view, &query, limits),
        Ok(Statement::Write(write)) => Err(ViewRefusal::ReadOnly(write_token(&write.ops))),
        Err(error) => Err(ViewRefusal::ClauseUnsupported(error)),
    }
}

fn write_token(ops: &[WriteOp]) -> String {
    match ops.first() {
        Some(WriteOp::Create(_)) => "CREATE",
        Some(WriteOp::Merge(_)) => "MERGE",
        Some(WriteOp::Set(_)) => "SET",
        Some(WriteOp::Delete { .. }) => "DELETE",
        Some(WriteOp::Remove(_)) => "REMOVE",
        None => "WRITE",
    }
    .to_string()
}

/// Compile one read query over `view` into a SPARQL plan.
pub fn compile_view_query(
    view: &GraphView,
    query: &CypherQuery,
    limits: ViewLimits,
) -> Result<ViewPlan, ViewRefusal> {
    let (pattern, where_clause) = single_match(query)?;
    let mut compiler = Compiler::new(view, limits);
    compiler.bind_pattern(pattern)?;
    if let Some(expr) = where_clause {
        compiler.add_where(expr)?;
    }
    let select = compiler.projection(&query.ret)?;
    Ok(render::render(&compiler, &select, &query.ret))
}

fn single_match(query: &CypherQuery) -> Result<(&Pattern, Option<&WhereExpr>), ViewRefusal> {
    let [ReadStage::Match {
        pattern,
        optional,
        where_clause,
        path_var,
    }] = query.stages.as_slice()
    else {
        return Err(unsupported(stage_token(query)));
    };
    if *optional {
        return Err(unsupported("OPTIONAL MATCH"));
    }
    if path_var.is_some() {
        return Err(unsupported("path variable"));
    }
    Ok((pattern, where_clause.as_ref()))
}

fn stage_token(query: &CypherQuery) -> &'static str {
    let other = query
        .stages
        .iter()
        .find(|s| !matches!(s, ReadStage::Match { .. }));
    match other {
        Some(ReadStage::With { .. }) => "WITH",
        Some(ReadStage::Unwind { .. }) => "UNWIND",
        Some(ReadStage::Call { .. }) => "CALL",
        Some(ReadStage::CallProc { .. }) => "CALL procedure",
        _ => "MATCH sequence",
    }
}

fn unsupported(token: impl Into<String>) -> ViewRefusal {
    ViewRefusal::ClauseUnsupported(token.into())
}
