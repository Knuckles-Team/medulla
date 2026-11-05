"""MEDULLA-DECISION-ENGINE-R178.2: the decision-budget regression check."""

from __future__ import annotations

import json
import re
from pathlib import Path

from _script_loader import load_script

check_decide_budgets = load_script("check_decide_budgets")


def _baseline() -> dict:
    stored = check_decide_budgets.load(check_decide_budgets.DEFAULT_BASELINE)
    stored["groups"] = {
        "decide/route": {"p50_ms": 5.0, "p99_ms": 40.0, "peak_mib": 8.0}
    }
    return stored


def test_planted_regression_fails(tmp_path: Path) -> None:
    # spec: MEDULLA-DECISION-ENGINE-R178.2
    baseline = _baseline()
    steady = {
        "groups": {"decide/route": {"p50_ms": 5.2, "p99_ms": 41.0, "peak_mib": 8.0}}
    }
    assert check_decide_budgets.check(steady, baseline) == []

    planted = {
        "groups": {"decide/route": {"p50_ms": 6.0, "p99_ms": 48.0, "peak_mib": 8.0}}
    }
    findings = check_decide_budgets.check(planted, baseline)
    assert any("p50_ms" in f and "regressed 20.0%" in f for f in findings)
    assert any("p99_ms" in f and "regressed 20.0%" in f for f in findings)

    breach = {
        "groups": {"decide/new": {"p50_ms": 21.0, "p99_ms": 151.0, "peak_mib": 33.0}}
    }
    findings = check_decide_budgets.check(breach, baseline)
    assert len(findings) == 3 and all("exceeds budget" in f for f in findings)

    baseline_path = tmp_path / "baseline.json"
    baseline_path.write_text(json.dumps(baseline), encoding="utf-8")
    report = tmp_path / "run.json"
    report.write_text(json.dumps(planted), encoding="utf-8")
    assert (
        check_decide_budgets.main([str(report), "--baseline", str(baseline_path)]) == 1
    )
    report.write_text(json.dumps(steady), encoding="utf-8")
    assert (
        check_decide_budgets.main([str(report), "--baseline", str(baseline_path)]) == 0
    )


ROOT = Path(__file__).resolve().parents[1]


def _question_kinds() -> set[str]:
    source = (
        ROOT / "crates/medulla-decision-types/src/decision/statistical.rs"
    ).read_text(encoding="utf-8")
    body = source.split("pub enum QuestionKind {", 1)[1].split("\n}", 1)[0]
    variants = re.findall(r"^\s+([A-Z]\w*),\s*$", body, flags=re.MULTILINE)
    return {re.sub(r"(?<!^)([A-Z])", r"_\1", v).lower() for v in variants}


def test_bench_groups_cover_every_family() -> None:
    # spec: MEDULLA-DECISION-ENGINE-R178.1
    bench = (ROOT / "benches/decide_budgets.rs").read_text(encoding="utf-8")
    arms = dict(re.findall(r'QuestionKind::(\w+) => "(\w+)",', bench))
    listed = set(re.findall(r"^\s+QuestionKind::(\w+),\s*$", bench, flags=re.MULTILINE))
    kinds = _question_kinds()
    assert kinds, "no QuestionKind variants parsed"
    assert set(arms.values()) == kinds
    assert listed == set(arms), "FAMILIES must list every named family"
    assert 'format!("decide/{}", family_name(*kind))' in bench
    cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    assert 'name = "decide_budgets"' in cargo


def test_workflow_invokes_the_check() -> None:
    # spec: MEDULLA-DECISION-ENGINE-R178.3
    workflow = (ROOT / ".github/workflows/nightly-decide-budgets.yml").read_text(
        encoding="utf-8"
    )
    assert 'cron: "0 3 * * *"' in workflow
    assert "refs/heads/main" in workflow
    assert "--bench decide_budgets" in workflow
    assert (
        "python scripts/check_decide_budgets.py target/decide-budgets.json" in workflow
    )
    assert "upload-artifact" in workflow
