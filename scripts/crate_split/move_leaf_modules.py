#!/usr/bin/env python3
"""Leaf-crate move codemod for the types split (MEDULLA-CONTRACT-R056.11).

Moves the top-level modules of `crates/<from>/src` that boundaries.toml assigns to `<to>` (a
`moves` prefix names `<p>.rs` and `<p>/**`; `*` globs one component; the longest prefix wins)
into `crates/<to>/src`, carries each `mod` declaration with its attributes and comments to the
target lib.rs, drops the lib.rs `pub use` aliases of moved modules (hard cutover) and rewrites
paths workspace-wide:

* inside moved files `crate::<m>` stays when `<m>` moved to the same crate, becomes
  `medulla_types::<m>` for items that stay in the thin crate;
* everywhere else `medulla_types::<m>::..` (or a removed root alias) becomes
  `medulla_<to>::<m>::..`.

The type crates form one chain (`[types_split].chain`): a crate may depend on any crate
earlier in it. Crates move top-down (the protocol hub first, identity last), so each move
leaves a compiling workspace; a dependency that runs up the chain is reported as a
VIOLATION and stops the move (`--plan` prints them). Cargo manifests get
the new dependency, forwarded features and the used third-party dependencies.

    move_leaf_modules.py --from medulla-types --to medulla-decision-types [--root R] [--plan]
"""
from __future__ import annotations

import argparse
import fnmatch
import re
import subprocess
import sys
import tomllib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from crate_split.graph import blank_non_code, find_use_statements, parse_use_tree  # noqa: E402

MOD_DECL = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;")
CRATE_PATH = re.compile(r"(?<![$\w:])crate((?:::[A-Za-z_]\w*)+)")
DCRATE_PATH = re.compile(r"\$crate((?:::[A-Za-z_]\w*)+)")
ALLOW = "#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)"


def snake(c: str) -> str:
    return c.replace("-", "_")


# ----------------------------------------------------------------------------- plan
def load_patterns(root: Path, frm: str) -> list[tuple[str, str]]:
    cfg = tomllib.loads((root / "scripts/crate_split/boundaries.toml").read_text())
    pre = f"crates/{frm}/src/"
    out = []
    for name, c in cfg["crates"].items():
        for mv in c.get("moves", []):
            if mv.startswith(pre):
                out.append((name, mv[len(pre):].removesuffix(".rs")))
    return out


def desired(mod: str, pats: list[tuple[str, str]]) -> str | None:
    best, who = (-1, 0), None
    for crate, p in pats:
        if "/" in p:
            continue
        if fnmatch.fnmatchcase(mod, p):
            key = (len(p.replace("*", "")), p.count("*") == 0)
            if key > best:
                best, who = key, crate
    return who


def list_mods(src: Path) -> dict[str, list[Path]]:
    out: dict[str, list[Path]] = {}
    if not src.is_dir():
        return out
    # files a top-level module includes with `#[path = "x.rs"]` belong to that module
    included = {m for f in src.glob("*.rs") for m in re.findall(r'#\[path\s*=\s*"([^"/]+\.rs)"\]', f.read_text())}
    for e in sorted(src.iterdir()):
        if e.name == "lib.rs" or e.name in included:
            continue
        if e.is_file() and e.suffix == ".rs":
            out.setdefault(e.stem, []).append(e)
        elif e.is_dir():
            out.setdefault(e.name, []).append(e)
    return out


def files_of(paths: list[Path]) -> list[Path]:
    fs = []
    for p in paths:
        if p.is_file():
            fs += [p.parent / m for m in re.findall(r'#\[path\s*=\s*"([^"/]+\.rs)"\]', p.read_text())]
        fs += [p] if p.is_file() else sorted(p.rglob("*.rs"))
    return fs


def parse_lib(text: str):
    """chunks: list of (modname|None, text) ; carries attrs/comments above each mod decl."""
    lines = text.split("\n")
    chunks: list[tuple[str | None, list[str]]] = []
    buf: list[str] = []
    depth = 0
    for ln in lines:
        s = ln.strip()
        if depth > 0:
            buf.append(ln)
            depth += s.count("[") - s.count("]")
            continue
        if s.startswith("//") or s.startswith("#["):
            buf.append(ln)
            if s.startswith("#["):
                depth = s.count("[") - s.count("]")
            continue
        m = MOD_DECL.match(ln)
        if m:
            chunks.append((m.group(1), buf + [ln]))
            buf = []
            continue
        if buf:
            chunks.append((None, buf))
            buf = []
        chunks.append((None, [ln]))
    if buf:
        chunks.append((None, buf))
    return chunks


def root_names(lib_text: str) -> dict[str, str]:
    """`pub use m::{A,B}` aliases in lib.rs: exported name -> module."""
    code = blank_non_code(lib_text)
    out: dict[str, str] = {}
    for _s, _e, indent, body in find_use_statements(code):
        if indent:
            continue
        for segs, _l, alias in parse_use_tree(body):
            if segs and segs[-1] != "*" and len(segs) >= 2 and segs[0] not in ("crate", "self", "super", "std"):
                out[alias or segs[-1]] = segs[0]
    return out


def mod_refs(files: list[Path], mods: set[str], rn: dict[str, str]) -> set[str]:
    refs: set[str] = set()
    for f in files:
        code = blank_non_code(f.read_text())
        segsets = [m.group(1).split("::")[1:] for m in CRATE_PATH.finditer(code)]
        segsets += [m.group(1).split("::")[1:] for m in DCRATE_PATH.finditer(code)]
        for _s, _e, _i, body in find_use_statements(code):
            for segs, _l, _a in parse_use_tree(body):
                if segs and segs[0] == "crate":
                    segsets.append(list(segs[1:]))
        for segs in segsets:
            if not segs:
                continue
            h = segs[0]
            if h in mods:
                refs.add(h)
            elif h in rn and rn[h] in mods:
                refs.add(rn[h])
    return refs


def load_chain(root: Path) -> list[str]:
    cfg = tomllib.loads((root / "scripts/crate_split/boundaries.toml").read_text())
    return cfg["types_split"]["chain"]


def plan(root: Path, frm: str, to: str, chain: list[str] | None = None, bottom_up: bool = False):
    pats = load_patterns(root, frm)
    src = root / "crates" / frm / "src"
    mods = list_mods(src)
    lib_text = (src / "lib.rs").read_text()
    rn = root_names(lib_text)
    assign: dict[str, str] = {}  # module -> crate (frm = stays)
    for m in mods:
        assign[m] = desired(m, pats) or frm
    # modules already moved to other types crates
    for crate, _p in {(c, 0) for c, _ in pats if c != frm}:
        for m in list_mods(root / "crates" / crate / "src"):
            assign[m] = crate
    allmods = set(assign)
    # deps (only modules still in frm, or L1 crates, both scanned)
    deps: dict[str, set[str]] = {}
    for m in allmods:
        paths = mods.get(m) or list_mods(root / "crates" / assign[m] / "src").get(m, [])
        deps[m] = mod_refs(files_of(paths), allmods, rn) - {m}
    # lib.rs non-use code ($crate refs in macros) counts as thin
    lib_code = "\n".join("\n".join(t) for n, t in parse_lib(lib_text) if n is None)
    lib_deps = mod_refs_text(lib_code, allmods, rn)
    # The type crates form one chain (boundaries.toml [types_split].chain): a crate may
    # depend on any crate earlier in it. Crates move top-down (the hub first), so every
    # module still in <from> belongs to a crate at or below <to>. `phys` is where each
    # module lives once <to> has moved; it, not the target map, drives the rewrite.
    chain = chain or load_chain(root)
    rank = {c: i for i, c in enumerate(chain)}
    rank_to = rank[to]
    moving = {m for m in mods if assign[m] == to}
    phys = {m: (to if m in moving else (frm if m in mods else assign[m])) for m in allmods}
    pinned: dict[str, str] = {}
    for m, ds in deps.items():
        for d in ds:
            if bottom_up:
                # lowest crate first: a moving module may use only crates already moved out
                # (below it); modules staying in <from> may use the moved ones freely.
                if m in moving and phys[d] == frm:
                    pinned[d] = f"VIOLATION: moving {m} needs {d}, which stays in {frm} (target {assign[d]})"
                elif m in moving and phys[d] not in (frm, to) and rank.get(phys[d], 99) > rank_to:
                    pinned[d] = f"VIOLATION: moving {m} needs {d} in higher {phys[d]}"
                continue
            if m in moving and phys[d] not in (frm, to) and rank.get(phys[d], 99) > rank_to:
                pinned[d] = f"VIOLATION: moving {m} needs {d} in higher {phys[d]}"
            if m in mods and m not in moving and d in moving:
                pinned[m] = f"VIOLATION: {m} stays in {frm} (target {assign[m]}) but needs moving {d}"
            if m in moving and phys[d] == frm and rank.get(assign[d], 0) > rank_to:
                pinned[d] = f"VIOLATION: moving {m} needs {d} whose target {assign[d]} is above {to}"
    assign = phys
    return assign, pinned, deps, rn, mods, lib_text


def mod_refs_text(code_text: str, mods: set[str], rn: dict[str, str]) -> set[str]:
    code = blank_non_code(code_text)
    out = set()
    for m in list(CRATE_PATH.finditer(code)) + list(DCRATE_PATH.finditer(code)):
        segs = m.group(1).split("::")[1:]
        if segs and segs[0] in mods:
            out.add(segs[0])
    return out


def count_lines(paths: list[Path]) -> int:
    return sum(len(f.read_text().split("\n")) for f in files_of(paths))


# ----------------------------------------------------------------------------- rewrite
def attrs_above(text: str, pos: int) -> tuple[int, str]:
    """start offset of contiguous single-line #[..] attribute lines directly above `pos`."""
    start = pos
    while start > 0:
        prev = text.rfind("\n", 0, start - 1) + 1
        line = text[prev:start - 1] if text[start - 1:start] == "\n" else text[prev:start]
        if line.strip().startswith("#[") and line.strip().endswith("]"):
            start = prev
        else:
            break
    return start, text[start:pos]


class Rewriter:
    def __init__(self, to: str, frm: str, assign: dict[str, str], rn: dict[str, str]):
        self.to, self.frm, self.assign, self.rn = to, frm, assign, rn
        self.moved = {m for m, c in assign.items() if c == to}
        self.touched_crates: set[str] = set()

    def resolve(self, segs: list[str]):
        """segs after the types crate -> (module, target-crate) for moved items, else None."""
        if not segs:
            return None
        h = segs[0]
        if h in self.assign and self.assign[h] != self.frm:
            return h, self.assign[h], segs
        if h in self.rn and self.rn[h] in self.assign and self.assign[self.rn[h]] != self.frm:
            m = self.rn[h]
            return m, self.assign[m], [m] + segs
        return None

    # path rewriting for moved crate only (the crate being created now)
    def new_path_external(self, segs: list[str]):
        r = self.resolve(segs)
        if r and r[1] == self.to:
            return snake(self.to) + "::" + "::".join(r[2])
        return None

    def new_path_inside(self, segs: list[str]):
        """for `crate::segs` inside moved files."""
        if not segs:
            return None
        h = segs[0]
        if h in self.assign:
            c = self.assign[h]
            if c == self.to:
                return None
            return snake(c) + "::" + "::".join(segs)
        if h in self.rn and self.rn[h] in self.assign:
            m = self.rn[h]
            c = self.assign[m]
            if c == self.to:
                return "crate::" + "::".join([m] + segs)
            return snake(c) + "::" + "::".join([m] + segs)
        return snake(self.frm) + "::" + "::".join(segs)

    def rewrite(self, text: str, inside: bool) -> str:
        code = blank_non_code(text)
        edits: list[tuple[int, int, str]] = []
        spans = []
        head = "crate" if inside else snake(self.frm)
        for s, e, indent, body in find_use_statements(code):
            spans.append((s, e))
            leaves = parse_use_tree(body)
            hit = False
            out: list[tuple[tuple[str, ...], str | None]] = []
            for segs, _l, alias in leaves:
                nsegs = segs
                if segs and segs[0] == head and len(segs) >= 2:
                    new = self.new_path_inside(list(segs[1:])) if inside else self.new_path_external(list(segs[1:]))
                    if new:
                        nsegs = tuple(new.split("::"))
                        hit = True
                out.append((nsegs, alias))
            if not hit:
                continue
            stmt = text[s:e]
            lead = re.match(r"([ \t]*(?:pub(?:\([^)]*\))?\s+)?use\s+)", stmt).group(1)
            a_start, attrs = attrs_above(text, s)
            groups: dict[str, list[str]] = {}
            order = []
            for segs, alias in out:
                key = segs[0]
                rest = "::".join(segs[1:])
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
            sep = "\n" + attrs
            edits.append((s, e, sep.join(lines)))
        pat = CRATE_PATH if inside else re.compile(r"(?<![\w:])" + snake(self.frm) + r"((?:::[A-Za-z_]\w*)+)")
        for m in pat.finditer(code):
            if any(a <= m.start() < b for a, b in spans):
                continue
            segs = m.group(1).split("::")[1:]
            new = self.new_path_inside(segs) if inside else self.new_path_external(segs)
            if new:
                edits.append((m.start(), m.end(), new))
        if inside:
            for m in DCRATE_PATH.finditer(code):
                segs = m.group(1).split("::")[1:]
                if segs and segs[0] in self.assign and self.assign[segs[0]] != self.to:
                    edits.append((m.start(), m.end(), "::" + snake(self.assign[segs[0]]) + m.group(1)))
                elif segs and segs[0] not in self.assign:
                    edits.append((m.start(), m.end(), "::" + snake(self.frm) + m.group(1)))
        for s, e, r in sorted(edits, reverse=True):
            text = text[:s] + r + text[e:]
        return text


# ----------------------------------------------------------------------------- manifests
def section(text: str, header: str):
    m = re.search(r"(?m)^\[" + re.escape(header) + r"\]\s*$", text)
    if not m:
        return None
    n = re.search(r"(?m)^\[", text[m.end():])
    return m.end(), (m.end() + n.start()) if n else len(text)


def add_line(text: str, header: str, line: str) -> str:
    b = section(text, header)
    if b is None:
        return text.rstrip("\n") + f"\n\n[{header}]\n{line}\n"
    head, tail = text[: b[1]].rstrip("\n"), text[b[1]:]
    return head + "\n" + line + "\n" + ("\n" if tail.strip() else "") + tail.lstrip("\n")


def has_dep(text: str, name: str) -> bool:
    d = tomllib.loads(text)
    return any(name in d.get(k, {}) for k in ("dependencies", "dev-dependencies", "build-dependencies"))


FEAT = re.compile(r'feature\s*=\s*"([A-Za-z0-9_-]+)"')


def sync_target_manifest(root: Path, frm: str, to: str):
    cdir = root / "crates" / to
    fman = tomllib.loads((root / "crates" / frm / "Cargo.toml").read_text())
    ct_p = cdir / "Cargo.toml"
    ct = ct_p.read_text()
    srcs = "\n".join(blank_non_code(f.read_text()) for f in cdir.rglob("*.rs"))
    raw = "\n".join(f.read_text() for f in cdir.rglob("*.rs"))
    uses_types = re.search(r"\b" + snake(frm) + r"\b", srcs) is not None
    if uses_types and not has_dep(ct, frm):
        feats_needed = "test-support" if False else None
        ct = add_line(ct, "dependencies", f'{frm} = {{ path = "../{frm}" }}')
    ct_text = open(root / "crates" / frm / "Cargo.toml").read()
    for dep, spec in fman.get("dependencies", {}).items():
        if dep == frm or has_dep(ct, dep):
            continue
        ident = snake(dep)
        if re.search(r"\b" + ident + r"\b", srcs) or (dep == "schemars" and "JsonSchema" in raw) or (dep == "strum" and "strum" in raw):
            m = re.search(r"(?m)^" + re.escape(dep) + r"\s*=.*$", ct_text)
            ct = add_line(ct, "dependencies", m.group(0))
    names = set(FEAT.findall(raw))
    cfeat = tomllib.loads(ct).get("features", {})
    ffeat = fman.get("features", {})
    for x in sorted(names):
        impl = [i for i in ffeat.get(x, []) if "/" not in i and not i.startswith("dep:") and i in names]
        dep_items = [i for i in ffeat.get(x, []) if i.startswith("dep:")]
        items = impl + dep_items
        if x in ffeat and uses_types:
            items.append(f"{frm}/{x}")
        if x not in cfeat:
            ct = add_line(ct, "features", f'{x} = [' + ", ".join(f'"{i}"' for i in items) + "]")
    ct_p.write_text(ct)


def forward_features(root: Path, frm: str, to: str, users: set[Path]):
    """Every manifest that depends on <to> and forwards `<frm>/X` also forwards `<to>/X`."""
    tfeat = tomllib.loads((root / "crates" / to / "Cargo.toml").read_text()).get("features", {})
    for mp in sorted(set(users)):
        t = mp.read_text()
        d = tomllib.loads(t)
        if not any(to in d.get(k, {}) for k in ("dependencies", "dev-dependencies")):
            continue
        optional = (d.get("dependencies", {}).get(to) or {})
        opt = isinstance(optional, dict) and optional.get("optional")
        def sub(m):
            q = m.group(0)
            x = m.group(2)
            if x in tfeat and f'"{to}/{x}"' not in t and f'"{to}?/{x}"' not in t:
                return q + f', "{to}/{x}"'
            return q
        t2 = re.sub(r'"(' + re.escape(frm) + r'\??)/([A-Za-z0-9_-]+)"', sub, t)
        if t2 != t:
            mp.write_text(t2)


def manifest_for(root: Path, f: Path) -> tuple[Path, str]:
    rel = f.relative_to(root).parts
    if rel[0] == "crates":
        mp = root / "crates" / rel[1] / "Cargo.toml"
        kind = "dev" if len(rel) > 3 and rel[2] in ("tests", "benches", "examples") else "dep"
        return mp, kind
    return root / "Cargo.toml", "dev" if rel[0] in ("tests", "benches", "examples") else "dep"


# ----------------------------------------------------------------------------- main
def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--from", dest="frm", required=True)
    ap.add_argument("--to", required=True)
    ap.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    ap.add_argument("--plan", action="store_true")
    ap.add_argument("--bottom-up", action="store_true", help="move the lowest crate first: the moved modules may not use modules still in <from>")
    ap.add_argument("--chain", default="", help="comma list of crates, lowest first (default: [types_split].chain)")
    a = ap.parse_args()
    root, frm, to = a.root, a.frm, a.to
    assign, pinned, deps, rn, mods, lib_text = plan(root, frm, to, [c for c in a.chain.split(',') if c], a.bottom_up)
    per: dict[str, int] = {}
    for m, c in assign.items():
        paths = mods.get(m) or list_mods(root / "crates" / c / "src").get(m, [])
        per[c] = per.get(c, 0) + count_lines(paths)
    if a.plan:
        for c, n in sorted(per.items()):
            print(f"{c}: {n} lines, {sum(1 for v in assign.values() if v == c)} modules")
        for m, why in sorted(pinned.items()):
            print(f"PINNED {m}: {why}")
        return 0
    moving = sorted(m for m, c in assign.items() if c == to and m in mods)
    if any(w.startswith("VIOLATION") for w in pinned.values()):
        for m, why in sorted(pinned.items()):
            print(why, file=sys.stderr)
        return 3
    if per.get(to, 0) > 25000:
        print(f"STOP: {to} would hold {per[to]} lines > 25000", file=sys.stderr)
        return 2
    src_to = root / "crates" / to / "src"
    src_to.mkdir(parents=True, exist_ok=True)
    # 1. git mv
    for m in moving:
        for p in mods[m]:
            dest = src_to / p.name
            subprocess.run(["git", "mv", str(p), str(dest)], check=True, cwd=root)
    # 2. lib.rs decls
    chunks = parse_lib(lib_text)
    kept, carried = [], []
    for n, t in chunks:
        if n in moving:
            t2 = [ln for ln in t if ln.strip() != ALLOW]
            idx = next(i for i, ln in enumerate(t2) if MOD_DECL.match(ln))
            t2.insert(idx, ALLOW)
            carried.append("\n".join(t2))
        else:
            kept.append("\n".join(t))
    lib_new = "\n".join(kept)
    # drop root aliases of moved modules
    code = blank_non_code(lib_new)
    cut = []
    for s, e, indent, body in find_use_statements(code):
        leaves = parse_use_tree(body)
        if leaves and not indent and all(l[0] and l[0][0] in moving for l in leaves):
            a0, _ = attrs_above(lib_new, s)
            cut.append((a0, e))
    for s, e in sorted(cut, reverse=True):
        lib_new = lib_new[:s] + lib_new[e:].lstrip("\n")
    (src_to.parent.parent.parent / "crates" / frm / "src" / "lib.rs").write_text(lib_new)
    tl = (src_to / "lib.rs").read_text().rstrip("\n")
    (src_to / "lib.rs").write_text(tl + "\n\n" + "\n".join(carried) + "\n")
    # 3. rewrite
    rw = Rewriter(to, frm, assign, rn)
    files = subprocess.run(["git", "ls-files", "*.rs"], cwd=root, capture_output=True, text=True).stdout.split()
    files += [str(p.relative_to(root)) for p in src_to.rglob("*.rs")]
    changed_users: dict[Path, str] = {}
    for rel in sorted(set(files)):
        f = root / rel
        if not f.exists():
            continue
        text = f.read_text()
        inside = f.is_relative_to(src_to)
        if not inside and "medulla_types" not in text and snake(frm) not in text:
            continue
        new = rw.rewrite(text, inside)
        if new != text:
            f.write_text(new)
            if not inside and snake(to) in new:
                mp, kind = manifest_for(root, f)
                if mp.parent.name != to and not (mp.parent.name == frm and False):
                    if changed_users.get(mp) != "dep":
                        changed_users[mp] = kind
    # 4. manifests
    sync_target_manifest(root, frm, to)
    for mp, kind in changed_users.items():
        t = mp.read_text()
        if has_dep(t, to):
            continue
        rel = Path(__import__("os").path.relpath(root / "crates" / to, mp.parent))
        line = f'{to} = {{ path = "{rel}" }}'
        t = add_line(t, "dependencies" if kind == "dep" else "dev-dependencies", line)
        mp.write_text(t)
    forward_features(root, frm, to, set(changed_users) | {root / "Cargo.toml"})
    print(f"moved {len(moving)} modules ({per.get(to, 0)} lines) to {to}; users: {len(changed_users)} manifests")
    for m, why in sorted(pinned.items()):
        print(f"pinned {m}: {why}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
