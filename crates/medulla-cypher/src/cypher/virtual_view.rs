//! openCypher over a graph view (MEDULLA-UNIFIED-DATA-PLANE-R038.6.2 and R038.6.4).
//!
//! [`compile_view_query`] turns the node and relationship patterns of one read query
//! into one SPARQL `SELECT` for the virtual path over the same approved mappings, so one
//! evaluator serves both languages. Names follow one convention: a node label maps to the
//! class of [`GraphView`]; a property `n.p` is the predicate `<namespace of the label's
//! class>p`; a relationship type `R` between labels is the predicate `<namespace of the
//! from-label's class>R`. A write, an unmapped label, a path deeper than the knob and an
//! unsupported clause refuse before any source is touched.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use medulla_types::virtual_graph_view::{GraphView, InvalidGraphView, ViewJoin, ViewPlan};
use serde_json::Value;

use super::parser;
use super::plan::{
    Accessor, CompareOp, CypherQuery, Direction, Expr, NodePat, PropVal, ReadStage, Statement,
    Test, WhereExpr, WriteOp,
};

/// A graph-view query was refused. Every refusal carries a stable code and the
/// offending token, and runs zero source statements.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewRefusal {
    /// `VIRTUAL_VIEW_READ_ONLY`: a write clause or write mode against a view.
    ReadOnly(String),
    /// `UNMAPPED_LABEL`: a label or relationship type the view does not declare.
    UnmappedLabel(String),
    /// `PATH_DEPTH_EXCEEDED`: a variable-length path deeper than the knob.
    PathDepthExceeded(String),
    /// `VIEW_CLAUSE_UNSUPPORTED`: a read clause the view path does not compile.
    ClauseUnsupported(String),
    /// `VIEW_JOIN_INVALID`: a relationship join condition that is not `a.col = b.col`.
    JoinInvalid(String),
    /// `UNKNOWN_VIEW`: no view of that name is registered for the caller.
    UnknownView(String),
    /// `INVALID_ARGUMENT`: the query or the view does not parse or validate.
    Invalid(String),
}

impl ViewRefusal {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ReadOnly(_) => "VIRTUAL_VIEW_READ_ONLY",
            Self::UnmappedLabel(_) => "UNMAPPED_LABEL",
            Self::PathDepthExceeded(_) => "PATH_DEPTH_EXCEEDED",
            Self::ClauseUnsupported(_) => "VIEW_CLAUSE_UNSUPPORTED",
            Self::JoinInvalid(_) => "VIEW_JOIN_INVALID",
            Self::UnknownView(_) => "UNKNOWN_VIEW",
            Self::Invalid(_) => "INVALID_ARGUMENT",
        }
    }

    /// The offending token.
    pub fn token(&self) -> &str {
        match self {
            Self::ReadOnly(t)
            | Self::UnmappedLabel(t)
            | Self::PathDepthExceeded(t)
            | Self::ClauseUnsupported(t)
            | Self::JoinInvalid(t)
            | Self::UnknownView(t)
            | Self::Invalid(t) => t,
        }
    }
}

impl std::fmt::Display for ViewRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.token())
    }
}

impl std::error::Error for ViewRefusal {}

/// The graph views a caller may read, by name.
pub trait ViewResolver {
    /// The view registered under `name` for this caller, if any.
    fn view(&self, name: &str) -> Option<GraphView>;
}

/// Resolve the named view and compile `text` against it. The one entry the served
/// read path calls (MEDULLA-UNIFIED-DATA-PLANE-R038.6.3).
pub fn resolve_view_plan(
    views: &dyn ViewResolver,
    name: &str,
    text: &str,
    max_path_depth: usize,
) -> Result<ViewPlan, ViewRefusal> {
    let view = views
        .view(name)
        .ok_or_else(|| ViewRefusal::UnknownView(name.to_string()))?;
    compile_view_statement(&view, text, max_path_depth)
}

/// Parse `text` and compile it: a write statement refuses with `VIRTUAL_VIEW_READ_ONLY`.
pub fn compile_view_statement(
    view: &GraphView,
    text: &str,
    max_path_depth: usize,
) -> Result<ViewPlan, ViewRefusal> {
    match parser::parse_statement(text).map_err(ViewRefusal::Invalid)? {
        Statement::Write(write) => Err(ViewRefusal::ReadOnly(
            write
                .ops
                .first()
                .map_or("WRITE", |op| match op {
                    WriteOp::Create(_) => "CREATE",
                    WriteOp::Merge(_) => "MERGE",
                    WriteOp::Set(_) => "SET",
                    WriteOp::Delete { .. } => "DELETE",
                    WriteOp::Remove(_) => "REMOVE",
                })
                .to_string(),
        )),
        Statement::Read(query) => compile_view_query(view, &query, max_path_depth),
    }
}

/// Split a leading `EXPLAIN` keyword off a view query: `(explain, rest)`
/// (MEDULLA-UNIFIED-DATA-PLANE-R038.6.5).
pub fn split_explain(text: &str) -> (bool, &str) {
    let trimmed = text.trim_start();
    match trimmed.get(..7) {
        Some(head)
            if head.eq_ignore_ascii_case("explain")
                && trimmed[7..].starts_with(|c: char| c.is_whitespace()) =>
        {
            (true, trimmed[7..].trim_start())
        }
        _ => (false, text),
    }
}

/// One source statement of a view read with the digest of the mapping it ran under
/// (MEDULLA-UNIFIED-DATA-PLANE-R038.6.5). Never carries an endpoint or credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExplainFragment {
    /// The registered name of the source that ran the statement.
    pub source: String,
    pub pushed_statement: String,
    pub mapping_digest: String,
}

/// Pair every statement the sources ran (`(source name, statement)`) with the mapping digest
/// of `plan`. A statement that names an endpoint is withheld rather than shown.
pub fn explain_fragments(plan: &ViewPlan, pushed: &[(String, String)]) -> Vec<ExplainFragment> {
    let digest = plan.mapping_digests.first().cloned().unwrap_or_default();
    pushed
        .iter()
        .map(|(source, statement)| ExplainFragment {
            source: source.clone(),
            pushed_statement: if statement.contains("://") {
                "[statement withheld: it names an endpoint]".to_string()
            } else {
                statement.clone()
            },
            mapping_digest: digest.clone(),
        })
        .collect()
}

fn unsupported<T>(token: &str) -> Result<T, ViewRefusal> {
    Err(ViewRefusal::ClauseUnsupported(token.to_string()))
}

fn namespace_of(class: &str) -> Option<&str> {
    if !class.contains("://") {
        return None;
    }
    class.rfind(['#', '/']).map(|at| &class[..=at])
}

fn sparql_literal(value: &Value) -> Result<String, ViewRefusal> {
    match value {
        Value::String(text) => {
            let mut out = String::from("\"");
            for ch in text.chars() {
                match ch {
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    other => out.push(other),
                }
            }
            out.push('"');
            Ok(out)
        }
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        _ => unsupported("literal"),
    }
}

fn compare_symbol(op: CompareOp) -> Result<&'static str, ViewRefusal> {
    match op {
        CompareOp::Eq => Ok("="),
        CompareOp::Lt => Ok("<"),
        CompareOp::Le => Ok("<="),
        CompareOp::Gt => Ok(">"),
        CompareOp::Ge => Ok(">="),
        CompareOp::Ne => unsupported("<>"),
    }
}

/// `a.col = b.col` -> (`col` of a, `col` of b).
fn parse_join(condition: &str) -> Result<(String, String), ViewRefusal> {
    let invalid = || ViewRefusal::JoinInvalid(condition.to_string());
    let (left, right) = condition.split_once('=').ok_or_else(invalid)?;
    let column = |side: &str| -> Option<String> {
        let name = side.trim().rsplit('.').next()?.trim();
        (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .then(|| name.to_string())
    };
    match (column(left), column(right)) {
        (Some(from), Some(to)) => Ok((from, to)),
        _ => Err(invalid()),
    }
}

fn conditions<'a>(
    expr: &'a WhereExpr,
    out: &mut Vec<&'a super::plan::Condition>,
) -> Result<(), ViewRefusal> {
    match expr {
        WhereExpr::And(parts) => parts.iter().try_for_each(|part| conditions(part, out)),
        WhereExpr::Cond(condition) => {
            out.push(condition);
            Ok(())
        }
        WhereExpr::Or(_) => unsupported("OR"),
    }
}

/// Property variable bookkeeping: `(node variable, property)` -> SPARQL variable.
struct Props<'a> {
    namespaces: &'a BTreeMap<String, String>,
    bound: BTreeMap<(String, String), String>,
    triples: Vec<String>,
}

impl Props<'_> {
    fn var(&mut self, node: &str, prop: &str) -> Result<String, ViewRefusal> {
        let key = (node.to_string(), prop.to_string());
        if let Some(existing) = self.bound.get(&key) {
            return Ok(existing.clone());
        }
        let Some(namespace) = self.namespaces.get(node) else {
            return unsupported(node);
        };
        let var = format!("{node}__{prop}");
        self.triples
            .push(format!("?{node} <{namespace}{prop}> ?{var} ."));
        self.bound.insert(key, var.clone());
        Ok(var)
    }
}

/// Compile one read query against `view`. `max_path_depth` is the value of
/// `knob federation.cypher_max_path_depth`.
pub fn compile_view_query(
    view: &GraphView,
    query: &CypherQuery,
    max_path_depth: usize,
) -> Result<ViewPlan, ViewRefusal> {
    view.validate().map_err(|error| match error {
        InvalidGraphView::UnmappedLabel(label) => ViewRefusal::UnmappedLabel(label),
        other => ViewRefusal::Invalid(other.to_string()),
    })?;
    let mut matches = Vec::new();
    for stage in &query.stages {
        match stage {
            ReadStage::Match {
                pattern,
                optional: false,
                where_clause,
                path_var: None,
            } => matches.push((pattern, where_clause)),
            ReadStage::Match { optional: true, .. } => return unsupported("OPTIONAL MATCH"),
            ReadStage::Match { .. } => return unsupported("path variable"),
            ReadStage::With { .. } => return unsupported("WITH"),
            ReadStage::Unwind { .. } => return unsupported("UNWIND"),
            ReadStage::Call { .. } | ReadStage::CallProc { .. } => return unsupported("CALL"),
        }
    }
    let [(pattern, where_clause)] = matches.as_slice() else {
        return unsupported("MATCH");
    };
    let ret = &query.ret;
    if ret.star {
        return unsupported("RETURN *");
    }
    if ret.distinct {
        return unsupported("DISTINCT");
    }
    if !ret.order_by.is_empty() {
        return unsupported("ORDER BY");
    }
    if ret.skip.is_some() {
        return unsupported("SKIP");
    }

    // Nodes: the start node, then the node after each hop; labels come from the
    // pattern or are inferred from the relationship that touches them.
    let mut nodes: Vec<&NodePat> = vec![&pattern.start];
    nodes.extend(pattern.hops.iter().map(|(_, node)| node));
    let mut labels: Vec<Option<String>> = nodes.iter().map(|node| node.label.clone()).collect();
    let mut rels = Vec::new();
    for (index, (edge, _)) in pattern.hops.iter().enumerate() {
        let Some(rel_type) = edge.rel_type.as_deref() else {
            return unsupported("untyped relationship");
        };
        let rel = view
            .relationship_types
            .iter()
            .find(|rel| rel.rel_type == rel_type)
            .ok_or_else(|| ViewRefusal::UnmappedLabel(rel_type.to_string()))?;
        let (from, to) = match edge.direction {
            Direction::Right => (index, index + 1),
            Direction::Left => (index + 1, index),
            Direction::Both => return unsupported("undirected relationship"),
        };
        for (slot, expected) in [(from, &rel.from_label), (to, &rel.to_label)] {
            match &labels[slot] {
                None => labels[slot] = Some(expected.clone()),
                Some(found) if found != expected => {
                    return Err(ViewRefusal::UnmappedLabel(format!("{rel_type}:{found}")))
                }
                Some(_) => {}
            }
        }
        if edge.props.is_some() {
            return unsupported("relationship properties");
        }
        if edge.group.is_some() {
            return unsupported("quantified path");
        }
        if edge.var.is_some() {
            return unsupported("relationship variable");
        }
        rels.push(rel);
    }

    let mut vars = Vec::new();
    let mut classes = Vec::new();
    let mut namespaces: BTreeMap<String, String> = BTreeMap::new();
    let mut triples = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        let label = labels[index]
            .as_deref()
            .ok_or_else(|| ViewRefusal::UnmappedLabel("(unlabeled)".into()))?;
        let mapping = view
            .resolve_label(label)
            .map_err(|_| ViewRefusal::UnmappedLabel(label.to_string()))?;
        let namespace = namespace_of(&mapping.ontology_class)
            .ok_or_else(|| ViewRefusal::UnmappedLabel(label.to_string()))?;
        let var = node.var.clone().unwrap_or_else(|| format!("_n{index}"));
        triples.push(format!("?{var} a <{}> .", mapping.ontology_class));
        namespaces.insert(var.clone(), namespace.to_string());
        classes.push(mapping.ontology_class.clone());
        vars.push(var);
    }

    let mut joins: Vec<ViewJoin> = Vec::new();
    for (index, ((edge, _), rel)) in pattern.hops.iter().zip(&rels).enumerate() {
        let (from, to) = match edge.direction {
            Direction::Left => (index + 1, index),
            _ => (index, index + 1),
        };
        let predicate = format!("{}{}", namespaces[&vars[from]], rel.rel_type);
        let path = match edge.var_len {
            None => format!("<{predicate}>"),
            Some((low, high)) => {
                if low == 0 {
                    return unsupported("*0");
                }
                if high > max_path_depth {
                    return Err(ViewRefusal::PathDepthExceeded(format!("*{low}..{high}")));
                }
                let alternatives = (low..=high)
                    .map(|len| vec![format!("<{predicate}>"); len].join("/"))
                    .collect::<Vec<_>>()
                    .join("|");
                format!("({alternatives})")
            }
        };
        triples.push(format!("?{} {path} ?{} .", vars[from], vars[to]));
        if !joins.iter().any(|join| join.rel_predicate == predicate) {
            let (on_from, on_to) = parse_join(&rel.join_condition)?;
            joins.push(ViewJoin {
                rel_predicate: predicate,
                from_class: classes[from].clone(),
                to_class: classes[to].clone(),
                on_from,
                on_to,
            });
        }
    }

    let mut props = Props {
        namespaces: &namespaces,
        bound: BTreeMap::new(),
        triples: Vec::new(),
    };
    let mut filters = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        for (key, value) in node.props.iter().flatten() {
            let PropVal::Lit(literal) = value else {
                return unsupported("parameter");
            };
            let var = props.var(&vars[index], key)?;
            filters.push(format!("FILTER (?{var} = {})", sparql_literal(literal)?));
        }
    }
    if let Some(expr) = where_clause {
        let mut found = Vec::new();
        conditions(expr, &mut found)?;
        for condition in found {
            let Accessor::Prop(prop) = &condition.accessor else {
                return unsupported("labels()/type()");
            };
            let Test::Cmp(op, value) = &condition.test else {
                return unsupported(match &condition.test {
                    Test::In(_) => "IN",
                    Test::StartsWith(_) => "STARTS WITH",
                    Test::EndsWith(_) => "ENDS WITH",
                    Test::Contains(_) => "CONTAINS",
                    Test::IsNull => "IS NULL",
                    _ => "IS NOT NULL",
                });
            };
            let var = props.var(&condition.var, prop)?;
            filters.push(format!(
                "FILTER (?{var} {} {})",
                compare_symbol(*op)?,
                sparql_literal(value)?
            ));
        }
    }
    let mut projection = Vec::new();
    for item in &ret.items {
        let var = match &item.expr {
            Expr::Var(name) if namespaces.contains_key(name) => name.clone(),
            Expr::Prop(name, prop) => props.var(name, prop)?,
            other => return unsupported(&other.column()),
        };
        projection.push((var, item.column()));
    }
    if projection.is_empty() {
        return unsupported("RETURN");
    }

    let mut sparql = String::from("SELECT");
    for (var, _) in &projection {
        let _ = write!(sparql, " ?{var}");
    }
    sparql.push_str(" WHERE { ");
    for triple in triples.iter().chain(&props.triples) {
        sparql.push_str(triple);
        sparql.push(' ');
    }
    for filter in &filters {
        sparql.push_str(filter);
        sparql.push(' ');
    }
    sparql.push('}');
    if let Some(limit) = ret.limit {
        let _ = write!(sparql, " LIMIT {limit}");
    }
    Ok(ViewPlan {
        sparql,
        mapping_digests: vec![view.mapping_digest.clone()],
        joins,
        projection,
    })
}

#[cfg(test)]
#[path = "tests/virtual_view.rs"]
mod tests;
