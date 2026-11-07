//! Fail when `mint_policy_decision_lease` grows a second production call site.
//!
//! The `MintAuthorization` hardening of the policy-lease contract closes the caller-supplied-claims
//! gap by requiring a cryptographic capability token instead of a raw claims reference. That
//! control is only as good as the claim that exactly one production code path ever calls
//! `IsolationLayer::mint_policy_decision_lease`, and that the path already holds the server auth
//! secret at the point it has verified claims in hand. A second production call site would be a NEW
//! place that must independently get the construction right, and nothing else in the build would
//! notice if it did not. This gate is the notice.
//!
//! The invariant is a property, not a file path: the sole production caller lives in the
//! compiler-declared module tree of the router's dispatch subsystem (the one that owns request
//! authorization and routing), and the function containing it constructs its `MintAuthorization`
//! from the server auth secret and the verified claims. A decomposition that relocates the caller
//! inside that tree keeps passing; a caller anywhere else, or one that skips the MAC construction,
//! fails closed. Test-only call sites are expected and permitted.

use std::collections::BTreeSet;
use std::path::Path;

use regex::Regex;

use crate::analysis::module_tree;

pub const NAME: &str = "mint-lease-call-sites";
pub const SUMMARY: &str =
    "mint_policy_decision_lease has one production call site that builds its own MintAuthorization";

/// The one subsystem permitted to hold the production call site, named by its module root.
const ALLOWED_PRODUCTION_CALL_MODULE: &str = "crates/medulla-server-router/src/dispatch.rs";

/// What the audit of the call site rests on: the enclosing function builds the capability token
/// itself, from the server auth secret and the verified claims it already holds.
const REQUIRED_MINT_AUTHORIZATION_TOKENS: [&str; 9] = [
    "let (auth_secret, isolation)",
    "MintAuthorization::compute_mac(",
    "MintAuthorization::new(",
    "auth_secret",
    "verified_context.claims()",
    "CarrierAuthority::from_verified(",
    "AccessLevel::Read",
    "KnowledgeStreamAuthority::from_verified_with_lease(",
    "policy_store()",
];

/// Returned when no `fn` item encloses the call site; the gate fails on it rather than reading the
/// whole file and handing the audit a neighbour's tokens.
const NO_ENCLOSING_FUNCTION: &str = "<no enclosing fn>";

const SCAN_DIRS: [&str; 2] = ["src", "crates"];

pub fn run(root: &Path, _args: &[String]) -> Result<(), String> {
    let message = check(root)?;
    println!("{message}");
    Ok(())
}

fn is_test_only(relative_posix: &str) -> bool {
    let test_only = Regex::new(
        r"(^|/)(tests?)/.*\.rs$|(^|/)tests\.rs$|(^|/)test_[^/]*\.rs$|(^|/)[^/]*_tests?\.rs$",
    )
    .expect("static regex");
    test_only.is_match(relative_posix)
}

fn is_commented_out(line: &str) -> bool {
    let stripped = line.trim();
    stripped.starts_with("//") || stripped.starts_with('*')
}

/// Every real `.mint_policy_decision_lease(` call under `root`'s Rust sources, as sorted
/// `(path, 1-based line)` pairs.
fn find_call_sites(root: &Path) -> Result<Vec<(String, usize)>, String> {
    let call = Regex::new(r"\.mint_policy_decision_lease\s*\(").expect("static regex");
    let mut sites = Vec::new();
    for dir in SCAN_DIRS {
        let base = root.join(dir);
        if !base.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&base)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() || entry.path().extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(entry.path())
                .map_err(|e| format!("cannot read {relative}: {e}"))?;
            for (index, line) in text.lines().enumerate() {
                if call.is_match(line) && !is_commented_out(line) {
                    sites.push((relative.clone(), index + 1));
                }
            }
        }
    }
    sites.sort();
    Ok(sites)
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Index of the line that opens the `fn` body, or None for a bodiless declaration.
fn body_opening(lines: &[&str], declaration: usize) -> Option<usize> {
    for (index, line) in lines.iter().enumerate().skip(declaration) {
        let stripped = line.trim_end();
        if stripped.contains('{') {
            return Some(index);
        }
        if stripped.ends_with(';') {
            return None;
        }
    }
    None
}

/// Exclusive end line of the `fn` declared at `declaration`: rustfmt closes it with a `}` at the
/// declaration's own indentation, so the first non-blank line after the opening brace that is not
/// indented deeper ends the body.
fn function_span(lines: &[&str], declaration: usize, indentation: usize) -> usize {
    let Some(opening) = body_opening(lines, declaration) else {
        return declaration + 1;
    };
    if lines[opening].trim_end().ends_with('}') {
        return opening + 1;
    }
    for (index, line) in lines.iter().enumerate().skip(opening + 1) {
        if !line.trim().is_empty() && indent(line) <= indentation {
            return index + 1;
        }
    }
    lines.len()
}

/// The `fn` item containing 1-based `line_no`, as `(name, body)`. `fn` items are recognised at any
/// nesting depth (impl method, nested `mod`, trait default body); each candidate's window is its
/// own body, so a call is never attributed to a neighbouring item. When several contain the call
/// the OUTERMOST wins: the audited window never shrinks below the function the reviewer read.
fn enclosing_function(text: &str, line_no: usize) -> (String, String) {
    let item_fn = Regex::new(
        r#"^[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)"#,
    )
    .expect("static regex");
    let lines: Vec<&str> = text.lines().collect();
    let target = line_no - 1;
    let outermost = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let name = item_fn.captures(line)?[1].to_string();
            let end = function_span(&lines, index, indent(line));
            (index <= target && target < end).then_some((index, end, name))
        })
        .min();
    match outermost {
        None => (NO_ENCLOSING_FUNCTION.to_string(), String::new()),
        Some((start, end, name)) => (name, format!("{}\n", lines[start..end].join("\n"))),
    }
}

/// The single production call site, proven to live in the owning module's declared tree.
fn require_sole_owning_call_site(
    root: &Path,
    production: &[(String, usize)],
) -> Result<(String, usize), String> {
    if production.len() != 1 {
        let rendered = production
            .iter()
            .map(|(p, l)| format!("{p}:{l}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "mint-lease call-site gate failed: expected exactly ONE production call site (in the {ALLOWED_PRODUCTION_CALL_MODULE} module), found {}: [{}]. A second production caller of mint_policy_decision_lease means a second place that must independently get MintAuthorization construction right \u{2014} audit it before letting this pass.",
            production.len(),
            if rendered.is_empty() { "none" } else { &rendered }
        ));
    }
    let (path, line) = production[0].clone();
    let canonical = root.canonicalize().map_err(|e| e.to_string())?;
    let owning: BTreeSet<String> =
        module_tree::load(root, ALLOWED_PRODUCTION_CALL_MODULE, true, false)?
            .paths
            .iter()
            .filter_map(|p| p.strip_prefix(&canonical).ok())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
    if !owning.contains(&path) {
        return Err(format!(
            "mint-lease call-site gate failed: the sole production call site is {path}:{line}, outside the {ALLOWED_PRODUCTION_CALL_MODULE} module that owns request authorization. Widening the allowed module is a policy decision, not a path fix: the new owner must independently hold the server auth secret and the verified claims at the call."
        ));
    }
    Ok((path, line))
}

/// Prove the call site builds its own `MintAuthorization`; return its `fn`.
fn require_self_constructed_authorization(
    root: &Path,
    relative: &str,
    line_no: usize,
) -> Result<String, String> {
    let text = std::fs::read_to_string(root.join(relative))
        .map_err(|e| format!("cannot read {relative}: {e}"))?;
    let (function, body) = enclosing_function(&text, line_no);
    if function == NO_ENCLOSING_FUNCTION {
        return Err(format!(
            "mint-lease call-site gate failed: the sole production call site {relative}:{line_no} is not inside any `fn` item, so there is no function whose MintAuthorization construction can be audited."
        ));
    }
    let missing: Vec<&str> = REQUIRED_MINT_AUTHORIZATION_TOKENS
        .iter()
        .copied()
        .filter(|t| !body.contains(t))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "mint-lease call-site gate failed: the sole production call site {relative}:{line_no} (in {function}) does not construct its own MintAuthorization \u{2014} missing {missing:?}. A caller-supplied capability token is exactly the gap the policy-lease contract closes."
        ));
    }
    Ok(function)
}

fn check(root: &Path) -> Result<String, String> {
    let sites = find_call_sites(root)?;
    if sites.is_empty() {
        return Err("mint-lease call-site gate failed: found ZERO calls to mint_policy_decision_lease \u{2014} either the scan paths drifted or the feature was deleted; update this gate deliberately if so, don't let it go quietly blind".into());
    }
    let (test_only, production): (Vec<_>, Vec<_>) =
        sites.into_iter().partition(|(path, _)| is_test_only(path));
    let (path, line) = require_sole_owning_call_site(root, &production)?;
    let function = require_self_constructed_authorization(root, &path, line)?;
    Ok(format!(
        "mint-lease call-site gate passed: 1 production call site ({path}:{line} in {function}), {} test-only call site(s)",
        test_only.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;

    const PLANTED_CALLER: &str = "async fn unrelated_earlier_item() {\n    let _ = 1;\n}\n\nasync fn dispatch_governed_stream_write_methods() {\n{BODY}\n}\n\nasync fn unrelated_later_item() {\n    let _ = isolation.mint_policy_decision_lease(&smuggled, &graph, read);\n}\n";

    const AUDITED_BODY: &str = "    let (auth_secret, isolation) = load(state).await;\n    let mint_auth = MintAuthorization::compute_mac(\n        &auth_secret,\n        verified_context.claims(),\n    )\n    .and_then(|mac| {\n        MintAuthorization::new(&auth_secret, verified_context.claims(), &mac)\n    })?;\n    let carrier = CarrierAuthority::from_verified(verified_context)?;\n    let lease =\n        isolation.mint_policy_decision_lease(&mint_auth, &graph, AccessLevel::Read)?;\n    let authority = KnowledgeStreamAuthority::from_verified_with_lease(\n        &auth_secret,\n        verified_context.claims(),\n        &graph,\n        &carrier,\n        lease,\n        isolation.policy_store()?,\n    )?;\n";

    const CALLER_SUPPLIED_BODY: &str =
        "    let lease = isolation.mint_policy_decision_lease(&caller_token, &graph, read)?;\n";

    fn call_line(source: &str) -> usize {
        source[..source.find(".mint_policy_decision_lease(").unwrap()]
            .matches('\n')
            .count()
            + 1
    }

    fn satisfied(window: &str) -> bool {
        REQUIRED_MINT_AUTHORIZATION_TOKENS
            .iter()
            .all(|t| window.contains(t))
    }

    #[test]
    fn the_audit_reads_the_enclosing_function_not_the_file() {
        for (body, audited) in [(AUDITED_BODY, true), (CALLER_SUPPLIED_BODY, false)] {
            let source = PLANTED_CALLER.replace("{BODY}", body);
            let (name, window) = enclosing_function(&source, call_line(&source));
            assert_eq!(name, "dispatch_governed_stream_write_methods");
            assert!(!window.contains("unrelated_later_item"));
            assert_eq!(satisfied(&window), audited);
        }
    }

    const IMPL_METHOD_CALLER: &str = "fn audited_free_function() {\n    let (auth_secret, verified) = load(state);\n    let mint_auth = MintAuthorization::compute_mac(&auth_secret, verified.claims())\n        .and_then(|mac| MintAuthorization::new(&auth_secret, verified.claims(), &mac))?;\n}\n\nimpl Router {\n    async fn smuggled_impl_method(&self) -> Result<()> {\n        let lease = isolation.mint_policy_decision_lease(&caller_token, &graph, read)?;\n        Ok(())\n    }\n}\n";

    const NESTED_MODULE_CALLER: &str = "fn audited_free_function() {\n    let mint_auth = MintAuthorization::compute_mac(&auth_secret, verified.claims())\n        .and_then(|mac| MintAuthorization::new(&auth_secret, verified.claims(), &mac))?;\n}\n\nmod outer {\n    pub mod inner {\n        pub async fn smuggled_nested_module_fn() {\n            let lease = isolation.mint_policy_decision_lease(&caller_token, &graph,\n            read);\n        }\n    }\n}\n";

    /// Known-bad: an indented `fn` must not borrow a free function's tokens.
    #[test]
    fn a_call_nested_below_column_zero_resolves_to_its_own_function() {
        for (source, expected) in [
            (IMPL_METHOD_CALLER, "smuggled_impl_method"),
            (NESTED_MODULE_CALLER, "smuggled_nested_module_fn"),
        ] {
            let (name, window) = enclosing_function(source, call_line(source));
            assert_eq!(name, expected);
            assert!(!window.contains("audited_free_function"));
            assert!(!satisfied(&window));
        }
    }

    const WRAPPED_SIGNATURE_CALLER: &str = "async fn dispatch_governed_stream_write_methods(\n    state: &ServerState,\n    graph: &GraphName,\n) -> Result<Lease> {\n    let (auth_secret, isolation) = load(state).await;\n    let mint_auth = MintAuthorization::compute_mac(\n        &auth_secret,\n        verified_context.claims(),\n    )\n    .and_then(|mac| {\n        MintAuthorization::new(&auth_secret, verified_context.claims(), &mac)\n    })?;\n    let carrier = CarrierAuthority::from_verified(verified_context)?;\n    let lease =\n        isolation.mint_policy_decision_lease(&mint_auth, &graph, AccessLevel::Read)?;\n    KnowledgeStreamAuthority::from_verified_with_lease(\n        &auth_secret,\n        verified_context.claims(),\n        &graph,\n        &carrier,\n        lease,\n        isolation.policy_store()?,\n    )\n}\n";

    /// Known-good: a multi-line signature must not truncate the window.
    #[test]
    fn a_wrapped_signature_does_not_truncate_the_window() {
        let source = WRAPPED_SIGNATURE_CALLER;
        let (name, window) = enclosing_function(source, call_line(source));
        assert_eq!(name, "dispatch_governed_stream_write_methods");
        assert!(satisfied(&window));
    }

    #[test]
    fn a_call_outside_any_fn_is_unattributable() {
        let source = "static S: X = isolation.mint_policy_decision_lease(&t);\n";
        assert_eq!(enclosing_function(source, 1).0, NO_ENCLOSING_FUNCTION);
    }

    /// Known-bad: two production call sites fail, not pick one.
    #[test]
    fn a_second_production_caller_is_rejected() {
        let t = Tree::new(&[
            ("src/server/dispatch.rs", "pub mod router;\n"),
            ("src/server/dispatch/router.rs", WRAPPED_SIGNATURE_CALLER),
            ("src/admin.rs", IMPL_METHOD_CALLER),
        ]);
        assert!(check(t.path())
            .unwrap_err()
            .contains("expected exactly ONE production call site"));
    }

    #[test]
    fn a_sole_caller_outside_the_owning_module_is_rejected() {
        let t = Tree::new(&[
            (
                "crates/medulla-server-router/src/dispatch.rs",
                "pub mod router;\n",
            ),
            ("crates/medulla-server-router/src/dispatch/router.rs", ""),
            ("crates/other/src/lib.rs", WRAPPED_SIGNATURE_CALLER),
        ]);
        assert!(check(t.path()).unwrap_err().contains("outside the"));
    }

    #[test]
    fn a_relocated_caller_inside_the_owning_module_passes_and_test_sites_are_counted() {
        let t = Tree::new(&[
            (
                "crates/medulla-server-router/src/dispatch.rs",
                "pub mod router;\n",
            ),
            (
                "crates/medulla-server-router/src/dispatch/router.rs",
                WRAPPED_SIGNATURE_CALLER,
            ),
            (
                "crates/medulla-isolation/src/tests.rs",
                "fn t() { x.mint_policy_decision_lease(&a); }\n",
            ),
            (
                "crates/medulla-isolation/src/doc.rs",
                "// x.mint_policy_decision_lease(&a)\n",
            ),
        ]);
        let message = check(t.path()).unwrap();
        assert!(
            message.contains("1 production call site")
                && message.contains("1 test-only call site(s)"),
            "{message}"
        );
    }

    #[test]
    fn zero_call_sites_is_blindness_not_a_pass() {
        let t = Tree::new(&[("src/lib.rs", "")]);
        assert!(check(t.path()).unwrap_err().contains("ZERO calls"));
    }

    #[test]
    fn passes_on_the_current_tree() {
        check(&crate::workspace_root()).unwrap();
    }
}
