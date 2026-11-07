"""Fixture-backed regression tests for what the static gates read as source.

Four always-run architecture gates went red on the integration head for one
reason: each pinned an invariant to a single facade *file* or to one exact
attribute *adjacency*, so an ordinary module decomposition (`<name>.rs` ->
`<name>.rs` + `<name>/**`) or an appended attribute read as a deleted
invariant. The repairs move every one of them onto the compiler's own view --
`scripts/rust_module_tree.py`'s declared module walk, and an attribute *block*
rather than an attribute *sequence*. These tests plant the known-bad and
known-good shapes so neither repair can quietly become a hole.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

# Pure/static tests -- never need the shared native engine (see conftest.py's
# session-scoped `start_medulla_server` fixture, which this marker
# exempts this module from triggering).
pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]


def _script(name: str):
    # Gate scripts run as `python3 scripts/<name>.py`, so scripts/ is their
    # sys.path[0] and sibling scanner modules resolve from it.  Reproduce that
    # here instead of relying on another test module having done it first.
    scripts = str(ROOT / "scripts")
    if scripts not in sys.path:
        sys.path.insert(0, scripts)
    path = ROOT / "scripts" / f"{name}.py"
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


# --------------------------------------------------------------------------
# scripts/rust_module_tree.py -- conditional attribute shapes
# --------------------------------------------------------------------------


def _plant_module(root: Path, attribute: str) -> None:
    """A two-file module tree whose facade carries `attribute`."""

    (root / "src").mkdir(parents=True, exist_ok=True)
    (root / "src" / "facade.rs").write_text(
        f"{attribute}\npub struct Planted;\n\npub mod child;\n", encoding="utf-8"
    )
    (root / "src" / "facade").mkdir(exist_ok=True)
    (root / "src" / "facade" / "child.rs").write_text(
        'pub const PLANTED_CHILD_TOKEN: &str = "planted-child";\n', encoding="utf-8"
    )


def test_conditional_derive_is_inert_and_the_child_module_is_still_read(
    tmp_path: Path,
) -> None:
    """Known-good: `cfg_attr(<pred>, derive(...))` must not stop the walk.

    The audited derive expands to impls only, so it cannot move a `mod`
    declaration. Rejecting it made every `crates/medulla-types` module tree
    unreadable (195 occurrences of the audited derives on the integration
    head), which is what kept the gates below pinned to single facade files.
    Unknown proc-macro derives remain rejected because they can emit items.
    """

    walker = _script("rust_module_tree")
    _plant_module(
        tmp_path,
        '#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]',
    )

    source = walker.read_module_tree("src/facade.rs", root_dir=tmp_path)

    assert "PLANTED_CHILD_TOKEN" in source


@pytest.mark.parametrize(
    "derive",
    [
        "schemars::JsonSchema",
        "serde::Serialize",
        "serde::Deserialize",
        "strum::IntoStaticStr",
    ],
)
def test_each_audited_conditional_derive_is_inert(tmp_path: Path, derive: str) -> None:
    walker = _script("rust_module_tree")
    _plant_module(tmp_path, f'#[cfg_attr(feature = "x", derive({derive}))]')

    source = walker.read_module_tree("src/facade.rs", root_dir=tmp_path)

    assert "PLANTED_CHILD_TOKEN" in source


def test_audited_conditional_derives_can_be_combined(tmp_path: Path) -> None:
    walker = _script("rust_module_tree")
    _plant_module(
        tmp_path,
        '#[cfg_attr(feature = "x", derive('
        "schemars::JsonSchema, serde::Serialize, serde::Deserialize, "
        "strum::IntoStaticStr,))]",
    )

    source = walker.read_module_tree("src/facade.rs", root_dir=tmp_path)

    assert "PLANTED_CHILD_TOKEN" in source


@pytest.mark.parametrize(
    "attribute",
    [
        # A conditional `path` really can move the file a `mod` resolves to.
        '#[cfg_attr(feature = "x", path = "elsewhere.rs")]',
        # A nested `cfg_attr` can smuggle either of the above back in.
        '#[cfg_attr(feature = "x", cfg_attr(feature = "y", path = "elsewhere.rs"))]',
        # Not a derive path list: an attribute-macro invocation wearing the
        # `derive` name must not be waved through by the new allowance.
        '#[cfg_attr(feature = "x", derive(schemars::JsonSchema(with = "Vec<u8>")))]',
        '#[cfg_attr(feature = "x", derive("JsonSchema"))]',
        '#[cfg_attr(feature = "x", derive(schemars::JsonSchema, CustomMacro))]',
        # Still-unknown conditional attributes stay rejected.
        '#[cfg_attr(feature = "x", some_unknown_attribute(arg))]',
    ],
)
def test_unproven_conditional_attribute_shapes_still_fail_closed(
    tmp_path: Path, attribute: str
) -> None:
    walker = _script("rust_module_tree")
    _plant_module(tmp_path, attribute)

    with pytest.raises(SystemExit):
        walker.read_module_tree("src/facade.rs", root_dir=tmp_path)


# --------------------------------------------------------------------------
# scripts/check_epistemic_operations_protocol.py -- attribute block, not order
# --------------------------------------------------------------------------


_ATTRIBUTE_BLOCK_CASES = [
    # Known-good: the closure attribute is present, whatever follows it.
    ("#[serde(deny_unknown_fields)]", True),
    (
        "#[serde(deny_unknown_fields)]\n"
        '#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]',
        True,
    ),
    (
        '#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]\n'
        "#[serde(deny_unknown_fields)]",
        True,
    ),
    (
        "/// A doc comment between the attribute and the item.\n"
        "#[serde(deny_unknown_fields)]\n"
        '#[serde(rename_all = "snake_case")]',
        True,
    ),
    # Known-bad: the closure attribute is genuinely absent.
    ('#[serde(rename_all = "snake_case")]', False),
    ('#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]', False),
    ("", False),
]


@pytest.mark.parametrize("attributes,closed", _ATTRIBUTE_BLOCK_CASES)
def test_deny_unknown_fields_is_read_from_the_whole_attribute_block(
    attributes: str, closed: bool
) -> None:
    """Attribute order must not decide whether the DTO looks closed.

    The previous check required `#[serde(deny_unknown_fields)]` to sit
    immediately above `pub struct X {`, so appending the schemars `cfg_attr`
    reported every wire DTO in the result-types `epistemic_operations` module as
    open to unknown fields when nothing about its serde behaviour had changed.
    """

    gate = _script("check_epistemic_operations_protocol")
    source = (
        "#[derive(Clone, Debug, Serialize, Deserialize)]\n"
        f"{attributes}\n"
        "pub struct Planted {\n    pub field: String,\n}\n"
    )
    declaration = source.index("pub struct Planted {")

    block = gate._attribute_block_before(source, declaration)

    assert ("#[serde(deny_unknown_fields)]" in "".join(block.split())) is closed


def test_absent_struct_declaration_is_fatal_not_silently_open() -> None:
    gate = _script("check_epistemic_operations_protocol")

    with pytest.raises(gate.GateError):
        gate._assert_rust_closed([{"rust_type": "NoSuchDtoIsDeclaredAnywhere"}])


def test_epistemic_operations_gate_rejects_an_omitted_child(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Every compiler-declared DTO child must remain in the reviewed inventory."""

    gate = _script("check_epistemic_operations_protocol")
    omitted = "crates/medulla-result-types/src/epistemic_operations/resource_status.rs"
    monkeypatch.setattr(
        gate,
        "EXPECTED_RUST_SOURCES",
        gate.EXPECTED_RUST_SOURCES - {omitted},
    )

    with pytest.raises(gate.GateError, match="compiler module family changed"):
        gate._rust_source()


# --------------------------------------------------------------------------
# The repaired gates themselves
# --------------------------------------------------------------------------


@pytest.mark.parametrize(
    "gate_name",
    [
        "check_epistemic_operations_protocol",
        "check_exact_fault_restart_harness",
    ],
)
def test_repaired_architecture_gate_passes_on_the_current_tree(gate_name: str) -> None:
    gate = _script(gate_name)
    result = gate.main()
    assert result in (None, 0)


_ITEM_POLICY_MACRO = """
macro_rules! request_policy {
    ($request:item) => {
        #[derive(Clone, Debug)]
        #[serde(deny_unknown_fields)]
        $request
    };
}
pub(super) use request_policy;
"""
_ITEM_POLICY_USE = """
super::common::request_policy! {
    pub struct Planted { pub field: String }
}
"""


def _plant_item_policy(gate, tmp_path, monkeypatch, definition, invocation):
    family = tmp_path / "epistemic_operations"
    family.mkdir()
    (family / "common.rs").write_text(definition, encoding="utf-8")
    monkeypatch.setattr(gate, "RUST_SOURCE", tmp_path / "epistemic_operations.rs")
    monkeypatch.setattr(gate, "_rust_source", lambda: invocation)


def test_item_macro_policy_is_resolved_from_its_actual_definition(
    tmp_path, monkeypatch
):
    gate = _script("check_epistemic_operations_protocol")
    _plant_item_policy(
        gate, tmp_path, monkeypatch, _ITEM_POLICY_MACRO, _ITEM_POLICY_USE
    )
    gate._assert_rust_closed([{"rust_type": "Planted"}])


@pytest.mark.parametrize(
    "definition",
    [
        "",
        _ITEM_POLICY_MACRO * 2,
        _ITEM_POLICY_MACRO.replace("request_policy", "renamed_policy"),
        _ITEM_POLICY_MACRO.replace("#[serde(deny_unknown_fields)]", ""),
        _ITEM_POLICY_MACRO.replace("        $request\n", ""),
        _ITEM_POLICY_MACRO.replace(
            "        $request\n", "        pub struct Other {}\n        $request\n"
        ),
        '#[cfg(feature = "optional")]\n' + _ITEM_POLICY_MACRO,
        _ITEM_POLICY_MACRO.replace(
            "#[serde(deny_unknown_fields)]",
            '#[cfg_attr(feature = "optional", serde(deny_unknown_fields))]',
        ),
        _ITEM_POLICY_MACRO.replace("    };\n}", "    };\n    () => {};\n}"),
        _ITEM_POLICY_MACRO.replace("pub(super) use request_policy;", ""),
        _ITEM_POLICY_MACRO.replace(
            "pub(super) use request_policy;",
            "pub(super) use request_policy; pub(super) use request_policy;",
        ),
        _ITEM_POLICY_MACRO.replace(
            "pub(super) use request_policy;",
            '#[cfg(feature = "optional")] pub(super) use request_policy;',
        ),
        _ITEM_POLICY_MACRO.replace(
            "pub(super) use request_policy;",
            "mod hidden { pub(super) use request_policy; }",
        ),
        _ITEM_POLICY_MACRO.replace(
            "#[serde(deny_unknown_fields)]", "// #[serde(deny_unknown_fields)]"
        ),
        _ITEM_POLICY_MACRO.replace(
            "#[serde(deny_unknown_fields)]", '#[doc = "#[serde(deny_unknown_fields)]"]'
        ),
        _ITEM_POLICY_MACRO.replace("$request:item", "$request:expr"),
        "/*" + _ITEM_POLICY_MACRO + "*/",
    ],
)
def test_item_macro_policy_rejects_definition_mutations(
    tmp_path, monkeypatch, definition
):
    gate = _script("check_epistemic_operations_protocol")
    _plant_item_policy(gate, tmp_path, monkeypatch, definition, _ITEM_POLICY_USE)
    with pytest.raises(gate.GateError):
        gate._assert_rust_closed([{"rust_type": "Planted"}])


@pytest.mark.parametrize(
    "invocation",
    [
        _ITEM_POLICY_USE.replace("super::common::", ""),
        _ITEM_POLICY_USE.replace("super::common::", "super::other::"),
        _ITEM_POLICY_USE.replace("request_policy", "unknown_policy"),
        _ITEM_POLICY_USE.replace(
            "pub field: String }", "pub field: String } pub struct Extra {}"
        ),
        "outer! {" + _ITEM_POLICY_USE + "}",
        _ITEM_POLICY_USE.replace("request_policy", "unknown_policy").replace(
            "pub struct", "#[serde(deny_unknown_fields)] pub struct"
        ),
    ],
)
def test_item_macro_policy_rejects_unsupported_invocations(
    tmp_path, monkeypatch, invocation
):
    gate = _script("check_epistemic_operations_protocol")
    _plant_item_policy(gate, tmp_path, monkeypatch, _ITEM_POLICY_MACRO, invocation)
    with pytest.raises(gate.GateError):
        gate._assert_rust_closed([{"rust_type": "Planted"}])


def test_real_required_nullable_requests_remain_closed():
    gate = _script("check_epistemic_operations_protocol")
    gate._assert_rust_closed(
        [
            {"rust_type": name}
            for name in ("ClaimWorkItemRequest", "DevelopmentLaneIntent")
        ]
    )


@pytest.mark.parametrize(
    "attribute",
    [
        "#[cfg(any())]",
        "#[custom_transform]",
        '#[cfg_attr(feature = "optional", custom_transform)]',
        "#[derive(CustomTransform)]",
    ],
)
def test_item_macro_policy_rejects_item_transforming_attributes(
    tmp_path, monkeypatch, attribute
):
    gate = _script("check_epistemic_operations_protocol")
    definition = _ITEM_POLICY_MACRO.replace(
        "        $request", attribute + "\n        $request"
    )
    _plant_item_policy(gate, tmp_path, monkeypatch, definition, _ITEM_POLICY_USE)
    with pytest.raises(gate.GateError):
        gate._assert_rust_closed([{"rust_type": "Planted"}])


@pytest.mark.parametrize(
    "invocation",
    [
        "unknown! ( #[serde(deny_unknown_fields)] pub struct Planted {} );",
        "unknown! [ #[serde(deny_unknown_fields)] pub struct Planted {} ];",
        "unknown! { pub struct Other {} "
        "#[serde(deny_unknown_fields)] pub struct Planted {} }",
        "super::common::request_policy! { pub struct Other {} pub struct Planted {} }",
        "super::common::request_policy! { #[cfg(any())] pub struct Planted {} }",
    ],
)
def test_item_macro_policy_rejects_hidden_item_contexts(
    tmp_path, monkeypatch, invocation
):
    gate = _script("check_epistemic_operations_protocol")
    _plant_item_policy(gate, tmp_path, monkeypatch, _ITEM_POLICY_MACRO, invocation)
    with pytest.raises(gate.GateError):
        gate._assert_rust_closed([{"rust_type": "Planted"}])


@pytest.mark.parametrize(
    "source",
    [
        "// #[serde(deny_unknown_fields)]\npub struct Planted {}",
        "/* #[serde(deny_unknown_fields)] */\npub struct Planted {}",
        '#[doc = "#[serde(deny_unknown_fields)]"]\npub struct Planted {}',
        "#[serde(deny_unknown_fields)] /* pub struct Planted {} */ "
        "pub struct Planted {} pub struct Planted {}",
        "// #[serde(deny_unknown_fields)] pub struct Planted {}",
    ],
)
def test_direct_item_policy_rejects_fake_or_ambiguous_closure(monkeypatch, source):
    gate = _script("check_epistemic_operations_protocol")
    monkeypatch.setattr(gate, "_rust_source", lambda: source)
    with pytest.raises(gate.GateError):
        gate._assert_rust_closed([{"rust_type": "Planted"}])


def test_direct_item_policy_preserves_closure_across_comments(monkeypatch):
    gate = _script("check_epistemic_operations_protocol")
    monkeypatch.setattr(
        gate,
        "_rust_source",
        lambda: (
            "#[serde(deny_unknown_fields)] /* harmless comment */ pub struct Planted {}"
        ),
    )
    gate._assert_rust_closed([{"rust_type": "Planted"}])


@pytest.mark.parametrize(
    "prefix",
    [
        "#[cfg(any())]\n",
        '#[cfg_attr(feature = "optional", custom_transform)]\n',
        "#[custom_transform] /* between attribute and invocation */\n",
        "#[derive(Clone)]\n",
    ],
)
def test_item_macro_policy_rejects_invocation_attributes(tmp_path, monkeypatch, prefix):
    gate = _script("check_epistemic_operations_protocol")
    _plant_item_policy(
        gate, tmp_path, monkeypatch, _ITEM_POLICY_MACRO, prefix + _ITEM_POLICY_USE
    )
    with pytest.raises(gate.GateError):
        gate._assert_rust_closed([{"rust_type": "Planted"}])


@pytest.mark.parametrize(
    "path",
    [
        "super::super::common::request_policy",
        "crate::super::common::request_policy",
        "::super::common::request_policy",
        "wrong::super::common::request_policy",
        "super :: super :: common :: request_policy",
        "super/* path comment */::super::common::request_policy",
        "r#super::common::request_policy",
    ],
)
def test_item_macro_policy_resolves_the_complete_path(tmp_path, monkeypatch, path):
    gate = _script("check_epistemic_operations_protocol")
    invocation = _ITEM_POLICY_USE.replace("super::common::request_policy", path)
    _plant_item_policy(gate, tmp_path, monkeypatch, _ITEM_POLICY_MACRO, invocation)
    with pytest.raises(gate.GateError):
        gate._assert_rust_closed([{"rust_type": "Planted"}])


@pytest.mark.parametrize("prefix", ["", "pub struct Other {}\n", "use other::Thing;\n"])
def test_item_macro_policy_accepts_complete_path_with_trivia(
    tmp_path, monkeypatch, prefix
):
    gate = _script("check_epistemic_operations_protocol")
    invocation = prefix + _ITEM_POLICY_USE.replace(
        "super::common::", "super /* comment */ :: common :: "
    )
    _plant_item_policy(gate, tmp_path, monkeypatch, _ITEM_POLICY_MACRO, invocation)
    gate._assert_rust_closed([{"rust_type": "Planted"}])


@pytest.mark.parametrize(
    "replacement",
    [
        "#[cfg(any())]\nsuper::common::strict_nullable_request!",
        "super::super::common::strict_nullable_request!",
    ],
)
def test_full_gate_rejects_real_invocation_context_mutations(monkeypatch, replacement):
    gate = _script("check_epistemic_operations_protocol")
    source = gate._rust_source().replace(
        "super::common::strict_nullable_request!", replacement
    )
    monkeypatch.setattr(gate, "_rust_source", lambda: source)
    assert gate.main() == 1
