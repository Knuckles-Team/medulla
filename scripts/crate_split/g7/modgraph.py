"""Module graph of one crate (G7 split, MEDULLA-CONTRACT-R056.11).

usage (repo root): modgraph.py <crate> [expand_mod ...] > graph.json

Builds the real module tree from lib.rs (file modules, `#[path]` includes, inline `mod x { }`
blocks and the `mod y;` files they declare) and resolves every reference at its true module path:
`crate::`, `$crate::`, `self::`, `super::` chains (at any depth, inside inline modules too), `use`
trees whose first segment is a child module, and bare `child::item` paths. A glob import
(`use super::*`, `use crate::m::*`) is an edge to the globbed module's unit and to every unit that
module itself imports, because a glob also brings the parent's own `use` names into scope.

Units: each top-level module is one unit, except a module listed in `expand` (`dispatch`,
`dispatch::router`), whose children become units (`dispatch::router`, `dispatch::seam`); the
expanded module's own file stays its own unit. lib.rs code outside its `mod x;` files is the
unit `@root`; `crates/<crate>/tests/**` is the unit `@tests`. A reference to a re-export in an expanded module's file lands on the unit that
defines it. Output: {"lines": {unit: n}, "edges": {unit: {unit: [items]}}, "files": {unit: [paths]}}.
"""

import sys, re, json, os
from pathlib import Path
from collections import defaultdict

sys.path.insert(0, "scripts")
from crate_split.graph import blank_non_code, find_use_statements, parse_use_tree  # noqa: E402

crate = sys.argv[1]
expand = set(sys.argv[2:])
src = Path(f"crates/{crate}/src")

MODDECL = re.compile(
    r"(?m)^[ \t]*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*([;{])"
)
PATHATTR = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
SUPER = re.compile(r"(?<![\w:$])((?:super::)+)((?:[A-Za-z_]\w*)(?:::[A-Za-z_]\w*)*)")
SELFP = re.compile(r"(?<![\w:$])self::([A-Za-z_]\w*(?:::[A-Za-z_]\w*)*)")
CRATEP = re.compile(r"(?<![\w:])\$?crate::([A-Za-z_]\w*(?:::[A-Za-z_]\w*)*)")
BARE = re.compile(r"(?<![\w:$.])([a-z_]\w*)::([A-Za-z_]\w*(?:::[A-Za-z_]\w*)*)")

mods = {(): {"file": src / "lib.rs", "children": set()}}  # module path -> info
file_mod = {}  # file -> module path of the file
inline_spans = defaultdict(list)  # file -> [(start, end, inline module path)]
inline_mods = set()  # module paths of inline `mod x { }` blocks


def match_brace(code, i):
    d = 0
    for j in range(i, len(code)):
        if code[j] == "{":
            d += 1
        elif code[j] == "}":
            d -= 1
            if d == 0:
                return j
    return len(code)


def walk(f, mp, child_dir):
    if not f.exists():
        return
    file_mod[f] = mp
    raw = f.read_text()
    code = blank_non_code(raw)
    spans = []  # inline modules in this file: (start, end, path suffix)
    for m in MODDECL.finditer(code):
        pos = m.start(1)
        inner = [s for s in spans if s[0] < pos < s[1]]
        inner.sort(key=lambda s: s[0])
        suffix = inner[-1][2] if inner else ()
        name = m.group(1)
        path = mp + suffix + (name,)
        mods.setdefault(path, {"file": None, "children": set()})
        mods[mp + suffix]["children"].add(name) if mp + suffix in mods else None
        if m.group(2) == "{":
            end = match_brace(code, m.end() - 1)
            spans.append((m.start(), end, suffix + (name,)))
            inline_spans[f].append((m.start(), end, path))
            inline_mods.add(path)
            continue
        pa = PATHATTR.search(raw[m.start() : m.end()])
        base = child_dir.joinpath(*suffix)
        if pa:
            tf = (f.parent if not suffix else base) / pa.group(1)
            cd = tf.parent / tf.stem
        elif (base / f"{name}.rs").exists():
            tf, cd = base / f"{name}.rs", base / name
        else:
            tf, cd = base / name / "mod.rs", base / name
        tf, cd = Path(os.path.normpath(tf)), Path(os.path.normpath(cd))
        mods[path]["file"] = tf
        walk(tf, path, cd)


walk(src / "lib.rs", (), src)


def unit_of_mod(mp):
    mp = tuple(mp)
    while (
        mp and mp in inline_mods
    ):  # an inline module's code lives in its parent's file
        mp = mp[:-1]
    if not mp:
        return "@root"
    u, i = mp[0], 1
    while u in expand and i < len(mp):
        u += "::" + mp[i]
        i += 1
    return u


def modpath_at(f, pos):
    best = file_mod[f]
    for s, e, p in inline_spans[f]:
        if s < pos < e and len(p) > len(best):
            best = p
    return best


# re-export / import tables per module (name -> absolute path), for expanded-unit files
imports = defaultdict(dict)
ext_imports = defaultdict(set)  # module path -> names it imports from other crates
globs = defaultdict(set)
uses_raw = {}
for f, mp in file_mod.items():
    code = blank_non_code(f.read_text())
    uses_raw[f] = (code, find_use_statements(code))


def resolve_head(segs, here):
    """Absolute module-ish path for a segment list as written in module `here`, or None."""
    segs = list(segs)
    if not segs:
        return None
    if segs[0] == "crate":
        return tuple(segs[1:])
    if segs[0] == "super":
        h = list(here)
        while segs and segs[0] == "super":
            if h:
                h.pop()
            segs.pop(0)
        return tuple(h + segs)
    if segs[0] in mods.get(tuple(here), {"children": ()})["children"]:
        return tuple(here) + tuple(segs)
    return None


for f, (code, uses) in uses_raw.items():
    for s, _e, _i, body in uses:
        here = modpath_at(f, s)
        for segs, _l, alias in parse_use_tree(body):
            ab = resolve_head(segs, here)
            if ab is None:
                if segs and segs[-1] != "*":
                    ext_imports[here].add(alias or segs[-1])
                continue
            if ab and ab[-1] == "*":
                globs[here].add(ab[:-1])
            else:
                imports[here][alias or ab[-1]] = ab


DEF = re.compile(
    r'(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|unsafe|const|extern(?:\s+"C")?)\s+)*(?:fn|struct|enum|trait|type|const|static|union|macro_rules!)\s+([A-Za-z_]\w*)'
)
defs = defaultdict(
    set
)  # module path -> item names defined in its own code (inline modules excluded)
for f, mp in file_mod.items():
    code = uses_raw[f][0]
    for m in DEF.finditer(code):
        defs[modpath_at(f, m.start())].add(m.group(1))


def locate(ab, depth=0):
    """(unit, item) for an absolute path, following re-exports of the module it names."""
    ab = tuple(x for x in ab if x != "*")
    k = len(ab)
    while k > 0 and ab[:k] not in mods:
        k -= 1
    mp, rest = ab[:k], ab[k:]
    if (
        rest
        and rest[0] in ext_imports.get(mp, ())
        and rest[0] not in imports.get(mp, {})
        and rest[0] not in defs.get(mp, ())
    ):
        return None, None  # a name the module imports from another crate
    if rest and depth < 6 and rest[0] in imports.get(mp, {}):
        tgt = imports[mp][rest[0]] + rest[1:]
        u, it = locate(tgt, depth + 1)
        if u != unit_of_mod(mp):
            return u, it
    u = unit_of_mod(mp)
    depth_u = 0 if u == "@root" else len(u.split("::"))
    return u, "::".join(
        ab[depth_u:]
    ) or "*"  # item path relative to the unit, as cuts' `covers` name it


def glob_names(mp, seen=None):
    """name -> absolute path for every name a glob of module mp brings into scope."""
    seen = seen or set()
    if mp in seen:
        return {}
    seen.add(mp)
    out = {}
    for g in globs.get(mp, ()):
        out.update(glob_names(g, seen))
    out.update({n: mp + (n,) for n in defs.get(mp, ())})
    out.update(imports.get(mp, {}))
    return out


def glob_units(mp, code):
    """(unit, item) edges a glob of mp creates in `code`: only the names the code uses."""
    out = set()
    words = set(re.findall(r"[A-Za-z_]\w*", code))
    for n, ab in glob_names(mp).items():
        if n in words and locate(ab)[0] is not None:
            out.add(locate(ab))
    return out


units_files = defaultdict(list)
for f, mp in file_mod.items():
    units_files[unit_of_mod(mp)].append(f)
lines = {
    u: sum(len(f.read_text().splitlines()) for f in fs if f.exists())
    for u, fs in units_files.items()
}
orphans = [str(p) for p in src.rglob("*.rs") if p not in file_mod]
tdir = Path(
    f"crates/{crate}/tests"
)  # integration tests: their own unit; they reach the crate only by its public name
if tdir.is_dir():
    units_files["@tests"] = sorted(tdir.rglob("*.rs"))
    lines["@tests"] = sum(
        len(f.read_text().splitlines()) for f in units_files["@tests"]
    )

E = defaultdict(lambda: defaultdict(set))
for f, (code, uses) in uses_raw.items():
    u0 = unit_of_mod(file_mod[f])

    def add(ab, here):
        if ab is None:
            return
        v, it = locate(ab)
        if v is not None and v != unit_of_mod(here):
            E[unit_of_mod(here)][v].add(it)

    for s, _e, _i, body in uses:
        here = modpath_at(f, s)
        for segs, _l, _a in parse_use_tree(body):
            ab = resolve_head(segs, here)
            if ab is not None and ab and ab[-1] == "*":
                sp = [(a, b) for a, b, p in inline_spans[f] if p == tuple(here)]
                for v, it in glob_units(
                    ab[:-1], code[sp[0][0] : sp[0][1]] if sp else code
                ):
                    if v != unit_of_mod(here):
                        E[unit_of_mod(here)][v].add(it)
            else:
                add(ab, here)
    for m in CRATEP.finditer(code):
        add(tuple(m.group(1).split("::")), modpath_at(f, m.start()))
    for m in SUPER.finditer(code):
        here = modpath_at(f, m.start())
        add(
            resolve_head(
                m.group(1).rstrip(":").split("::") + m.group(2).split("::"), here
            ),
            here,
        )
    for m in SELFP.finditer(code):
        here = modpath_at(f, m.start())
        add(tuple(here) + tuple(m.group(1).split("::")), here)
    for m in BARE.finditer(code):
        here = modpath_at(f, m.start())
        name = m.group(1)
        if name in ("crate", "self", "super", "std", "core", "alloc"):
            continue
        if name in mods.get(tuple(here), {"children": ()})["children"]:
            add(tuple(here) + (name,) + tuple(m.group(2).split("::")), here)
        elif name in imports.get(tuple(here), {}):
            add(imports[tuple(here)][name] + tuple(m.group(2).split("::")), here)
        else:
            for g in globs.get(tuple(here), ()):
                if name in mods.get(g, {"children": ()})["children"]:
                    add(g + (name,) + tuple(m.group(2).split("::")), here)
                    break

json.dump(
    {
        "lines": lines,
        "edges": {a: {b: sorted(v) for b, v in d.items()} for a, d in E.items()},
        "files": {u: sorted(str(f) for f in fs) for u, fs in units_files.items()},
        "orphans": orphans,
    },
    sys.stdout,
    indent=0,
)
