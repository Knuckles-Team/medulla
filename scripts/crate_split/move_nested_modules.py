#!/usr/bin/env python3
"""Nested-module move codemod for the types split (MEDULLA-CONTRACT-R056.11).

`move_leaf_modules.py` moves whole top-level modules. This tool moves the units that boundaries.toml
assigns to `<to>` under `crates/<from>/src/` at ANY depth (`decision/answer_ranking`, `policy_evolution`).
A unit keeps its module path in the target crate, so `medulla_<from>::decision::answer_ranking::X`
becomes `medulla_<to>::decision::answer_ranking::X`. The target gets a parent module (`decision.rs`)
that declares only the moved children; the source parent keeps the rest.

Path rewriting (hard cutover, no re-exports):

* inside a moved file, `crate::p` / `super::p` that stays inside a moved unit is unchanged; anything
  else becomes an absolute `medulla_<from>::p` path;
* inside a staying file of `<from>`, `crate::p` / `super::p` naming a moved unit becomes
  `medulla_<to>::p`;
* in any other crate, `medulla_<from>::p` naming a moved unit becomes `medulla_<to>::p`.

`--chain a,b,c` is the crate order, lowest first. A reference from a lower crate to a higher one is a
VIOLATION and stops the move (`--plan` prints them).

    move_nested_modules.py --from medulla-decision-types --to medulla-decision-outcome-types \
        --chain medulla-decision-types,medulla-decision-outcome-types [--root R] [--plan]
"""
from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tomllib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from crate_split.graph import blank_non_code, find_use_statements, parse_use_tree  # noqa: E402
from move_leaf_modules import (  # noqa: E402
    ALLOW,
    MOD_DECL,
    add_line,
    forward_features,
    has_dep,
    manifest_for,
    parse_lib,
    snake,
    sync_target_manifest,
)

PATH_RE = re.compile(r"(?<![\w:$])((?:crate|super|medulla_\w+)(?:::[A-Za-z_]\w*)+)")
INLINE_MOD = re.compile(r"\bmod\s+([A-Za-z_]\w*)\s*\{")


def units_of(root: Path, frm: str, to: str) -> list[tuple[str, ...]]:
    cfg = tomllib.loads((root / "scripts/crate_split/boundaries.toml").read_text())
    pre = f"crates/{frm}/src/"
    out = []
    for mv in cfg["crates"][to].get("moves", []):
        if mv.startswith(pre):
            out.append(tuple(mv[len(pre):].removesuffix(".rs").split("/")))
    return sorted(out)


def under(path: tuple[str, ...] | list[str], units) -> bool:
    p = tuple(path)
    return any(p[: len(u)] == u for u in units)


def file_modpath(src: Path, f: Path) -> list[str]:
    parts = list(f.relative_to(src).with_suffix("").parts)
    if parts[-1] in ("mod", "lib") and len(parts) >= 1:
        parts = parts[:-1]
    return parts


def inline_bases(code: str) -> list[tuple[int, int, str]]:
    """(open, close, name) for every inline `mod name { .. }`."""
    spans = []
    for m in INLINE_MOD.finditer(code):
        depth, i = 1, m.end()
        while i < len(code) and depth:
            c = code[i]
            depth += (c == "{") - (c == "}")
            i += 1
        spans.append((m.end(), i, m.group(1)))
    return spans


def base_at(spans, pos: int) -> list[str]:
    return [n for a, b, n in sorted(spans) if a <= pos < b]


def resolve(segs: list[str], base: list[str], frm_s: str):
    """-> (kind, absolute module-path segs) for a path text, or None when it is not crate-relative."""
    h = segs[0]
    if h == "crate":
        return "crate", segs[1:]
    if h == "super":
        n = 0
        while n < len(segs) and segs[n] == "super":
            n += 1
        if n > len(base):
            return None
        return "super", base[: len(base) - n] + segs[n:]
    if h == frm_s:
        return "ext", segs[1:]
    return None


def all_files(root: Path) -> list[Path]:
    rel = subprocess.run(["git", "ls-files", "*.rs"], cwd=root, capture_output=True, text=True).stdout.split()
    return [root / r for r in rel]


def build(root: Path, frm: str, to: str, chain: list[str]):
    src = root / "crates" / frm / "src"
    units = units_of(root, frm, to)
    rank = {c: i for i, c in enumerate(chain)}
    frm_s = snake(frm)
    violations = []
    for f in sorted(src.rglob("*.rs")):
        mp = file_modpath(src, f)
        moving_file = under(mp, units)
        code = blank_non_code(f.read_text())
        spans = inline_bases(code)
        for m in PATH_RE.finditer(code):
            segs = m.group(1).split("::")
            r = resolve(segs, mp + base_at(spans, m.start()), frm_s)
            if not r or r[0] == "ext":
                continue
            dst_moving = under(r[1], units)
            if dst_moving and not moving_file and rank[frm] < rank[to]:
                violations.append(f"VIOLATION: {'::'.join(mp)} stays in {frm} but needs moving {'::'.join(r[1])}")
            if not dst_moving and moving_file and rank[to] < rank[frm]:
                violations.append(f"VIOLATION: {'::'.join(mp)} moves to {to} but needs staying {'::'.join(r[1])}")
        if f.name == "lib.rs" and not mp:
            pass
    return units, sorted(set(violations))


class Mapper:
    def __init__(self, frm: str, to: str, units):
        self.frm, self.to, self.units = frm, to, units
        self.frm_s, self.to_s = snake(frm), snake(to)

    def new(self, segs: list[str], base: list[str], ctx: str):
        """ctx: 'moving' | 'staying' | 'other'. -> new segs or None."""
        r = resolve(segs, base, self.frm_s)
        if not r:
            return None
        kind, absp = r
        mv = under(absp, self.units)
        if ctx == "moving":
            if kind == "ext":
                return [self.to_s] + absp if mv else None
            return None if mv else [self.frm_s] + absp
        if ctx == "staying":
            return [self.to_s] + absp if (mv and kind != "ext") else None
        return [self.to_s] + absp if (mv and kind == "ext") else None

    def rewrite(self, text: str, mp: list[str], ctx: str) -> str:
        code = blank_non_code(text)
        spans = inline_bases(code)
        edits: list[tuple[int, int, str]] = []
        use_spans = []
        for s, e, indent, body in find_use_statements(code):
            use_spans.append((s, e))
            base = mp + base_at(spans, s)
            out, hit = [], False
            for segs, _l, alias in parse_use_tree(body):
                n = self.new(list(segs), base, ctx)
                if n is not None:
                    hit = True
                    out.append((tuple(n), alias))
                else:
                    out.append((tuple(segs), alias))
            if not hit:
                continue
            stmt = text[s:e]
            lead = re.match(r"([ \t]*(?:pub(?:\([^)]*\))?\s+)?use\s+)", stmt).group(1)
            groups: dict[str, list[str]] = {}
            order = []
            for segs, alias in out:
                key, rest = segs[0], "::".join(segs[1:])
                item = (rest if rest else "self") + (f" as {alias}" if alias else "")
                if key not in groups:
                    groups[key] = []
                    order.append(key)
                groups[key].append(item)
            lines = []
            for key in order:
                items = groups[key]
                if items == ["self"]:
                    lines.append(f"{lead}{key};")
                elif len(items) == 1:
                    lines.append(f"{lead}{key}::{items[0]};")
                else:
                    lines.append(f"{lead}{key}::{{{', '.join(items)}}};")
            edits.append((s, e, "\n".join(lines)))
        for m in PATH_RE.finditer(code):
            if any(a <= m.start() < b for a, b in use_spans):
                continue
            n = self.new(m.group(1).split("::"), mp + base_at(spans, m.start()), ctx)
            if n is not None:
                edits.append((m.start(), m.end(), "::".join(n)))
        for s, e, r in sorted(edits, reverse=True):
            text = text[:s] + r + text[e:]
        return text


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--from", dest="frm", required=True)
    ap.add_argument("--to", required=True)
    ap.add_argument("--chain", required=True)
    ap.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    ap.add_argument("--plan", action="store_true")
    a = ap.parse_args()
    root, frm, to = a.root, a.frm, a.to
    chain = a.chain.split(",")
    units, viol = build(root, frm, to, chain)
    for v in viol:
        print(v, file=sys.stderr if not a.plan else sys.stdout)
    if a.plan:
        print(f"{len(units)} units -> {to}; {len(viol)} violations")
        return 0
    if viol:
        return 3
    src, src_to = root / "crates" / frm / "src", root / "crates" / to / "src"
    src_to.mkdir(parents=True, exist_ok=True)
    # 1. git mv each unit (file and dir) and fix the parent module declarations
    moved_decl: dict[tuple[str, ...], list[str]] = {}
    for u in units:
        rel = Path(*u)
        for cand in (src / rel.with_suffix(".rs"), src / rel):
            if cand.exists():
                dest = src_to / cand.relative_to(src)
                dest.parent.mkdir(parents=True, exist_ok=True)
                subprocess.run(["git", "mv", str(cand), str(dest)], check=True, cwd=root)
        parent = u[:-1]
        pf = src / "lib.rs" if not parent else next(
            p for p in (src / Path(*parent).with_suffix(".rs"), src / Path(*parent) / "mod.rs") if p.exists()
        )
        chunks = parse_lib(pf.read_text())
        kept = []
        for n, t in chunks:
            if n == u[-1]:
                t2 = [ln for ln in t if ln.strip() != ALLOW]
                moved_decl.setdefault(parent, []).append("\n".join(t2))
            else:
                kept.append("\n".join(t))
        pf.write_text("\n".join(kept))
    # 2. declarations in the target
    for parent, decls in sorted(moved_decl.items(), key=lambda kv: len(kv[0])):
        body = []
        for d in decls:
            ls = d.split("\n")
            idx = next(i for i, ln in enumerate(ls) if MOD_DECL.match(ln))
            ls.insert(idx, ALLOW)
            body.append("\n".join(ls))
        if not parent:
            lib = src_to / "lib.rs"
            lib.write_text(lib.read_text().rstrip("\n") + "\n\n" + "\n".join(body) + "\n")
        else:
            pf = src_to / Path(*parent).with_suffix(".rs")
            head = f"//! Modules moved here from `{snake(frm)}::{'::'.join(parent)}` (hard cutover, no re-exports).\n\n"
            pf.write_text(head + "\n".join(body) + "\n")
            lib = src_to / "lib.rs"
            if f"mod {parent[0]};" not in lib.read_text():
                lib.write_text(lib.read_text().rstrip("\n") + f"\n\n{ALLOW}\npub mod {parent[0]};\n")
    # 3. rewrite every Rust file
    mp = Mapper(frm, to, units)
    changed_users: dict[Path, str] = {}
    for f in all_files(root) + [p for p in src_to.rglob("*.rs")]:
        if not f.exists():
            continue
        text = f.read_text()
        if f.is_relative_to(src_to):
            ctx, path = "moving", file_modpath(src_to, f)
        elif f.is_relative_to(src):
            ctx, path = "staying", file_modpath(src, f)
        else:
            ctx, path = "other", []
            if snake(frm) not in text:
                continue
        if ctx == "staying" and not under(path, units) and "crate::" not in text and "super::" not in text:
            continue
        new = mp.rewrite(text, path, ctx)
        if new != text:
            f.write_text(new)
            if ctx != "moving" and snake(to) in new:
                m_path, kind = manifest_for(root, f)
                if m_path.parent.name != to and changed_users.get(m_path) != "dep":
                    changed_users[m_path] = kind
    # 4. manifests
    sync_target_manifest(root, frm, to)
    for m_path, kind in changed_users.items():
        t = m_path.read_text()
        if has_dep(t, to):
            continue
        rel = Path(__import__("os").path.relpath(root / "crates" / to, m_path.parent))
        t = add_line(t, "dependencies" if kind == "dep" else "dev-dependencies", f'{to} = {{ path = "{rel}" }}')
        m_path.write_text(t)
    forward_features(root, frm, to, set(changed_users) | {root / "Cargo.toml"})
    print(f"moved {len(units)} units to {to}; users: {len(changed_users)} manifests")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
