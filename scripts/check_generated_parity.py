#!/usr/bin/env python3
"""Fail when the Rust serde wire types and `medulla/generated` Python models drift.

Parses every public serde struct and enum under `crates/*/src` and compares its
wire shape (field names, defaults, enum variants, tagged-union members) with the
Python name of the same spelling in `medulla/generated/*.py`. Intentional
differences live in `ALLOWED` with a reason each. Exit status is 1 on any
unexplained drift or stale allowlist entry.

usage: check_generated_parity.py [repo-root]
"""

import ast
import collections
import glob
import os
import re
import sys
from typing import Any

ROOT = os.path.abspath(
    next((a for a in sys.argv[1:] if not a.startswith("--")), None)
    or os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
)


# ---------- rust ----------
def strip_rust(src):
    out = []
    i = 0
    n = len(src)
    while i < n:
        c = src[i]
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            out.append(" " * 0 + "\n" if False else "")
            i = j
            continue
        if src.startswith("/*", i):
            depth = 1
            j = i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth += 1
                    j += 2
                elif src.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            i = j
            continue
        if (
            c == "r"
            and re.match(r'r#*"', src[i : i + 4])
            and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_"))
        ):
            m = re.match(r'r(#*)"', src[i:])
            assert m is not None
            h = m.group(1)
            end = src.find('"' + h, i + len(m.group(0)))
            if end < 0:
                end = n
            out.append('""')
            i = end + 1 + len(h)
            continue
        if c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            # keep rename strings: we need them in attributes
            out.append(src[i : j + 1])
            i = j + 1
            continue
        if c == "'":
            m = re.match(r"'(\\.|[^\\'])'", src[i : i + 6])
            if m:
                out.append("' '")
                i += len(m.group(0))
                continue
        out.append(c)
        i += 1
    return "".join(out)


def match_close(s, i, o, c):
    # s[i]==o; return index of matching c; strings are skipped
    depth = 0
    n = len(s)
    j = i
    while j < n:
        ch = s[j]
        if ch == '"':
            j += 1
            while j < n and s[j] != '"':
                j += 2 if s[j] == "\\" else 1
        elif ch == o:
            depth += 1
        elif ch == c:
            depth -= 1
            if depth == 0:
                return j
        j += 1
    return -1


def split_top(s):
    parts = []
    depth = 0
    cur = []
    i = 0
    n = len(s)
    while i < n:
        ch = s[i]
        if ch == '"':
            j = i + 1
            while j < n and s[j] != '"':
                j += 2 if s[j] == "\\" else 1
            cur.append(s[i : j + 1])
            i = j + 1
            continue
        if ch in "([{<":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        elif ch == ">" and not (i > 0 and s[i - 1] == "-"):
            depth -= 1
        if ch == "," and depth == 0:
            parts.append("".join(cur))
            cur = []
        else:
            cur.append(ch)
        i += 1
    if "".join(cur).strip():
        parts.append("".join(cur))
    return parts


def take_attrs(s):
    """leading #[...] attrs of s -> (list of attr bodies, rest)"""
    attrs = []
    s = s.lstrip()
    while s.startswith("#["):
        e = match_close(s, 1, "[", "]")
        attrs.append(s[2:e])
        s = s[e + 1 :].lstrip()
    return attrs, s


def words(name):
    # split identifier into words
    s = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", name)
    s = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", s)
    return [w.lower() for w in s.split("_") if w]


def case(name, rule, is_variant):
    if not rule:
        return name
    w = words(name)
    if rule == "lowercase":
        return (
            "".join(w)
            if False
            else name.lower().replace("_", "")
            if is_variant
            else name.lower()
        )
    if rule == "UPPERCASE":
        return name.upper()
    if rule == "PascalCase":
        return "".join(x.capitalize() for x in w)
    if rule == "camelCase":
        return w[0] + "".join(x.capitalize() for x in w[1:])
    if rule == "snake_case":
        return "_".join(w)
    if rule == "SCREAMING_SNAKE_CASE":
        return "_".join(w).upper()
    if rule == "kebab-case":
        return "-".join(w)
    if rule == "SCREAMING-KEBAB-CASE":
        return "-".join(w).upper()
    return name


def serde_info(attrs):
    info: dict[str, Any] = dict(
        rename_all=None,
        rename=None,
        default=False,
        skip=False,
        flatten=False,
        tag=None,
        content=None,
        untagged=False,
        transparent=False,
        deny=False,
        alias=[],
        skip_ser=False,
        skip_de=False,
        skip_if=False,
        other=False,
    )
    for a in attrs:
        if not a.startswith("serde"):
            continue
        b = a[a.index("(") + 1 : a.rindex(")")] if "(" in a else ""
        for part in split_top(b):
            p = part.strip()
            m = re.match(r'(\w+)\s*=\s*"([^"]*)"', p)
            if m:
                k, v = m.groups()
                if k == "rename_all":
                    info["rename_all"] = v
                elif k == "rename":
                    info["rename"] = v
                elif k == "tag":
                    info["tag"] = v
                elif k == "content":
                    info["content"] = v
                elif k == "default":
                    info["default"] = True
                    info["default_fn"] = v
                elif k == "alias":
                    info["alias"].append(v)
                elif k == "skip_serializing_if":
                    info["skip_if"] = True
                continue
            m = re.match(r"rename\s*\(", p)
            if m:
                s = re.search(r'deserialize\s*=\s*"([^"]*)"', p) or re.search(
                    r'serialize\s*=\s*"([^"]*)"', p
                )
                if s:
                    info["rename"] = s.group(1)
                continue
            m = re.match(r"rename_all\s*\(", p)
            if m:
                s = re.search(r'deserialize\s*=\s*"([^"]*)"', p)
                if s:
                    info["rename_all"] = s.group(1)
                continue
            if p == "default":
                info["default"] = True
            elif p == "skip":
                info["skip"] = True
            elif p == "skip_serializing":
                info["skip_ser"] = True
            elif p == "skip_deserializing":
                info["skip_de"] = True
            elif p == "flatten":
                info["flatten"] = True
            elif p == "untagged":
                info["untagged"] = True
            elif p == "transparent":
                info["transparent"] = True
            elif p == "deny_unknown_fields":
                info["deny"] = True
            elif p == "other":
                info["other"] = True
    return info


IDENT = re.compile(r"[A-Za-z_]\w*")


def idents(t):
    return set(IDENT.findall(t))


def parse_fields(body, rule):
    fields = []
    for part in split_top(body):
        attrs, rest = take_attrs(part)
        rest = re.sub(r"^pub(\([^)]*\))?\s+", "", rest.strip())
        m = re.match(r"(?:r#)?(\w+)\s*:\s*(.*)$", rest, re.S)
        if not m:
            continue
        nm, ty = m.group(1), m.group(2).strip()
        si = serde_info(attrs)
        wire = si["rename"] or case(nm, rule, False)
        fields.append(
            dict(
                name=nm,
                wire=wire,
                type=ty,
                default_fn=si.get("default_fn"),
                **{
                    k: si[k]
                    for k in (
                        "default",
                        "skip",
                        "flatten",
                        "skip_ser",
                        "skip_de",
                        "skip_if",
                        "alias",
                    )
                },
            )
        )
    return fields


RS: dict[str, list[dict[str, Any]]] = {}  # name -> list of defs


def add(d):
    RS.setdefault(d["name"], []).append(d)


ITEM = re.compile(r"\bpub(?:\([^)]*\))?\s+(struct|enum|type)\s+(\w+)")


def parse_rust_file(path):
    try:
        src = open(path, encoding="utf-8").read()
    except Exception:
        return
    s = strip_rust(src)
    # drop cfg(test) mod blocks
    for m in list(re.finditer(r"#\[cfg\(test\)\]\s*(?:pub\s+)?mod\s+\w+\s*\{", s))[
        ::-1
    ]:
        e = match_close(s, m.end() - 1, "{", "}")
        if e > 0:
            s = s[: m.start()] + s[e + 1 :]
    for m in ITEM.finditer(s):
        kind, name = m.groups()
        # attrs before
        pre = s[: m.start()].rstrip()
        attrs = []
        while pre.endswith("]"):
            # find matching [ backwards
            depth = 0
            j = len(pre) - 1
            while j >= 0:
                if pre[j] == "]":
                    depth += 1
                elif pre[j] == "[":
                    depth -= 1
                    if depth == 0:
                        break
                j -= 1
            if j < 1 or pre[j - 1] != "#":
                break
            attrs.append(pre[j + 1 : -1])
            pre = pre[: j - 1].rstrip()
        derives = "".join(a for a in attrs if a.startswith("derive"))
        pos = m.end()
        if kind == "type":
            semi = s.find(";", pos)
            eq = s.find("=", pos)
            add(
                dict(
                    name=name,
                    kind="alias",
                    target=s[eq + 1 : semi].strip(),
                    path=path,
                    ser=True,
                    refs=idents(s[eq + 1 : semi]),
                    generic=False,
                )
            )
            continue
        # generics
        generic = False
        k = pos
        while s[k].isspace():
            k += 1
        if s[k] == "<":
            e = match_close(s, k, "<", ">")
            # naive for nested <> ok
            generic = True
            k = e + 1
        # find body start
        mm = re.compile(r"[{(;]").search(s, k)
        if not mm:
            continue
        # where clause may include { ? rare
        ch = mm.group(0)
        ser = "Serialize" in derives or "Deserialize" in derives
        si = serde_info(attrs)
        base = dict(name=name, path=path, ser=ser, generic=generic, si=si)
        if ch == ";":
            add(dict(base, kind="unit", fields=[], refs=set()))
            continue
        if ch == "(":
            e = match_close(s, mm.start(), "(", ")")
            inner = s[mm.start() + 1 : e]
            tys = []
            for part in split_top(inner):
                _, r = take_attrs(part)
                r = re.sub(r"^pub(\([^)]*\))?\s+", "", r.strip())
                tys.append(r)
            add(
                dict(
                    base,
                    kind="newtype",
                    fields=[],
                    types=tys,
                    refs=set().union(*[idents(t) for t in tys]) if tys else set(),
                )
            )
            continue
        e = match_close(s, mm.start(), "{", "}")
        body = s[mm.start() + 1 : e]
        if kind == "struct":
            fl = parse_fields(body, si["rename_all"])
            refs = set()
            for f in fl:
                refs |= idents(f["type"])
            add(dict(base, kind="struct", fields=fl, refs=refs))
        else:
            variants = []
            refs = set()
            for part in split_top(body):
                vattrs, rest = take_attrs(part)
                rest = rest.strip()
                vm = re.match(r"(\w+)\s*(.*)$", rest, re.S)
                if not vm:
                    continue
                vn, tail = vm.group(1), vm.group(2).strip()
                vsi = serde_info(vattrs)
                wire = vsi["rename"] or case(vn, si["rename_all"], True)
                v = dict(
                    is_default=("default" in [a.strip() for a in vattrs]),
                    name=vn,
                    wire=wire,
                    skip=vsi["skip"],
                    other=vsi["other"],
                    alias=vsi["alias"],
                    shape="unit",
                    fields=[],
                    types=[],
                )
                if tail.startswith("("):
                    ee = match_close(tail, 0, "(", ")")
                    tys = [
                        re.sub(r"^pub\s+", "", take_attrs(p)[1].strip())
                        for p in split_top(tail[1:ee])
                    ]
                    v["shape"] = "tuple"
                    v["types"] = tys
                    for t in tys:
                        refs |= idents(t)
                elif tail.startswith("{"):
                    ee = match_close(tail, 0, "{", "}")
                    v["shape"] = "struct"
                    v["fields"] = parse_fields(tail[1:ee], vsi["rename_all"])
                    for f in v["fields"]:
                        refs |= idents(f["type"])
                variants.append(v)
            add(dict(base, kind="enum", variants=variants, refs=refs))


for p in glob.glob(
    os.path.join(ROOT, "crates", "*", "src", "**", "*.rs"), recursive=True
):
    if (
        "/tests/" in p
        or p.endswith("tests.rs")
        or "_tests" in p.split("/crates/")[1].split("/")[0]
    ):
        continue
    parse_rust_file(p)

# ---------- python ----------
PY_CLASSES: dict[
    str, dict[str, Any]
] = {}  # name -> dict(kind, fields{wire:...}, values)
PY_ALIASES: dict[str, Any] = {}  # name -> ast expr
PY_NAMES: set[str] = set()


def lit_strings(node):
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return [node.value]
    if isinstance(node, ast.Tuple):
        return [x.value for x in node.elts if isinstance(x, ast.Constant)]
    return []


def flatten_union(node):
    """-> list of members: ('lit', str) | ('name', str)"""
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.BitOr):
        return flatten_union(node.left) + flatten_union(node.right)
    if isinstance(node, ast.Subscript):
        base = (
            node.value.id
            if isinstance(node.value, ast.Name)
            else getattr(node.value, "attr", "")
        )
        sl = node.slice
        if base == "Annotated":
            return flatten_union(sl.elts[0] if isinstance(sl, ast.Tuple) else sl)
        if base in ("Union",):
            return sum(
                [
                    flatten_union(x)
                    for x in (sl.elts if isinstance(sl, ast.Tuple) else [sl])
                ],
                [],
            )
        if base == "Literal":
            return [("lit", v) for v in lit_strings(sl)]
        if base == "Optional":
            return flatten_union(sl)
        return [("other", base)]
    if isinstance(node, ast.Name):
        return [("name", node.id)]
    if isinstance(node, ast.Constant) and node.value is None:
        return [("none", "")]
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return [("name", node.value)]
    return [("other", "")]


for p in glob.glob(os.path.join(ROOT, "medulla", "generated", "*.py")):
    if os.path.basename(p) in ("_runtime.py", "_ids.py", "_manifest.py", "__init__.py"):
        continue
    tree = ast.parse(open(p).read())
    for n in tree.body:
        if isinstance(n, ast.ClassDef):
            bases = [b.id for b in n.bases if isinstance(b, ast.Name)]
            if (
                "Enum" in bases
                or "StrEnum" in bases
                or any(b in ("str",) for b in bases)
                and "Enum" in bases
            ):
                vals = [
                    x.value.value
                    for x in n.body
                    if isinstance(x, ast.Assign) and isinstance(x.value, ast.Constant)
                ]
                PY_CLASSES[n.name] = dict(kind="enum", values=set(vals), path=p)
            else:
                flds: dict[str, Any] = {}
                for x in n.body:
                    if isinstance(x, ast.AnnAssign) and isinstance(x.target, ast.Name):
                        wire: Any = x.target.id
                        lit = None
                        hasdef = x.value is not None
                        if isinstance(x.value, ast.Call):
                            for kw in x.value.keywords:
                                if kw.arg == "alias" and isinstance(
                                    kw.value, ast.Constant
                                ):
                                    wire = kw.value.value
                            fa = x.value.args[0] if x.value.args else None
                            if isinstance(fa, ast.Constant) and fa.value is ...:
                                hasdef = False
                            if any(
                                kw.arg in ("default", "default_factory")
                                for kw in x.value.keywords
                            ):
                                hasdef = True
                        for sub in ast.walk(x.annotation):
                            if isinstance(sub, ast.Call):
                                for kw in sub.keywords:
                                    if kw.arg == "alias" and isinstance(
                                        kw.value, ast.Constant
                                    ):
                                        wire = kw.value.value
                        ann = ast.unparse(x.annotation)
                        m = re.fullmatch(r"Literal\[['\"](.*)['\"]\]", ann)
                        if m:
                            lit = m.group(1)
                        flds[wire] = dict(
                            lit=lit, hasdef=hasdef, ann=ann, pyname=x.target.id
                        )
                if not (n.name in PY_CLASSES and list(flds) == ["request"]):
                    PY_CLASSES[n.name] = dict(
                        kind="model", fields=flds, bases=bases, path=p
                    )
            PY_NAMES.add(n.name)
        elif (
            isinstance(n, ast.Assign)
            and len(n.targets) == 1
            and isinstance(n.targets[0], ast.Name)
        ):
            PY_ALIASES[n.targets[0].id] = n.value
            PY_NAMES.add(n.targets[0].id)
        elif (
            isinstance(n, ast.AnnAssign)
            and isinstance(n.target, ast.Name)
            and n.value is not None
        ):
            PY_ALIASES[n.target.id] = n.value
            PY_NAMES.add(n.target.id)


def py_union_variants(name, seen=None):
    """-> (wire-variant-set, {wire: class or None}) for alias union"""
    seen = seen or set()
    if name in seen:
        return {}, False
    seen.add(name)
    node = PY_ALIASES.get(name)
    if node is None:
        return None
    out: dict[Any, Any] = {}
    for kind, v in flatten_union(node):
        if kind == "lit":
            out[v] = None
        elif kind == "name":
            if v in PY_CLASSES and PY_CLASSES[v]["kind"] == "enum":
                for x in PY_CLASSES[v]["values"]:
                    out[x] = None
            elif v in PY_CLASSES:
                c = PY_CLASSES[v]
                out[("cls", v)] = c
            elif v in PY_ALIASES:
                sub = py_union_variants(v, seen)
                if sub:
                    out.update(sub)
    return out


def rust_fields_flat(d, depth=0):
    fl = []
    for f in d.get("fields", []):
        if f["flatten"]:
            base = None
            for t in idents(f["type"]):
                if t in RS and depth < 4:
                    base = RS[t][0]
                    break
            if base and base["kind"] == "struct":
                fl += rust_fields_flat(base, depth + 1)
            else:
                fl.append(dict(f, unresolved=True))
        else:
            fl.append(f)
    return fl


def cmp_struct(d, pyc):
    problems = []
    fl = rust_fields_flat(d)
    if any(f.get("unresolved") for f in fl):
        return ["flatten-unresolved"]
    rf = {f["wire"]: f for f in fl if not f["skip"]}
    # fields that are never serialized/deserialized
    pf = pyc["fields"]
    for w, f in rf.items():
        if f["skip_ser"] and f["skip_de"]:
            continue
        if w not in pf:
            problems.append(f"missing field {w}")
    for w in pf:
        if w not in rf:
            problems.append(f"extra field {w}")
    for w, f in rf.items():
        if w in pf:
            if f["default"] and not pf[w]["hasdef"]:
                problems.append(f"default missing on {w}")
    return problems


def rust_wire_name_set(d):
    return {v["wire"] for v in d["variants"] if not v["skip"]}


def cmp_enum(d, name):
    si = d["si"]
    vs = [v for v in d["variants"] if not v["skip"]]
    if si["untagged"]:
        return None
    allunit = all(v["shape"] == "unit" for v in vs)
    want = {v["wire"] for v in vs}
    if name in PY_CLASSES and PY_CLASSES[name]["kind"] == "enum":
        have = PY_CLASSES[name]["values"]
        if not allunit:
            return ["python enum but rust enum has payloads"]
        return [f"missing variant {w}" for w in sorted(want - have)] + [
            f"extra variant {w}" for w in sorted(have - want)
        ]
    u = py_union_variants(name)
    if u is None:
        return (
            ["no python definition"]
            if name not in PY_CLASSES
            else ["python is a model but rust is enum"]
        )
    have = set()
    perv = []
    for k, c in u.items():
        if isinstance(k, tuple):
            cname = k[1]
            if si["tag"]:
                lits = [
                    f["lit"]
                    for w, f in c["fields"].items()
                    if w == si["tag"] and f["lit"] is not None
                ]
                if lits:
                    have.add(lits[0])
                    rv = next((v for v in vs if v["wire"] == lits[0]), None)
                    if rv and rv["shape"] == "struct" and not si["content"]:
                        rf = {f["wire"] for f in rv["fields"] if not f["skip"]}
                        pf = set(c["fields"]) - {si["tag"]}
                        for w in sorted(rf - pf):
                            perv.append(f"{lits[0]}: missing field {w}")
                        for w in sorted(pf - rf):
                            perv.append(f"{lits[0]}: extra field {w}")
                else:
                    have.add("?" + cname)
            else:
                ks = list(c["fields"])
                if len(ks) == 1:
                    have.add(ks[0])
                else:
                    have.add("?" + cname)
        else:
            have.add(k)
    miss = want - have
    extra = {h for h in have if not h.startswith("?")} - want
    return (
        [f"missing variant {w}" for w in sorted(miss)]
        + [f"extra variant {w}" for w in sorted(extra)]
        + perv
        + [f"unattributed member {h[1:]}" for h in have if h.startswith("?")]
    )


def main():
    report: dict[str, Any] = collections.OrderedDict()
    mirrored = set()
    for name, defs in RS.items():
        if name not in PY_NAMES:
            continue
        cand = [
            d
            for d in defs
            if d["kind"] in ("struct", "enum", "newtype", "unit")
            and d["ser"]
            and not d["generic"]
        ]
        if not cand:
            continue
        mirrored.add(name)
        best = None
        for d in cand:
            if d["kind"] == "struct":
                if name in PY_CLASSES and PY_CLASSES[name]["kind"] == "model":
                    pr = cmp_struct(d, PY_CLASSES[name])
                else:
                    pr = ["python is not a model"]
            elif d["kind"] == "enum":
                pr = cmp_enum(d, name)
                if pr is None:
                    continue
            else:
                continue
            if not pr:
                best = None
                break
            if best is None or len(pr) < len(best[1]):
                best = (d, pr)
        else:
            if best:
                report[name] = best
            continue
    return report, mirrored


# ---------- the allowlist ----------
# Every intentional Rust/Python difference, with its reason. An entry is a type
# name plus regular expressions that must each fullmatch a drift line; a line
# that no entry covers fails the gate, and an entry that covers no line fails it
# too, so the list cannot go stale. Add an entry only for a difference that is
# deliberate and explained.
ALLOWED = {
    "PackViolation": (
        "`raw_code` is Python-only: when the engine sends a code this client does "
        "not know, the model keeps the code as sent in `raw_code` and sets `code` "
        "to `PackViolationCode.UNKNOWN`",
        [r"extra field raw_code"],
    ),
    "PackViolationCode": (
        "`UNKNOWN` is the Python enum's forward-compatible member for a code the "
        "engine added after this client was generated; the Rust enum is closed",
        [r"extra variant UNKNOWN"],
    ),
    "SolverConfig": (
        "the Rust `SolverConfig` is a validating wrapper whose single private "
        "`spec: SolverConfigSpec` field is (de)serialized as `SolverConfigSpec` "
        "(`try_from`/`into`); the Python model carries the spec's four fields "
        "directly",
        [
            r"missing field spec",
            r"extra field (accepted_gap|bound_denominator"
            r"|max_certificate_leaves|node_budget)",
        ],
    ),
    "Method": (
        "name collision: the Rust definition this check sees is the quantum "
        "transport `Method`; Python `Method` is the protocol method union, which "
        "is generated by a macro this check cannot parse and is covered by "
        "`test_generated_contract_inventory`",
        [r"extra variant \w+"],
    ),
    "Violation": (
        "name collision: the Rust definition this check picks is the ontology "
        "shape-report `Violation` struct; Python `Violation` is the policy "
        "violation union of the same name, which has its own Rust enum",
        [r"python is not a model"],
    ),
    "SyncOutcome": (
        "Rust's `SyncReceipt` flattens the outcome plus `warnings`; Python mirrors "
        "the receipt as `SyncReceipt = SyncOutcome`, so `warnings` is carried on "
        "each non-refused variant (a refused answer is an error response, never a "
        "receipt)",
        [
            r"(accepted|conflict_retained|resolved|observed|acknowledged)"
            r": extra field warnings"
        ],
    ),
    "SyncReceipt": (
        "Rust `SyncReceipt` is a struct flattening `SyncOutcome` plus `warnings`; "
        "Python mirrors it as the alias `SyncReceipt = SyncOutcome` (see "
        "`SyncOutcome`)",
        [r"python is not a model"],
    ),
}

# String-id newtypes (`struct XId(String)`) are mirrored as plain `str`, not as
# a Python class, so newtypes are not compared field by field.
NEWTYPE_NOTE = "string-id newtypes are mirrored as `str` and are not compared"


def apply_allowlist(report):
    """-> (unexplained lines, stale allowlist entries)."""
    unexplained = []
    used = set()
    for name, (d, problems) in sorted(report.items()):
        patterns = ALLOWED.get(name, (None, []))[1]
        for problem in problems:
            hit = next((p for p in patterns if re.fullmatch(p, problem)), None)
            if hit is None:
                unexplained.append(
                    f"{name}: {problem}  [{os.path.relpath(d['path'], ROOT)}]"
                )
            else:
                used.add((name, hit))
    stale = [
        f"{name}: allowlist pattern {p!r} matches no drift"
        for name, (_reason, patterns) in ALLOWED.items()
        for p in patterns
        if (name, p) not in used
    ]
    return unexplained, stale


def run():
    report, mirrored = main()
    unexplained, stale = apply_allowlist(report)
    newtypes = sum(1 for n in mirrored if any(d["kind"] == "newtype" for d in RS[n]))
    for line in unexplained:
        print(f"DRIFT {line}")
    for line in stale:
        print(f"STALE {line}")
    print(
        f"generated parity: {len(mirrored)} mirrored types, {len(report)} with "
        f"allowlisted differences, {len(unexplained)} unexplained, {len(stale)} stale "
        f"allowlist entries; {newtypes} newtypes skipped ({NEWTYPE_NOTE})"
    )
    return 1 if unexplained or stale else 0


if __name__ == "__main__":
    sys.exit(run())
