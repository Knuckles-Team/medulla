//! The RDF term a solution binds to a variable (MEDULLA-FEDERATED-QUERY-R742.3).
//!
//! A binding is the term itself: an IRI, a blank node, a literal with its lexical form and
//! its datatype or language tag, or a quoted triple. No consumer recovers a term by parsing
//! a lexical form, so `LANG`, `DATATYPE`, `STRLANG` and `STRDT` read what the literal holds.

use std::cmp::Ordering;

/// The XML Schema namespace.
pub const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
/// `xsd:string`, the datatype of a plain literal.
pub const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
/// `xsd:integer`.
pub const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
/// `xsd:decimal`.
pub const XSD_DECIMAL: &str = "http://www.w3.org/2001/XMLSchema#decimal";
/// `xsd:double`.
pub const XSD_DOUBLE: &str = "http://www.w3.org/2001/XMLSchema#double";
/// `xsd:float`.
pub const XSD_FLOAT: &str = "http://www.w3.org/2001/XMLSchema#float";
/// `xsd:boolean`.
pub const XSD_BOOLEAN: &str = "http://www.w3.org/2001/XMLSchema#boolean";
/// `xsd:dateTime`.
pub const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";
/// `rdf:langString`, the datatype of a language-tagged literal.
pub const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";

/// The integer-derived datatypes (`xsd:integer` and the types derived from it).
const INTEGER_DERIVED: [&str; 12] = [
    "integer",
    "int",
    "long",
    "short",
    "byte",
    "nonNegativeInteger",
    "positiveInteger",
    "nonPositiveInteger",
    "negativeInteger",
    "unsignedLong",
    "unsignedInt",
    "unsignedShort",
];

/// A literal: its lexical form and the datatype or language tag that give it a value.
/// A plain literal has datatype `xsd:string`; a language-tagged one has `rdf:langString`
/// and a lower-case tag.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Literal {
    /// The lexical form.
    pub lexical: String,
    /// The datatype IRI, without angle brackets.
    pub datatype: String,
    /// The language tag, lower case; present only when the datatype is `rdf:langString`.
    pub language: Option<String>,
}

impl Literal {
    /// A plain (`xsd:string`) literal.
    pub fn string(lexical: impl Into<String>) -> Self {
        Self {
            lexical: lexical.into(),
            datatype: XSD_STRING.to_string(),
            language: None,
        }
    }

    /// A language-tagged literal. The tag is lower-cased.
    pub fn lang(lexical: impl Into<String>, tag: &str) -> Self {
        Self {
            lexical: lexical.into(),
            datatype: RDF_LANG_STRING.to_string(),
            language: Some(tag.to_ascii_lowercase()),
        }
    }

    /// A typed literal. `rdf:langString` needs a tag, so it is never built here.
    pub fn typed(lexical: impl Into<String>, datatype: impl Into<String>) -> Self {
        Self {
            lexical: lexical.into(),
            datatype: datatype.into(),
            language: None,
        }
    }

    /// Whether this is a simple (`xsd:string`) or language-tagged literal: the only literals
    /// a string builtin accepts.
    pub fn is_string_like(&self) -> bool {
        self.datatype == XSD_STRING || self.language.is_some()
    }

    /// The numeric value, when the datatype is numeric and the lexical form is valid for it.
    pub fn numeric(&self) -> Option<Numeric> {
        let local = self.datatype.strip_prefix(XSD)?;
        if INTEGER_DERIVED.contains(&local) {
            return self
                .lexical
                .trim()
                .parse::<i64>()
                .ok()
                .map(Numeric::Integer);
        }
        match local {
            "decimal" => {
                let text = self.lexical.trim();
                if text.contains(['e', 'E']) {
                    return None;
                }
                text.parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(Numeric::Decimal)
            }
            "double" | "float" => parse_double(self.lexical.trim()).map(Numeric::Double),
            _ => None,
        }
    }

    /// The boolean value of an `xsd:boolean` literal.
    pub fn boolean(&self) -> Option<bool> {
        if self.datatype != XSD_BOOLEAN {
            return None;
        }
        match self.lexical.trim() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        }
    }
}

fn parse_double(text: &str) -> Option<f64> {
    match text {
        "INF" | "+INF" => Some(f64::INFINITY),
        "-INF" => Some(f64::NEG_INFINITY),
        "NaN" => Some(f64::NAN),
        other => other.parse::<f64>().ok(),
    }
}

/// A numeric value of the XPath tower, kept apart so an operation promotes by type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Numeric {
    /// An integer-derived value.
    Integer(i64),
    /// An `xsd:decimal` value.
    Decimal(f64),
    /// An `xsd:double` or `xsd:float` value.
    Double(f64),
}

impl Numeric {
    /// The value as a double.
    pub fn as_f64(self) -> f64 {
        match self {
            Numeric::Integer(n) => n as f64,
            Numeric::Decimal(v) | Numeric::Double(v) => v,
        }
    }

    /// The typed term of this value.
    pub fn into_binding(self) -> Binding {
        match self {
            Numeric::Integer(n) => Binding::integer(n),
            Numeric::Decimal(v) => Binding::decimal(v),
            Numeric::Double(v) => Binding::double(v),
        }
    }

    /// `self op other` under type promotion: two integers stay integers (a division gives a
    /// decimal), a double with anything is a double, every other pair is a decimal. A
    /// division by zero is an error value (`None`).
    pub fn arith(self, op: ArithOp, other: Numeric) -> Option<Numeric> {
        if matches!(op, ArithOp::Div) && other.as_f64() == 0.0 {
            return None;
        }
        if let (Numeric::Integer(a), Numeric::Integer(b)) = (self, other) {
            let exact = match op {
                ArithOp::Add => a.checked_add(b),
                ArithOp::Sub => a.checked_sub(b),
                ArithOp::Mul => a.checked_mul(b),
                ArithOp::Div => None,
            };
            if let Some(n) = exact {
                return Some(Numeric::Integer(n));
            }
        }
        let (a, b) = (self.as_f64(), other.as_f64());
        let value = match op {
            ArithOp::Add => a + b,
            ArithOp::Sub => a - b,
            ArithOp::Mul => a * b,
            ArithOp::Div => a / b,
        };
        Some(
            if matches!(self, Numeric::Double(_)) || matches!(other, Numeric::Double(_)) {
                Numeric::Double(value)
            } else {
                Numeric::Decimal(value)
            },
        )
    }

    /// The same numeric type with `value` (the result of `ABS`, `ROUND`, `CEIL`, `FLOOR`).
    pub fn with_value(self, value: f64) -> Numeric {
        match self {
            Numeric::Integer(_) => Numeric::Integer(value as i64),
            Numeric::Decimal(_) => Numeric::Decimal(value),
            Numeric::Double(_) => Numeric::Double(value),
        }
    }
}

/// The four arithmetic operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
}

/// A solution binding: an RDF term.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Binding {
    /// An IRI, without angle brackets.
    Iri(String),
    /// A blank node, the label without `_:`.
    Blank(String),
    /// A literal.
    Literal(Literal),
    /// A quoted triple (subject, predicate, object).
    Triple(Box<[Binding; 3]>),
}

impl Binding {
    /// An IRI term.
    pub fn iri(iri: impl Into<String>) -> Self {
        Binding::Iri(iri.into())
    }

    /// A blank-node term.
    pub fn blank(label: impl Into<String>) -> Self {
        Binding::Blank(label.into())
    }

    /// A plain string literal.
    pub fn string(lexical: impl Into<String>) -> Self {
        Binding::Literal(Literal::string(lexical))
    }

    /// A language-tagged literal.
    pub fn lang(lexical: impl Into<String>, tag: &str) -> Self {
        Binding::Literal(Literal::lang(lexical, tag))
    }

    /// A typed literal.
    pub fn typed(lexical: impl Into<String>, datatype: impl Into<String>) -> Self {
        Binding::Literal(Literal::typed(lexical, datatype))
    }

    /// An `xsd:integer` literal.
    pub fn integer(value: i64) -> Self {
        Binding::typed(value.to_string(), XSD_INTEGER)
    }

    /// An `xsd:decimal` literal.
    pub fn decimal(value: f64) -> Self {
        Binding::typed(fmt_num(value), XSD_DECIMAL)
    }

    /// An `xsd:double` literal.
    pub fn double(value: f64) -> Self {
        Binding::typed(fmt_num(value), XSD_DOUBLE)
    }

    /// An `xsd:boolean` literal.
    pub fn boolean(value: bool) -> Self {
        Binding::typed(if value { "true" } else { "false" }, XSD_BOOLEAN)
    }

    /// The term a graph node key names: `<iri>` is an IRI, `_:label` a blank node, and a key
    /// kept bare by the identity projection is an IRI of that text.
    pub fn from_node_key(key: &str) -> Self {
        if let Some(iri) = key.strip_prefix('<').and_then(|k| k.strip_suffix('>')) {
            Binding::Iri(iri.to_string())
        } else if let Some(label) = key.strip_prefix("_:") {
            Binding::Blank(label.to_string())
        } else {
            Binding::Iri(key.to_string())
        }
    }

    /// The term of a stored property cell: a typed RDF cell (`value`, `datatype`, `lang`)
    /// keeps its datatype and tag; a native scalar is `xsd:integer` for an integer,
    /// `xsd:double` for another number, `xsd:boolean` for a boolean and `xsd:string` for a
    /// string. An array, a null or an object with no `value` has no term.
    pub fn from_cell(cell: &serde_json::Value) -> Option<Self> {
        match cell {
            serde_json::Value::Object(m) => {
                let lexical = m.get("value")?.as_str()?;
                if let Some(tag) = m.get("lang").and_then(|l| l.as_str()) {
                    return Some(Binding::lang(lexical, tag));
                }
                let datatype = m
                    .get("datatype")
                    .and_then(|d| d.as_str())
                    .unwrap_or(XSD_STRING);
                Some(Binding::typed(lexical, datatype))
            }
            serde_json::Value::String(text) => Some(Binding::string(text.as_str())),
            serde_json::Value::Number(n) if n.is_i64() || n.is_u64() => {
                Some(Binding::typed(n.to_string(), XSD_INTEGER))
            }
            serde_json::Value::Number(n) => Some(Binding::typed(n.to_string(), XSD_DOUBLE)),
            serde_json::Value::Bool(b) => Some(Binding::boolean(*b)),
            _ => None,
        }
    }

    /// The term of an `oxrdf` subject.
    pub fn from_subject(subject: &oxrdf::NamedOrBlankNode) -> Self {
        match subject {
            oxrdf::NamedOrBlankNode::NamedNode(n) => Binding::iri(n.as_str()),
            oxrdf::NamedOrBlankNode::BlankNode(b) => Binding::blank(b.as_str()),
        }
    }

    /// The term of an `oxrdf` term; `None` for a form this build does not model.
    pub fn from_term(term: &oxrdf::Term) -> Option<Self> {
        match term {
            oxrdf::Term::NamedNode(n) => Some(Binding::iri(n.as_str())),
            oxrdf::Term::BlankNode(b) => Some(Binding::blank(b.as_str())),
            oxrdf::Term::Literal(l) => Some(Binding::from_oxrdf_literal(l)),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    /// The term of an `oxrdf` literal.
    pub fn from_oxrdf_literal(literal: &oxrdf::Literal) -> Self {
        match literal.language() {
            Some(tag) => Binding::lang(literal.value(), tag),
            None => Binding::typed(literal.value(), literal.datatype().as_str()),
        }
    }

    /// The term of a query literal, keeping its datatype and tag.
    pub fn from_query_literal(literal: &spargebra::term::Literal) -> Self {
        match literal.language() {
            Some(tag) => Binding::lang(literal.value(), tag),
            None => Binding::typed(literal.value(), literal.datatype().as_str()),
        }
    }

    /// The `oxrdf` term of this binding; `None` for a quoted triple or a malformed IRI,
    /// blank-node label or tag.
    pub fn to_oxrdf_term(&self) -> Option<oxrdf::Term> {
        match self {
            Binding::Iri(iri) => Some(oxrdf::Term::NamedNode(oxrdf::NamedNode::new(iri).ok()?)),
            Binding::Blank(label) => {
                Some(oxrdf::Term::BlankNode(oxrdf::BlankNode::new(label).ok()?))
            }
            Binding::Literal(l) => Some(oxrdf::Term::Literal(match &l.language {
                Some(tag) => {
                    oxrdf::Literal::new_language_tagged_literal(l.lexical.as_str(), tag.as_str())
                        .ok()?
                }
                None => oxrdf::Literal::new_typed_literal(
                    l.lexical.as_str(),
                    oxrdf::NamedNode::new(l.datatype.as_str()).ok()?,
                ),
            })),
            Binding::Triple(_) => None,
        }
    }

    /// The graph node keys this term can name, most specific first: `<iri>` then the bare
    /// text for an IRI, `_:label` for a blank node. A literal names no node.
    pub fn node_keys(&self) -> Vec<String> {
        match self {
            Binding::Iri(iri) => vec![format!("<{iri}>"), iri.clone()],
            Binding::Blank(label) => vec![format!("_:{label}")],
            Binding::Literal(_) | Binding::Triple(_) => Vec::new(),
        }
    }

    /// The canonical graph node key of an IRI or blank node.
    pub fn node_key(&self) -> Option<String> {
        self.node_keys().into_iter().next()
    }

    /// Whether this is an IRI.
    pub fn is_iri(&self) -> bool {
        matches!(self, Binding::Iri(_))
    }

    /// Whether this is a blank node.
    pub fn is_blank(&self) -> bool {
        matches!(self, Binding::Blank(_))
    }

    /// Whether this is a literal.
    pub fn is_literal(&self) -> bool {
        matches!(self, Binding::Literal(_))
    }

    /// Whether this is a quoted triple.
    pub fn is_triple(&self) -> bool {
        matches!(self, Binding::Triple(_))
    }

    /// Whether this term is a node of the graph (an IRI or a blank node).
    pub fn is_resource(&self) -> bool {
        matches!(self, Binding::Iri(_) | Binding::Blank(_))
    }

    /// The literal, if this is one.
    pub fn as_literal(&self) -> Option<&Literal> {
        match self {
            Binding::Literal(l) => Some(l),
            _ => None,
        }
    }

    /// The lexical form of a string-like literal; `None` for any other term. A string
    /// builtin reads its argument through this.
    pub fn string_value(&self) -> Option<&str> {
        match self {
            Binding::Literal(l) if l.is_string_like() => Some(l.lexical.as_str()),
            _ => None,
        }
    }

    /// The numeric value of a numeric literal.
    pub fn numeric(&self) -> Option<Numeric> {
        self.as_literal().and_then(Literal::numeric)
    }

    /// The `STR()` value: the IRI text or the lexical form. A blank node and a quoted
    /// triple have none (an error value).
    pub fn str_value(&self) -> Option<String> {
        match self {
            Binding::Iri(iri) => Some(iri.clone()),
            Binding::Literal(l) => Some(l.lexical.clone()),
            Binding::Blank(_) | Binding::Triple(_) => None,
        }
    }

    /// The value text: the IRI, the blank-node label, the lexical form, or the quoted triple
    /// in N-Triples-star form. The CSV serialization writes this.
    pub fn value_text(&self) -> String {
        match self {
            Binding::Iri(iri) => iri.clone(),
            Binding::Blank(label) => label.clone(),
            Binding::Literal(l) => l.lexical.clone(),
            Binding::Triple(_) => self.to_ntriples(),
        }
    }

    /// A one-line rendering for diagnostics and assertions: an IRI as `<iri>`, a blank node as
    /// `_:label`, a literal as its lexical form. It is not a term and nothing parses it.
    pub fn display_text(&self) -> String {
        match self {
            Binding::Literal(l) => l.lexical.clone(),
            other => other.to_ntriples(),
        }
    }

    /// The N-Triples form: `<iri>`, `_:label`, `"lexical"`, `"lexical"@tag` or
    /// `"lexical"^^<datatype>`. Also the stable key a solution is deduplicated and grouped by.
    pub fn to_ntriples(&self) -> String {
        match self {
            Binding::Iri(iri) => format!("<{iri}>"),
            Binding::Blank(label) => format!("_:{label}"),
            Binding::Literal(l) => {
                let quoted = escape_literal(&l.lexical);
                match (&l.language, l.datatype.as_str()) {
                    (Some(tag), _) => format!("\"{quoted}\"@{tag}"),
                    (None, XSD_STRING) => format!("\"{quoted}\""),
                    (None, datatype) => format!("\"{quoted}\"^^<{datatype}>"),
                }
            }
            Binding::Triple(parts) => format!(
                "<< {} {} {} >>",
                parts[0].to_ntriples(),
                parts[1].to_ntriples(),
                parts[2].to_ntriples()
            ),
        }
    }
}

fn escape_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

/// A number as a lexical form, without a trailing `.0` for an integral value.
pub fn fmt_num(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9.0e15 {
        format!("{}", n as i64)
    } else if n.is_nan() {
        "NaN".to_string()
    } else if n.is_infinite() {
        if n > 0.0 { "INF" } else { "-INF" }.to_string()
    } else {
        format!("{n}")
    }
}

/// The effective boolean value of a term (SPARQL 1.1 section 17.2.2): a boolean is its value,
/// a string is non-empty, a number is non-zero and not NaN. Any other term is an error value.
pub fn ebv(b: &Binding) -> Option<bool> {
    let literal = b.as_literal()?;
    if let Some(value) = literal.boolean() {
        return Some(value);
    }
    if literal.datatype == XSD_BOOLEAN {
        return None;
    }
    if let Some(number) = literal.numeric() {
        let v = number.as_f64();
        return Some(v != 0.0 && !v.is_nan());
    }
    if literal.is_string_like() {
        return Some(!literal.lexical.is_empty());
    }
    None
}

/// `=` over two terms: numerics compare by value, every other pair by term identity.
pub fn term_equal(a: &Binding, b: &Binding) -> bool {
    match (a.numeric(), b.numeric()) {
        (Some(x), Some(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

/// The ordering of two literals of comparable kinds: numerics by value, strings by code
/// point, booleans false before true, and dateTime lexically. `None` when the kinds differ.
pub fn literal_compare(a: &Literal, b: &Literal) -> Option<Ordering> {
    if let (Some(x), Some(y)) = (a.numeric(), b.numeric()) {
        return x.as_f64().partial_cmp(&y.as_f64());
    }
    if let (Some(x), Some(y)) = (a.boolean(), b.boolean()) {
        return Some(x.cmp(&y));
    }
    if a.datatype == XSD_DATETIME && b.datatype == XSD_DATETIME {
        return Some(a.lexical.cmp(&b.lexical));
    }
    if a.is_string_like() && b.is_string_like() {
        return Some(a.lexical.cmp(&b.lexical));
    }
    None
}

/// `<`, `>`, `<=` and `>=` over two terms: defined only for comparable literals.
pub fn term_compare(a: &Binding, b: &Binding) -> Option<Ordering> {
    literal_compare(a.as_literal()?, b.as_literal()?)
}
