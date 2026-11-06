//! Terms, filters and the projection of a view read.

use serde_json::Value;

use super::compiler::{Compiler, Pushed};
use super::{unsupported, ViewRefusal};
use crate::cypher::plan::{Accessor, CompareOp, Condition, Expr, ReturnSpec, Test, WhereExpr};

fn op_symbol(op: CompareOp) -> Option<&'static str> {
    match op {
        CompareOp::Eq => Some("="),
        CompareOp::Lt => Some("<"),
        CompareOp::Le => Some("<="),
        CompareOp::Gt => Some(">"),
        CompareOp::Ge => Some(">="),
        CompareOp::Ne => None,
    }
}

/// A scalar JSON value as a SQL and SPARQL literal.
fn render_literal(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(format!(
            "'{}'",
            s.replace('\\', "\\\\").replace('\'', "\\'")
        )),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

const COND_TOKENS: [(&str, fn(&Test) -> bool); 4] = [
    ("IN", |t| matches!(t, Test::In(_))),
    ("STARTS WITH", |t| matches!(t, Test::StartsWith(_))),
    ("ENDS WITH", |t| matches!(t, Test::EndsWith(_))),
    ("CONTAINS", |t| matches!(t, Test::Contains(_))),
];

fn cond_token(cond: &Condition) -> String {
    let named = COND_TOKENS.iter().find(|(_, is)| is(&cond.test));
    match (&cond.accessor, named) {
        (Accessor::RelType, _) => "type()".to_string(),
        (Accessor::Labels, _) => "labels()".to_string(),
        (_, Some((name, _))) => (*name).to_string(),
        _ => "IS NULL".to_string(),
    }
}

fn ret_token(ret: &ReturnSpec) -> &'static str {
    [
        (ret.star, "RETURN *"),
        (!ret.order_by.is_empty(), "ORDER BY"),
        (ret.skip.is_some(), "SKIP"),
    ]
    .into_iter()
    .find_map(|(hit, name)| hit.then_some(name))
    .unwrap_or("RETURN")
}

impl Compiler<'_> {
    pub(super) fn class_term(&mut self, class: &str) -> String {
        match class.split_once(':') {
            Some((prefix, _)) if !class.contains("://") => {
                self.prefixes.insert(prefix.to_string());
                class.to_string()
            }
            _ => format!("<{class}>"),
        }
    }

    /// `<namespace of class + local>`, or `prefix:local` for a CURIE class.
    pub(super) fn predicate_term(&mut self, class: &str, local: &str) -> String {
        if class.contains("://") {
            let cut = class.rfind(['#', '/']).map_or(0, |i| i + 1);
            return format!("<{}{local}>", &class[..cut]);
        }
        let prefix = class.split(':').next().unwrap_or_default();
        self.prefixes.insert(prefix.to_string());
        format!("{prefix}:{local}")
    }

    fn prop_var(&mut self, var: &str, prop: &str) -> Result<String, ViewRefusal> {
        let class = self
            .bound
            .iter()
            .find(|b| b.var == var)
            .map(|b| b.mapping.ontology_class.clone())
            .ok_or_else(|| unsupported(format!("unbound variable {var}")))?;
        let name = format!("{var}__{prop}");
        if self.props.insert((var.to_string(), prop.to_string())) {
            let predicate = self.predicate_term(&class, prop);
            self.triples.push(format!("?{var} {predicate} ?{name} ."));
        }
        Ok(format!("?{name}"))
    }

    /// Add `var.prop <op> literal` as a SPARQL filter and a pushed source predicate.
    pub(super) fn compare(
        &mut self,
        var: &str,
        prop: &str,
        op: CompareOp,
        lit: &Value,
    ) -> Result<(), ViewRefusal> {
        let sym = op_symbol(op).ok_or_else(|| unsupported("<>"))?;
        let literal = render_literal(lit).ok_or_else(|| unsupported("non-scalar literal"))?;
        let target = self.prop_var(var, prop)?;
        self.filters
            .push(format!("FILTER({target} {sym} {literal})"));
        self.pushed.push(Pushed {
            var: var.to_string(),
            prop: prop.to_string(),
            op: sym,
            literal,
        });
        Ok(())
    }

    /// `WHERE` supports equality, ordering comparisons and `AND`.
    pub(super) fn add_where(&mut self, expr: &WhereExpr) -> Result<(), ViewRefusal> {
        match expr {
            WhereExpr::And(parts) => parts.iter().try_for_each(|p| self.add_where(p)),
            WhereExpr::Or(_) => Err(unsupported("OR")),
            WhereExpr::Cond(cond) => match (&cond.accessor, &cond.test) {
                (Accessor::Prop(prop), Test::Cmp(op, lit)) => {
                    self.compare(&cond.var, prop, *op, lit)
                }
                _ => Err(unsupported(cond_token(cond))),
            },
        }
    }

    /// `RETURN` projects variables and properties.
    pub(super) fn projection(&mut self, ret: &ReturnSpec) -> Result<Vec<String>, ViewRefusal> {
        if ret.star || !ret.order_by.is_empty() || ret.skip.is_some() {
            return Err(unsupported(ret_token(ret)));
        }
        ret.items
            .iter()
            .map(|item| match &item.expr {
                Expr::Var(v) => Ok(format!("?{v}")),
                Expr::Prop(v, p) => self.prop_var(v, p),
                other => Err(unsupported(other.column())),
            })
            .collect()
    }
}
