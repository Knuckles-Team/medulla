"""The incremental-views page states what the source defines (R014)."""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PAGE = ROOT / "docs" / "interfaces" / "incremental-views.md"
KNOBS = ROOT / "crates" / "medulla-server-api" / "src" / "knob_config.rs"
ERRORS = ROOT / "crates" / "medulla-types" / "src" / "contract" / "server_error_code.rs"
SERIES = ROOT / "crates" / "medulla-kernel" / "src" / "metrics" / "series.rs"
CATALOG = ROOT / "crates" / "medulla-sql" / "src" / "sql" / "catalog_matviews.rs"
JOIN = (
    ROOT
    / "crates"
    / "medulla-table-store"
    / "src"
    / "tables"
    / "store"
    / "matview_maintain.rs"
)
MATVIEW = ROOT / "crates" / "medulla-table-store" / "src" / "tables" / "matview.rs"
MKDOCS = ROOT / "mkdocs.yml"


def table(text: str, heading: str) -> list[list[str]]:
    """The body rows of the first table after `## heading`, backticks removed."""
    section = text.split(f"## {heading}", 1)[1]
    rows = []
    for line in section.splitlines()[1:]:
        if line.startswith("## "):
            break
        if line.startswith("|") and not re.match(r"^\|[-| ]+\|$", line):
            rows.append(
                [cell.strip().strip("`") for cell in line.strip("|").split("|")]
            )
    return rows[1:]


def numbers(text: str) -> list[int]:
    return [int(value.replace("_", "")) for value in re.findall(r"\d[\d_]*", text)]


# spec: MEDULLA-INCREMENTAL-VIEWS-R014
def test_page_matches_knobs_codes_and_metrics():
    page = PAGE.read_text()
    knob_source = KNOBS.read_text()
    errors = ERRORS.read_text() + MATVIEW.read_text()
    series = SERIES.read_text()
    catalog = CATALOG.read_text()

    # Every registry knob of the view families is on the page, as sourced.
    registered = {
        name: (numbers(default)[0], numbers(span))
        for name, default, span in re.findall(
            r'"(views?\.[a-z_]+)", U64\(([\d_]+)\), (\[[^\]]+\])', knob_source
        )
    }
    documented = {row[0]: row for row in table(page, "Knobs")}
    for name, (default, span) in registered.items():
        assert name in documented, f"knob {name} is not on the page"
        _, env, shown_default, shown_range, _ = documented[name]
        assert env == "MEDULLA_" + name.replace(".", "_").upper(), name
        assert numbers(shown_default) == [default], name
        assert numbers(shown_range) == span, name
    # The one env-only knob names its source default and range.
    join = JOIN.read_text()
    fanout = documented["view.join_fanout"]
    assert fanout[1] == "MEDULLA_VIEW_JOIN_FANOUT" and fanout[1] in join
    assert numbers(fanout[2]) == [10000] and "const DEFAULT: usize = 10_000" in join
    assert numbers(fanout[3]) == [1, 1000000] and "(1..=1_000_000)" in join
    assert set(documented) == set(registered) | {"view.join_fanout"}

    # Every reason code exists in the error vocabulary.
    codes = [row[0] for row in table(page, "Reason codes")]
    assert codes, "the page lists no reason code"
    for code in codes:
        assert f'"{code}"' in errors, f"{code} is not a defined code"

    # Every view metric of the registry is on the page, with its labels.
    shown = {row[0]: row[1] for row in table(page, "Metrics")}
    defined = re.findall(
        r'"(medulla_views?_[a-z_]+)",\s*"[^"]*",\s*(&\[[^\]]*\])?', series
    )
    assert defined, "no view metric found in the registry"
    for name, labels in defined:
        assert name in shown, f"metric {name} is not on the page"
        expected = re.findall(r'"([a-z_]+)"', labels)
        listed = [
            label.strip("` ") for label in shown[name].split(",") if label.strip()
        ]
        assert listed == (expected or ["none"]), name
    assert set(shown) == {name for name, _ in defined}

    # The catalog columns are the schema's, in order.
    schema = re.findall(
        r'Field::new\("([a-z_]+)"', catalog.split("pub fn matviews_schema")[1]
    )
    assert [
        row[0] for row in table(page, "The `medulla_catalog.matviews` columns")
    ] == schema

    # The page is on the Interfaces nav.
    assert "interfaces/incremental-views.md" in MKDOCS.read_text()
