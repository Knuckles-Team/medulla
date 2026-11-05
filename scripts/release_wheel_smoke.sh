#!/usr/bin/env bash
set -euo pipefail
python -m pip install --upgrade pip
# Install the BARE wheel — NO compatibility extra. The native kernel retired its
# interpreter-side NumPy dependency; `check_wheel_completeness.py` rejects both
# base and extra-gated NumPy metadata. Keep this smoke test independent from the
# developer-only parity environment below.
pip install --force-reinstall "$(ls dist/medulla-*.whl)"
# (1) The wheel installs no console script: the server is package data at
#     medulla/_bin/medulla-server, reached through medulla.server_binary.
if command -v medulla-server >/dev/null 2>&1; then
  echo "unexpected medulla-server console script from the wheel" >&2
  exit 1
fi
# (2) Import + compute the kernel FROM THE INSTALLED WHEEL. Run from a neutral
#     dir ($RUNNER_TEMP) — the repo checkout root holds the `medulla/`
#     SOURCE package (no compiled `.so`), and cwd is on sys.path for `python -c`,
#     so importing at the repo root would shadow the wheel and always fail.
cd "${RUNNER_TEMP:-/tmp}"
python - <<'PY'
import importlib.metadata as metadata
import re

requirements = metadata.requires("medulla") or []
assert not any(
    re.split(r"[\\[<>=!~;]", requirement, maxsplit=1)[0].lower() == "numpy"
    for requirement in requirements
), requirements
PY
python -c "import medulla.numeric as n; values=[1.,2.,3.,4.]; assert n.sum(values)==10.0 and n.mean(values)==2.5, (n.sum(values), n.mean(values)); print('numeric kernel OK:', n.__file__, '| sum=', n.sum(values), 'mean=', n.mean(values), 'std=', n.std(values))"
python -c "import medulla.engine as e; print('engine kernel OK:', e.__file__, '| __engine__=', getattr(e, '__engine__', None))"
# (3) The packaged server runs and reports the wheel's own version.
python - <<'PY'
import importlib.metadata as metadata
import subprocess

from medulla.server_binary import resolve_server_binary

binary = resolve_server_binary()
version = metadata.version("medulla")
out = subprocess.run(
    [binary, "--version"], capture_output=True, text=True, check=True
).stdout.strip()
assert out == f"medulla-server {version}", (out, version)
print("packaged server OK:", binary, "|", out)
PY
