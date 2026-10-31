#!/usr/bin/env python3
"""Route every open medulla spec row to its target crate (MEDULLA-CONTRACT-R056.11).

Reads boundaries.toml (crate map, moves, routing rules), placement.toml (file-to-crate
authority for src/) and the open-row census; writes row_routing.jsonl and prints current
and projected lines per crate. Deterministic: same inputs, same output.

  PYTHONPATH=<pipelines> python3 scripts/crate_split/route_rows.py [--census PATH] [--update] [--check]

--update rewrites current_lines/projected_lines in boundaries.toml.
--check exits 1 when any projected crate exceeds the budget or a row is unrouted.
"""
from __future__ import annotations

import argparse
import collections
import fnmatch
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HERE = ROOT / "scripts" / "crate_split"
DEFAULT_CENSUS = "/var/tmp/l9/split/census-medulla.json"
PATH_RE = re.compile(r"(crates/medulla-[\w-]+/src/[\w/]+(?:\.rs)?|(?<![\w/.-])src/[\w/]+(?:\.rs)?)")
ROW_RE = re.compile(r"^\| `(MEDULLA-[A-Z0-9.\-]+)` \| (.*)$")
TITLE_RE = re.compile(r"\*\*(.*?)\*\*")


def load_moves(cfg):
    """Return [(prefix_components, target_crate)] sorted longest first."""
    moves = []
    for crate, spec in cfg["crates"].items():
        for m in spec.get("moves", []):
            target = crate
            if "=" in m:
                m, target = m.split("=", 1)
            moves.append((m.split("/"), target))
    moves.sort(key=lambda x: -len(x[0]))
    return moves


def _match(prefix, parts):
    if len(parts) < len(prefix):
        return False
    for i, comp in enumerate(prefix):
        part = parts[i]
        last = i == len(prefix) - 1
        if last and not comp.endswith(".rs"):
            part = part.removesuffix(".rs")
        if not fnmatch.fnmatchcase(part, comp):
            return False
    return True


def resolve(path, moves, placement):
    parts = path.split("/")
    for prefix, target in moves:
        if _match(prefix, parts):
            return target, prefix
    if parts[0] == "crates" and len(parts) > 1:
        return parts[1], None
    if path in placement:
        return placement[path], None
    for f, c in placement.items():  # a module path: first placed file under it
        if f.startswith(path.removesuffix(".rs") + "/") or f == path.removesuffix(".rs") + ".rs":
            return c, None
    return None, None


def target_module(path, crate, prefix):
    parts = path.removesuffix(".rs").split("/")
    if parts[0] == "crates":
        parts = parts[3:]
    else:
        parts = parts[1:]
    if prefix is not None:
        depth = len(prefix) - (3 if prefix[0] == "crates" else 1) - 1
        parts = parts[max(depth, 0):]
    return f"crates/{crate}/src/" + "/".join(parts[:2])


def measure(moves, placement):
    files = subprocess.run(["git", "ls-files", "*.rs"], cwd=ROOT, capture_output=True,
                           text=True, check=True).stdout.split()
    lines = collections.Counter()
    for f in files:
        p = f.split("/")
        if not ((p[0] == "crates" and len(p) > 3 and p[2] == "src") or f in placement):
            continue
        crate, _ = resolve(f, moves, placement)
        with open(ROOT / f, errors="ignore") as fh:
            lines[crate] += sum(1 for _ in fh)
    return lines


def load_open(census_path):
    """Open = not VERIFIED and not RETIRED, leaf rows only, computed at HEAD.

    Uses the pipelines spec generator (run with PYTHONPATH=<pipelines checkout>); falls
    back to a census JSON whose per-spec `open` lists are taken as given.
    """
    try:
        from pipelines_hooks.specs.generator import generate_at
    except ImportError:
        census = json.load(open(census_path))["medulla"]
        return {s: set(v["open"]) for s, v in census.items()}
    out = {}
    for f, d in generate_at(str(ROOT), "HEAD").items():
        r = d["requirements"]
        it = r.values() if isinstance(r, dict) else r
        out[f.split("/")[1]] = {x["id"] for x in it if not x.get("rollup_of")
                                and x["delivery_state"] not in ("VERIFIED", "RETIRED")}
    return out


def load_rows(open_ids):
    rows = {}
    for req in sorted((ROOT / "specs").glob("*/requirements.md")):
        spec = req.parent.name
        for line in req.read_text().splitlines():
            m = ROW_RE.match(line)
            if m and m.group(1) in open_ids.get(spec, ()):
                rows[m.group(1)] = (spec, m.group(2))
    return rows


def route(row_id, spec, text, cfg, moves, placement):
    t = TITLE_RE.search(text)
    title = (t.group(1) if t else text[:200]).lower()
    for r in cfg.get("route", []):
        if r["spec"] == spec and re.search(r["match"], title):
            return r["crate"], f"crates/{r['crate']}/{r['module']}", f"title~/{r['match'][:40]}/"
    rcfg = cfg.get("routing", {})
    for m in PATH_RE.finditer(text):
        path = m.group(1).rstrip("/")
        if any(path.startswith(sp) for sp in rcfg.get("skip_paths", [])):
            continue  # registry or error-code reference: the work lands elsewhere
        crate, prefix = resolve(path, moves, placement)
        if crate == "medulla-types" and spec in rcfg.get("types_home", {}):
            home = rcfg["types_home"][spec]
            return home, target_module(path, home, None), f"names {path}; new type goes to the spec's types crate"
        if crate and crate in cfg["crates"] and crate != "medulla-engine":
            return crate, target_module(path, crate, prefix), f"names {path}"
        if crate and crate not in cfg["crates"]:  # unchanged leaf crate
            return crate, target_module(path, crate, prefix), f"names {path} (unchanged leaf)"
    d = cfg["defaults"].get(spec)
    if d:
        return d[0], f"crates/{d[0]}/{d[1]}", f"spec default {spec}"
    return None, None, "unrouted"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--census", default=DEFAULT_CENSUS)
    ap.add_argument("--update", action="store_true")
    ap.add_argument("--check", action="store_true")
    a = ap.parse_args()
    cfg = tomllib.loads((HERE / "boundaries.toml").read_text())
    placement = tomllib.loads((HERE / "placement.toml").read_text())["files"]
    moves = load_moves(cfg)
    open_ids = load_open(a.census)
    rows = load_rows(open_ids)
    est = cfg["estimates"]
    budget = est["budget_lines"]
    current = measure(moves, placement)
    added = collections.Counter()
    nrows = collections.Counter()
    out, unrouted = [], []
    missing = sorted(i for s in open_ids.values() for i in s if i not in rows)
    for rid in sorted(rows):
        spec, text = rows[rid]
        crate, module, reason = route(rid, spec, text, cfg, moves, placement)
        if crate is None:
            unrouted.append(rid)
        else:
            nrows[crate] += 1
            added[crate] += est["spec_lines_per_row"].get(spec, est["default_lines_per_row"])
        out.append({"id": rid, "spec": spec, "target_crate": crate,
                    "target_module": module, "reason": reason})
    for rid in missing:
        out.append({"id": rid, "spec": next(s for s, v in open_ids.items() if rid in v),
                    "target_crate": None, "target_module": None,
                    "reason": "row not found in requirements.md"})
        unrouted.append(rid)
    with open(HERE / "row_routing.jsonl", "w") as fh:
        for o in sorted(out, key=lambda o: o["id"]):
            fh.write(json.dumps(o, sort_keys=False) + "\n")
    over = []
    names = sorted(set(cfg["crates"]) | set(current) | set(added))
    for c in names:
        proj = current[c] + added[c]
        flag = ""
        if proj > budget and c in cfg["crates"]:
            flag = "  OVER"
            over.append(c)
        print(f"{c:32} current {current[c]:7} rows {nrows[c]:4} projected {proj:7}{flag}")
    print(f"rows {len(out)} routed {len(out) - len(unrouted)} unrouted {len(unrouted)}")
    if a.update:
        text = (HERE / "boundaries.toml").read_text()
        for c in cfg["crates"]:
            pat = re.compile(r"(\[crates\.%s\]\n(?:(?!\n\[).)*?current_lines = )\d+(\nprojected_lines = )\d+" % re.escape(c), re.S)
            text = pat.sub(lambda m: f"{m.group(1)}{current[c]}{m.group(2)}{current[c] + added[c]}", text)
        (HERE / "boundaries.toml").write_text(text)
    if a.check and (over or unrouted):
        print("over budget:", over, "unrouted:", unrouted[:20])
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
