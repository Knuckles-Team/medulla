#!/usr/bin/env python3
"""Fold the G7 manifests into boundaries.toml (MEDULLA-CONTRACT-R056.11): every manifest move
becomes a `moves` prefix of its target crate, new crates get a block, levels are set. Idempotent.
usage (repo root): python3 scripts/crate_split/g7/apply_boundaries.py [source crate ...]
With source crates, only their manifests are folded in and only the crates those manifests
name are touched (earlier lanes' hand-applied deviations stay as they are)."""
import json, re, sys, tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
BT = HERE.parent / "boundaries.toml"
NEW = json.loads((HERE / "new_crates.json").read_text())
text = BT.read_text()
ONLY = set(sys.argv[1:])
mans = [tomllib.loads(p.read_text()) for p in sorted(HERE.glob("medulla-*.toml"))]
if ONLY:
    mans = [m for m in mans if m["source"]["crate"] in ONLY]
    scope = ONLY | {t for m in mans for t in m["targets"]}
    for k in ("crates", "levels", "responsibility", "extra_moves"):
        NEW[k] = {c: v for c, v in NEW[k].items() if c in scope}
    NEW["remove"] = [c for c in NEW["remove"] if c in scope]
srcs = {m["source"]["crate"] for m in mans}
add = {}
for m in mans:
    s = m["source"]
    for mv in m.get("move", []):
        add.setdefault(mv["to"], []).append(mv["path"])
    if s["residual"] != s["crate"]:
        add.setdefault(s["crate"], []).append(f'crates/{s["crate"]}/src={s["residual"]}')
for c, v in NEW["extra_moves"].items():
    add.setdefault(c, []).extend(v)
drop = set(NEW["drop_moves"])


def block_span(name):
    m = re.search(rf"^\[crates\.{re.escape(name)}\]\n", text, re.M)
    if not m:
        return None
    end = re.search(r"^\[", text[m.end():], re.M)
    return m.start(), m.end() + (end.start() if end else len(text) - m.end())


for name in NEW["remove"]:
    sp = block_span(name)
    if sp:
        text = text[:sp[0]] + text[sp[1]:]
for name, b in NEW["crates"].items():
    if not block_span(name):
        sp = block_span(b["after"])
        blk = (f'[crates.{name}]\nlevel = "{b["level"]}"\nkind = "{b["kind"]}"\nresponsibility = {json.dumps(b["responsibility"])}\n'
               f'moves = []\nspecs = {json.dumps(b.get("specs", []))}\ncurrent_lines = 0\nprojected_lines = 0\n\n')
        text = text[:sp[1]] + blk + text[sp[1]:]
cfg = tomllib.loads(text)
for name in set(add) | set(NEW["levels"]) | set(NEW["responsibility"]):
    sp = block_span(name)
    if not sp:
        raise SystemExit(f"no block for {name}")
    blk = text[sp[0]:sp[1]]
    old = cfg["crates"][name].get("moves", [])
    keep = [x for x in old if x not in drop and not any(x.startswith(f"crates/{s}/src") for s in srcs)]
    moves = keep + [x for x in add.get(name, []) if x not in keep]
    blk = re.sub(r"^moves = .*$", "moves = " + json.dumps(moves), blk, flags=re.M)
    if name in NEW["levels"]:
        blk = re.sub(r'^level = .*$', f'level = "{NEW["levels"][name]}"', blk, flags=re.M)
    if name in NEW["responsibility"]:
        blk = re.sub(r'^responsibility = .*$', "responsibility = " + json.dumps(NEW["responsibility"][name]), blk, flags=re.M)
    text = text[:sp[0]] + blk + text[sp[1]:]
chain = NEW["chain"]
if not ONLY:
    text = re.sub(r"^chain = \[.*\]$", "chain = " + json.dumps(chain), text, flags=re.M)
BT.write_text(text)
tomllib.loads(text)
print("ok")
