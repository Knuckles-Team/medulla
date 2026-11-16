//! The form and the predicates of one SPARQL query text
//! (MEDULLA-FEDERATED-QUERY-R058.4.11).
//!
//! A natural-language planner answers with SPARQL text. Before that text runs, the
//! engine needs two facts from the one parser it already uses: which query form
//! it is (an update form is never a query), and which predicate IRIs its graph
//! patterns, property paths, filters and construct template name.

use std::collections::BTreeSet;

use spargebra::algebra::{Expression, GraphPattern, PropertyPathExpression};
use spargebra::term::{NamedNodePattern, TriplePattern};
use spargebra::Query;

use super::parse_query;

/// The read forms of SPARQL 1.1 query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryForm {
    /// `SELECT`.
    Select,
    /// `ASK`.
    Ask,
    /// `CONSTRUCT`.
    Construct,
    /// `DESCRIBE`.
    Describe,
}

/// Why a text has no query shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShapeError {
    /// The text parses as neither a query nor an update; carries the parser message.
    Parse(String),
    /// The text parses as a SPARQL update (`INSERT`, `DELETE`, `LOAD`, `CLEAR`, `DROP`,
    /// `CREATE`, ...): it is not a query.
    Update,
    /// A query construct whose predicates cannot be listed, so none is trusted.
    Unsupported(String),
}

/// A parsed query's form and every predicate IRI it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryShape {
    /// The query form.
    pub form: QueryForm,
    /// Absolute predicate IRIs of triple patterns, property paths (including negated
    /// property sets), nested `FILTER EXISTS` patterns and the construct template.
    pub predicates: BTreeSet<String>,
}

/// Parse `text` as a SPARQL query and list its form and predicates.
pub fn query_shape(text: &str) -> Result<QueryShape, ShapeError> {
    let query = match parse_query(text) {
        Ok(query) => query,
        Err(message) => {
            return Err(if crate::update::parse_update(text).is_ok() {
                ShapeError::Update
            } else {
                ShapeError::Parse(message)
            });
        }
    };
    let mut predicates = BTreeSet::new();
    let form = match &query {
        Query::Select { pattern, .. } => {
            walk_pattern(pattern, &mut predicates)?;
            QueryForm::Select
        }
        Query::Ask { pattern, .. } => {
            walk_pattern(pattern, &mut predicates)?;
            QueryForm::Ask
        }
        Query::Describe { pattern, .. } => {
            walk_pattern(pattern, &mut predicates)?;
            QueryForm::Describe
        }
        Query::Construct {
            template, pattern, ..
        } => {
            for triple in template {
                walk_triple(triple, &mut predicates);
            }
            walk_pattern(pattern, &mut predicates)?;
            QueryForm::Construct
        }
    };
    Ok(QueryShape { form, predicates })
}

fn walk_triple(triple: &TriplePattern, out: &mut BTreeSet<String>) {
    if let NamedNodePattern::NamedNode(node) = &triple.predicate {
        out.insert(node.as_str().to_string());
    }
}

fn walk_path(path: &PropertyPathExpression, out: &mut BTreeSet<String>) {
    match path {
        PropertyPathExpression::NamedNode(node) => {
            out.insert(node.as_str().to_string());
        }
        PropertyPathExpression::Reverse(inner)
        | PropertyPathExpression::ZeroOrMore(inner)
        | PropertyPathExpression::OneOrMore(inner)
        | PropertyPathExpression::ZeroOrOne(inner) => walk_path(inner, out),
        PropertyPathExpression::Sequence(left, right)
        | PropertyPathExpression::Alternative(left, right) => {
            walk_path(left, out);
            walk_path(right, out);
        }
        PropertyPathExpression::NegatedPropertySet(nodes) => {
            out.extend(nodes.iter().map(|node| node.as_str().to_string()));
        }
    }
}

fn walk_expression(expr: &Expression, out: &mut BTreeSet<String>) -> Result<(), ShapeError> {
    match expr {
        Expression::Or(a, b)
        | Expression::And(a, b)
        | Expression::Equal(a, b)
        | Expression::SameTerm(a, b)
        | Expression::Greater(a, b)
        | Expression::GreaterOrEqual(a, b)
        | Expression::Less(a, b)
        | Expression::LessOrEqual(a, b)
        | Expression::Add(a, b)
        | Expression::Subtract(a, b)
        | Expression::Multiply(a, b)
        | Expression::Divide(a, b) => {
            walk_expression(a, out)?;
            walk_expression(b, out)
        }
        Expression::In(a, list) => {
            walk_expression(a, out)?;
            list.iter().try_for_each(|e| walk_expression(e, out))
        }
        Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => {
            walk_expression(a, out)
        }
        Expression::Exists(pattern) => walk_pattern(pattern, out),
        Expression::If(a, b, c) => {
            walk_expression(a, out)?;
            walk_expression(b, out)?;
            walk_expression(c, out)
        }
        Expression::Coalesce(list) | Expression::FunctionCall(_, list) => {
            list.iter().try_for_each(|e| walk_expression(e, out))
        }
        _ => Ok(()),
    }
}

fn walk_pattern(pattern: &GraphPattern, out: &mut BTreeSet<String>) -> Result<(), ShapeError> {
    match pattern {
        GraphPattern::Bgp { patterns } => {
            patterns.iter().for_each(|triple| walk_triple(triple, out));
            Ok(())
        }
        GraphPattern::Path { path, .. } => {
            walk_path(path, out);
            Ok(())
        }
        GraphPattern::Join { left, right }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right } => {
            walk_pattern(left, out)?;
            walk_pattern(right, out)
        }
        GraphPattern::LeftJoin {
            left,
            right,
            expression,
        } => {
            walk_pattern(left, out)?;
            walk_pattern(right, out)?;
            expression
                .as_ref()
                .map_or(Ok(()), |expr| walk_expression(expr, out))
        }
        GraphPattern::Filter { expr, inner } => {
            walk_expression(expr, out)?;
            walk_pattern(inner, out)
        }
        GraphPattern::Extend {
            inner, expression, ..
        } => {
            walk_expression(expression, out)?;
            walk_pattern(inner, out)
        }
        GraphPattern::Graph { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::Group { inner, .. } => walk_pattern(inner, out),
        GraphPattern::Values { .. } => Ok(()),
        other => Err(ShapeError::Unsupported(format!(
            "graph pattern `{other}` has no listed predicates"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R058.4.11
    #[test]
    fn shape_lists_every_predicate_position_and_the_form() {
        let shape = query_shape(
            "SELECT ?s WHERE { ?s <http://x/a> ?o . OPTIONAL { ?o <http://x/b>/<http://x/c> ?p } \
             FILTER EXISTS { ?s <http://x/d> ?q } }",
        )
        .expect("select");
        assert_eq!(shape.form, QueryForm::Select);
        for iri in ["a", "b", "c", "d"] {
            assert!(
                shape.predicates.contains(&format!("http://x/{iri}")),
                "{iri}"
            );
        }
        let construct =
            query_shape("CONSTRUCT { ?s <http://x/t> ?o } WHERE { ?s <http://x/a> ?o }")
                .expect("construct");
        assert_eq!(construct.form, QueryForm::Construct);
        assert!(construct.predicates.contains("http://x/t"));
    }

    // spec: MEDULLA-FEDERATED-QUERY-R058.4.11
    #[test]
    fn an_update_text_is_an_update_not_a_parse_error() {
        assert_eq!(
            query_shape("INSERT DATA { <http://x/s> <http://x/p> <http://x/o> }"),
            Err(ShapeError::Update)
        );
        assert!(matches!(
            query_shape("SELECT WHERE {"),
            Err(ShapeError::Parse(_))
        ));
    }
}
