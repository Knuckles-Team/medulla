//! `cfg` predicates: parsed with `syn`, evaluated for satisfiability without `test`.

use syn::punctuated::Punctuated;
use syn::{Expr, ExprLit, Lit, Meta, Token};

/// A parsed `cfg` expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Cfg {
    Atom(String),
    All(Vec<Cfg>),
    Any(Vec<Cfg>),
    Not(Box<Cfg>),
}

/// Atoms that are never set in a production build.
const NON_PRODUCTION_ATOMS: &[&str] = &["test", "feature=\"test-support\""];

/// Parse the text between the parentheses of `#[cfg(...)]`.
pub fn parse(expression: &str) -> Result<Cfg, String> {
    let meta: Meta =
        syn::parse_str(expression).map_err(|e| format!("unsupported cfg `{expression}`: {e}"))?;
    from_meta(&meta)
}

fn from_meta(meta: &Meta) -> Result<Cfg, String> {
    match meta {
        Meta::Path(path) => Ok(Cfg::Atom(path_text(path))),
        Meta::NameValue(pair) => match &pair.value {
            Expr::Lit(ExprLit {
                lit: Lit::Str(value),
                ..
            }) => Ok(Cfg::Atom(format!(
                "{}=\"{}\"",
                path_text(&pair.path),
                value.value()
            ))),
            _ => Err("cfg name-value must carry a string literal".into()),
        },
        Meta::List(list) => {
            let inner = list
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .map_err(|e| format!("unsupported cfg list: {e}"))?;
            let children = inner.iter().map(from_meta).collect::<Result<Vec<_>, _>>()?;
            match path_text(&list.path).as_str() {
                "all" => Ok(Cfg::All(children)),
                "any" => Ok(Cfg::Any(children)),
                "not" if children.len() == 1 => {
                    Ok(Cfg::Not(Box::new(children.into_iter().next().unwrap())))
                }
                other => Err(format!("unsupported cfg operator `{other}`")),
            }
        }
    }
}

fn path_text(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn evaluate(cfg: &Cfg, values: &std::collections::BTreeMap<String, bool>) -> Option<bool> {
    match cfg {
        Cfg::Atom(atom) if NON_PRODUCTION_ATOMS.contains(&atom.as_str()) => Some(false),
        Cfg::Atom(atom) => values.get(atom).copied(),
        Cfg::All(children) => {
            let all: Vec<_> = children.iter().map(|c| evaluate(c, values)).collect();
            if all.contains(&Some(false)) {
                Some(false)
            } else if all.iter().all(|v| *v == Some(true)) {
                Some(true)
            } else {
                None
            }
        }
        Cfg::Any(children) => {
            let all: Vec<_> = children.iter().map(|c| evaluate(c, values)).collect();
            if all.contains(&Some(true)) {
                Some(true)
            } else if all.iter().all(|v| *v == Some(false)) {
                Some(false)
            } else {
                None
            }
        }
        Cfg::Not(child) => evaluate(child, values).map(|v| !v),
    }
}

fn collect(cfg: &Cfg, atoms: &mut std::collections::BTreeSet<String>) {
    match cfg {
        Cfg::Atom(atom) => {
            if !NON_PRODUCTION_ATOMS.contains(&atom.as_str()) {
                atoms.insert(atom.clone());
            }
        }
        Cfg::All(c) | Cfg::Any(c) => c.iter().for_each(|x| collect(x, atoms)),
        Cfg::Not(c) => collect(c, atoms),
    }
}

/// Whether every predicate can hold at once in a build where `test` is off.
pub fn possible_without_test(predicates: &[Cfg]) -> Result<bool, String> {
    if predicates.is_empty() {
        return Ok(true);
    }
    let mut atoms = std::collections::BTreeSet::new();
    predicates.iter().for_each(|p| collect(p, &mut atoms));
    if atoms.len() > 16 {
        return Err("Rust cfg expression exceeds the static production-proof bound".into());
    }
    let names: Vec<String> = atoms.into_iter().collect();
    let mut values = std::collections::BTreeMap::new();
    Ok(satisfiable(predicates, &names, 0, &mut values))
}

fn satisfiable(
    predicates: &[Cfg],
    names: &[String],
    position: usize,
    values: &mut std::collections::BTreeMap<String, bool>,
) -> bool {
    let evaluated: Vec<_> = predicates.iter().map(|p| evaluate(p, values)).collect();
    if evaluated.contains(&Some(false)) {
        return false;
    }
    if evaluated.iter().all(|v| *v == Some(true)) {
        return true;
    }
    if position == names.len() {
        return false;
    }
    for choice in [true, false] {
        values.insert(names[position].clone(), choice);
        if satisfiable(predicates, names, position + 1, values) {
            return true;
        }
    }
    values.remove(&names[position]);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn possible(expression: &str) -> bool {
        possible_without_test(&[parse(expression).unwrap()]).unwrap()
    }

    #[test]
    fn test_only_items_are_not_production() {
        assert!(!possible("test"));
        assert!(!possible("all(test, feature = \"x\")"));
        assert!(!possible("feature = \"test-support\""));
        assert!(possible("not(test)"));
        assert!(possible("any(test, unix)"));
        assert!(possible("feature = \"x\""));
    }

    #[test]
    fn predicates_conjoin_across_nesting() {
        let a = parse("feature = \"x\"").unwrap();
        let b = parse("not(feature = \"x\")").unwrap();
        assert!(!possible_without_test(&[a, b]).unwrap());
    }

    #[test]
    fn unsupported_operator_fails_closed() {
        assert!(parse("weird(a)").is_err());
    }
}
