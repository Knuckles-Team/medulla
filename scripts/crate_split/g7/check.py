#!/usr/bin/env python3
"""Check a G7 manifest (MEDULLA-CONTRACT-R056.11): every cross-target module edge must point to a
strictly lower rank once the manifest's item cuts are applied. Prints lines per target, the SCCs
of the source crate and each violating edge with the items it names. Run from the repo root:

    python3 scripts/crate_split/g7/check.py scripts/crate_split/g7/<crate>.toml
Exit 1 on any violation.
"""
import json, subprocess, sys, tomllib
from pathlib import Path

man = tomllib.loads(Path(sys.argv[1]).read_text())
src = man["source"]
here = Path(__file__).resolve().parent
g = json.loads(subprocess.run([sys.executable, str(here / "modgraph.py"), src["crate"], *src["expand"]],
                              capture_output=True, text=True, check=True).stdout)
L, E = g["lines"], g["edges"]
rank = {t: v["rank"] for t, v in man["targets"].items()}
T = {u: src["residual"] for u in L}
for mv in man.get("move", []):
    T[mv["unit"]] = mv["to"]
covered = set()
for c in man.get("cut", []):
    for it in c.get("items", []):
        covered.add(f'{c["from"].split(" ")[0]}::{it}')
    covered.add(c["from"])
    covered.update(c.get("covers", []))
tot = {}
for u, t in T.items():
    tot[t] = tot.get(t, 0) + L.get(u, 0)
for t in sorted(tot, key=lambda t: rank.get(t, 0)):
    print(f"  {t:32} rank {rank.get(t, 0)} lines {tot[t]}")
bad = 0
for u, ds in E.items():
    for v, items in ds.items():
        if v not in T or T[u] == T[v] or rank.get(T[v], 0) < rank.get(T[u], 0):
            continue
        left = [i for i in items if f"{v}::{i}" not in covered and f"{v}::{i.split('::')[0]}" not in covered]
        if left:
            bad += 1
            print(f"VIOLATION {u} [{T[u]}] -> {v} [{T[v]}]: {left[:12]}")
print("violations", bad)
sys.exit(1 if bad else 0)
