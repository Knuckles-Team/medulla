//! SPARQL assembly and the pushed source statement of each fragment.

use medulla_types::contract::Digest256;

use super::compiler::{Compiler, Fragment};
use super::ViewPlan;
use crate::cypher::plan::ReturnSpec;

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn digest_of(fields: &[&str]) -> String {
    let bytes: Vec<&[u8]> = fields.iter().map(|f| f.as_bytes()).collect();
    Digest256::framed(b"graph-view-fragment", &bytes)
        .map(Digest256::to_hex)
        .unwrap_or_default()
}

fn sparql_text(c: &Compiler<'_>, select: &[String], ret: &ReturnSpec) -> String {
    let mut out: String = c
        .prefixes
        .iter()
        .map(|p| format!("PREFIX {p}: <urn:medulla:view:{p}/>\n"))
        .collect();
    let distinct = if ret.distinct { "DISTINCT " } else { "" };
    out.push_str(&format!("SELECT {distinct}{} WHERE {{\n", select.join(" ")));
    for line in c.triples.iter().chain(&c.filters) {
        out.push_str(&format!("  {line}\n"));
    }
    out.push('}');
    if let Some(limit) = ret.limit {
        out.push_str(&format!(" LIMIT {limit}"));
    }
    out
}

/// Assemble the SPARQL text and the per-fragment digests and statements.
pub(super) fn render(c: &Compiler<'_>, select: &[String], ret: &ReturnSpec) -> ViewPlan {
    let (mapping_digests, pushed_statements) = if c.fragments.is_empty() {
        c.scan_fragment()
    } else {
        c.fragments
            .iter()
            .map(|f| (c.join_digest(f), c.join_statement(f)))
            .unzip()
    };
    ViewPlan {
        sparql: sparql_text(c, select, ret),
        mapping_digests,
        pushed_statements,
    }
}

impl Compiler<'_> {
    /// A pattern with no relationship is one scan of its single mapped label.
    fn scan_fragment(&self) -> (Vec<String>, Vec<String>) {
        self.bound
            .iter()
            .take(1)
            .map(|b| {
                let table = quote_ident(&b.mapping.mapping_id);
                let wheres = self.where_for(&[(&b.var, &table)]);
                let digest = digest_of(&[&self.view.mapping_digest, &b.mapping.mapping_id]);
                (digest, format!("SELECT * FROM {table}{wheres}"))
            })
            .unzip()
    }

    fn join_digest(&self, f: &Fragment<'_>) -> String {
        let ids = f.join.join_mapping_ids.join(",");
        digest_of(&[
            &self.view.mapping_digest,
            &f.join.rel_type,
            &ids,
            &f.join.join_condition,
        ])
    }

    fn table_of(&self, label: &str) -> String {
        self.view
            .label(label)
            .map_or_else(String::new, |m| quote_ident(&m.mapping_id))
    }

    /// One same-source join as one statement, with the filters of its two endpoints.
    fn join_statement(&self, f: &Fragment<'_>) -> String {
        let (left, right) = (
            self.table_of(&f.join.from_label),
            self.table_of(&f.join.to_label),
        );
        let wheres = self.where_for(&[(&f.from, &left), (&f.to, &right)]);
        format!(
            "SELECT * FROM {left} JOIN {right} ON {}{wheres}",
            f.join.join_condition
        )
    }

    fn where_for(&self, owners: &[(&String, &String)]) -> String {
        let terms: Vec<String> = self
            .pushed
            .iter()
            .filter_map(|p| {
                let (_, table) = owners.iter().find(|(v, _)| **v == p.var)?;
                Some(format!(
                    "{table}.{} {} {}",
                    quote_ident(&p.prop),
                    p.op,
                    p.literal
                ))
            })
            .collect();
        if terms.is_empty() {
            return String::new();
        }
        format!(" WHERE {}", terms.join(" AND "))
    }
}
