//! MEDULLA-FEDERATED-QUERY-R742.1: every builtin of SPARQL 1.1 section 17.4 gives the value of
//! its W3C vector, and a wrong-typed argument gives an error value.
//!
//! An error value leaves the `BIND` variable unbound, so a vector whose answer is an error is
//! asserted as `None`. The engine's term model carries the lexical form of a literal, so the
//! `LANG`, `DATATYPE`, `STRDT` and `STRLANG` vectors assert the lexical value and the plain
//! literal answers only.
// spec: MEDULLA-FEDERATED-QUERY-R742.1
#![cfg(feature = "sparql")]

use medulla_graph_core::graph::GraphCore;
use medulla_rdf::sparql::{execute, Dataset, Projection, QueryOutcome};

/// Run `query` over an empty graph and return its solution rows as `var -> lexical` maps.
fn rows(query: &str) -> Vec<std::collections::BTreeMap<String, String>> {
    let view = GraphCore::new().analysis_snapshot();
    let ds = Dataset::new(&view, Vec::new());
    let outcome = execute(&ds, query, &Projection::raw(), None)
        .unwrap_or_else(|error| panic!("{query}: {error}"));
    let QueryOutcome::Solutions(result) = outcome else {
        panic!("{query}: not a SELECT");
    };
    result
        .solutions
        .iter()
        .map(|solution| {
            solution
                .iter()
                .map(|(var, binding)| (var.clone(), binding.as_str().to_string()))
                .collect()
        })
        .collect()
}

/// The value of `BIND(expr AS ?x)` under `prefix`; `None` when `expr` is an error value.
fn scalar_under(prefix: &str, expr: &str) -> Option<String> {
    let found = rows(&format!("{prefix}SELECT ?x WHERE {{ BIND({expr} AS ?x) }}"));
    assert_eq!(found.len(), 1, "{expr}: one row");
    found[0].get("x").cloned()
}

fn scalar(expr: &str) -> Option<String> {
    scalar_under("", expr)
}

fn text(expr: &str) -> String {
    scalar(expr).unwrap_or_else(|| panic!("{expr}: an error value, not a string"))
}

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

#[test]
fn every_builtin_matches_its_w3c_vector() {
    // Terms: STR, LANG, LANGMATCHES, DATATYPE.
    assert_eq!(text(r#"STR("a")"#), "a");
    assert_eq!(text("STR(<http://ex/a>)"), "http://ex/a");
    assert_eq!(text(r#"LANG("abc")"#), "");
    assert_eq!(text(r#"LANGMATCHES("en-GB", "en")"#), "true");
    assert_eq!(text(r#"LANGMATCHES("fr", "en")"#), "false");
    assert_eq!(text(r#"LANGMATCHES("en", "*")"#), "true");
    assert_eq!(text(r#"LANGMATCHES("", "*")"#), "false");
    assert_eq!(text(r#"DATATYPE("abc")"#), format!("<{XSD}string>"));

    // IRI and URI resolve a relative reference against BASE, and need one.
    assert_eq!(text(r#"IRI("http://ex/a")"#), "<http://ex/a>");
    assert_eq!(text(r#"URI("http://ex/a")"#), "<http://ex/a>");
    assert_eq!(text("IRI(<http://ex/a>)"), "<http://ex/a>");
    let base = "BASE <http://ex/dir/>\n";
    assert_eq!(
        scalar_under(base, r#"IRI("b")"#).as_deref(),
        Some("<http://ex/dir/b>")
    );
    assert_eq!(
        scalar_under(base, r#"URI("../c")"#).as_deref(),
        Some("<http://ex/c>")
    );
    assert_eq!(
        scalar_under(base, r#"IRI("/root")"#).as_deref(),
        Some("<http://ex/root>")
    );
    assert_eq!(scalar(r#"IRI("b")"#), None, "a relative reference with no BASE");

    // Constructors: BNODE, STRDT, STRLANG, UUID, STRUUID.
    assert_eq!(text(r#"STR(BNODE("tag"))"#), "_:tag");
    assert_eq!(text("isBlank(BNODE())"), "true");
    assert_eq!(text(&format!(r#"STRDT("42", <{XSD}integer>)"#)), "42");
    assert_eq!(text(r#"STRLANG("chat", "fr")"#), "chat");
    let uuid = text("STR(UUID())");
    assert!(uuid.starts_with("urn:uuid:"), "{uuid}");
    let struuid = text("STRUUID()");
    assert_eq!(struuid.len(), 36, "{struuid}");
    assert_eq!(struuid.as_bytes()[14], b'4', "version 4: {struuid}");
    assert!(uuid.trim_start_matches("urn:uuid:").as_bytes()[14] == b'4');

    // Strings.
    assert_eq!(text(r#"STRLEN("héllo")"#), "5");
    assert_eq!(text(r#"SUBSTR("foobar", 4)"#), "bar");
    assert_eq!(text(r#"SUBSTR("foobar", 4, 1)"#), "b");
    assert_eq!(text(r#"SUBSTR("abcdef", 0, 3)"#), "ab");
    assert_eq!(text(r#"SUBSTR("foobar", -1)"#), "foobar");
    assert_eq!(text(r#"SUBSTR("foobar", 10)"#), "");
    assert_eq!(text(r#"UCASE("abC")"#), "ABC");
    assert_eq!(text(r#"LCASE("abC")"#), "abc");
    assert_eq!(text(r#"STRSTARTS("foobar", "foo")"#), "true");
    assert_eq!(text(r#"STRSTARTS("foobar", "bar")"#), "false");
    assert_eq!(text(r#"STRENDS("foobar", "bar")"#), "true");
    assert_eq!(text(r#"CONTAINS("foobar", "oba")"#), "true");
    assert_eq!(text(r#"CONTAINS("foobar", "xyz")"#), "false");
    assert_eq!(text(r#"STRBEFORE("abc", "b")"#), "a");
    assert_eq!(text(r#"STRBEFORE("abc", "xyz")"#), "");
    assert_eq!(text(r#"STRAFTER("abc", "b")"#), "c");
    assert_eq!(text(r#"STRAFTER("abc", "xyz")"#), "");
    assert_eq!(text(r#"ENCODE_FOR_URI("Los Angeles")"#), "Los%20Angeles");
    assert_eq!(text(r#"ENCODE_FOR_URI("Münster")"#), "M%C3%BCnster");
    assert_eq!(text(r#"ENCODE_FOR_URI("a-b_c.d~e")"#), "a-b_c.d~e");
    assert_eq!(text(r#"CONCAT("foo", "bar")"#), "foobar");
    assert_eq!(text(r#"CONCAT("a", "b", "c")"#), "abc");
    assert_eq!(text(r#"REPLACE("abracadabra", "bra", "*")"#), "a*cada*");
    assert_eq!(text(r#"REPLACE("abracadabra", "a.*a", "*")"#), "*");
    assert_eq!(text(r#"REPLACE("abracadabra", "a.*?a", "*")"#), "*c*bra");
    assert_eq!(
        text(r#"REPLACE("abracadabra", "a(.)", "a$1$1")"#),
        "abbraccaddabbra"
    );
    assert_eq!(text(r#"REPLACE("Foo", "o", "0", "i")"#), "F00");
    assert_eq!(
        scalar(r#"REPLACE("abcd", ".*", "Z")"#),
        None,
        "a pattern that matches the empty string"
    );
    assert_eq!(text(r#"REGEX("abracadabra", "bra")"#), "true");
    assert_eq!(text(r#"REGEX("abracadabra", "^bra")"#), "false");
    assert_eq!(text(r#"REGEX("Hello", "hello", "i")"#), "true");

    // Numerics.
    assert_eq!(text("ABS(-1.5)"), "1.5");
    assert_eq!(text("ROUND(2.5)"), "3");
    assert_eq!(text("ROUND(-2.5)"), "-2");
    assert_eq!(text("CEIL(10.5)"), "11");
    assert_eq!(text("FLOOR(10.5)"), "10");
    let rand: f64 = text("RAND()").parse().unwrap();
    assert!((0.0..1.0).contains(&rand), "RAND() = {rand}");

    // Dates and times.
    let dt = r#""2011-01-10T14:45:13.815-05:00""#;
    assert_eq!(text(&format!("YEAR({dt})")), "2011");
    assert_eq!(text(&format!("MONTH({dt})")), "1");
    assert_eq!(text(&format!("DAY({dt})")), "10");
    assert_eq!(text(&format!("HOURS({dt})")), "14");
    assert_eq!(text(&format!("MINUTES({dt})")), "45");
    assert_eq!(text(&format!("SECONDS({dt})")), "13.815");
    assert_eq!(text(&format!("TZ({dt})")), "-05:00");
    assert_eq!(text(&format!("TIMEZONE({dt})")), "-PT5H");
    assert_eq!(text(r#"TZ("2011-01-10T14:45:13Z")"#), "Z");
    assert_eq!(text(r#"TIMEZONE("2011-01-10T14:45:13Z")"#), "PT0S");

    // Functional forms.
    assert_eq!(text(r#"COALESCE(?unbound, "x")"#), "x");
    assert_eq!(text(r#"IF(1 < 2, "yes", "no")"#), "yes");
    assert_eq!(text(r#"IF(1 > 2, "yes", "no")"#), "no");
}

#[cfg(feature = "sparql-hash")]
#[test]
fn hash_builtins_match_their_vectors() {
    assert_eq!(text(r#"MD5("abc")"#), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(
        text(r#"SHA1("abc")"#),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
    assert_eq!(
        text(r#"SHA256("abc")"#),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        text(r#"SHA384("abc")"#),
        "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
    );
    assert_eq!(
        text(r#"SHA512("abc")"#),
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );
    assert_eq!(scalar("SHA256(<http://ex/a>)"), None, "an IRI is not a string");
}

#[test]
fn a_function_on_a_wrong_typed_argument_is_an_error_value() {
    for expr in [
        "STRLEN(<http://ex/a>)",
        "UCASE(<http://ex/a>)",
        "LCASE(<http://ex/a>)",
        r#"SUBSTR(<http://ex/a>, 1)"#,
        r#"STRSTARTS(<http://ex/a>, "http")"#,
        r#"CONTAINS(<http://ex/a>, "ex")"#,
        r#"CONCAT("a", <http://ex/a>)"#,
        r#"REGEX(<http://ex/a>, "ex")"#,
        "ENCODE_FOR_URI(<http://ex/a>)",
        r#"YEAR("not a date")"#,
        r#"ABS("not a number")"#,
    ] {
        assert_eq!(scalar(expr), None, "{expr}");
    }
    // A FILTER treats the error value as false, so no row survives.
    let kept = rows(r#"SELECT ?x WHERE { BIND(1 AS ?x) FILTER(STRSTARTS(<http://ex/a>, "http")) }"#);
    assert!(kept.is_empty(), "{kept:?}");
}

#[test]
fn now_is_one_value_for_the_whole_query_and_uuids_are_fresh_per_row() {
    let found = rows(
        "SELECT ?i ?a ?b ?u WHERE { VALUES ?i { 1 2 3 4 5 } \
         BIND(NOW() AS ?a) BIND(NOW() AS ?b) BIND(STRUUID() AS ?u) }",
    );
    assert_eq!(found.len(), 5);
    let instant = &found[0]["a"];
    assert!(found.iter().all(|row| &row["a"] == instant && &row["b"] == instant));
    let mut uuids: Vec<&String> = found.iter().map(|row| &row["u"]).collect();
    uuids.sort();
    uuids.dedup();
    assert_eq!(uuids.len(), 5, "STRUUID() is fresh per row");
}
