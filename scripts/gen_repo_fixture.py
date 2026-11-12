#!/usr/bin/env python3
"""Generate the symbol fixture of the repository enrichment hot-path bench.

MEDULLA-REPO-INGEST-R600.6: the nightly bench ``r600_enrich_served`` indexes a
repository of 200,000 symbols. This script writes that fixture as JSON lines,
one symbol per line: ``{"id", "file", "name", "calls": [ids]}``. Every symbol
calls at most ``--fanout`` neighbours inside its own module and one symbol of the
next module, so the call graph has communities and its edge count is linear in
the symbol count. The output is deterministic for one set of arguments.

Usage: gen_repo_fixture.py OUT [--symbols N] [--module-size N] [--fanout N]
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def symbols(count: int, module_size: int, fanout: int):
    """Yield the fixture rows in id order."""
    for n in range(count):
        module, offset = divmod(n, module_size)
        calls = [
            module * module_size + (offset + step) % module_size
            for step in range(1, fanout + 1)
        ]
        cross = ((module + 1) * module_size + offset) % count
        calls.append(cross)
        yield {
            "id": f"symbol:{n}",
            "file": f"src/module_{module}.py",
            "name": f"fn_{n}",
            "calls": [
                f"symbol:{target}" for target in dict.fromkeys(calls) if target != n
            ],
        }


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=Path)
    parser.add_argument("--symbols", type=int, default=200_000)
    parser.add_argument("--module-size", type=int, default=50)
    parser.add_argument("--fanout", type=int, default=2)
    args = parser.parse_args(argv)
    if min(args.symbols, args.module_size, args.fanout) < 1:
        print("symbols, module-size and fanout must be positive", file=sys.stderr)
        return 2
    with args.out.open("w", encoding="utf-8") as handle:
        for row in symbols(args.symbols, args.module_size, args.fanout):
            handle.write(json.dumps(row, separators=(",", ":")) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
