#!/usr/bin/env python3
"""Fetch the last production release's engine and client for the upgrade proof.

Spec: MEDULLA-DURABLE-KERNEL-R070.4. The release pinned in
`tests/fixtures/production_release.json` is the one deployments run today. This
script takes its server executable from the published linux/amd64 container
image and its client from the published wheel. Both are checked against the
pinned SHA-256 digests before use. It needs only the standard library and pip.

Usage: fetch_production_engine.py <output-dir>
Writes `<output-dir>/server` and a virtual environment `<output-dir>/venv`.
"""

from __future__ import annotations

import hashlib
import io
import json
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PIN = ROOT / "tests" / "fixtures" / "production_release.json"
REGISTRY = "https://registry-1.docker.io/v2"
TIMEOUT_S = 300
MANIFEST_TYPES = ",".join(
    (
        "application/vnd.oci.image.index.v1+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
        "application/vnd.oci.image.manifest.v1+json",
        "application/vnd.docker.distribution.manifest.v2+json",
    )
)


def _get(url: str, token: str, accept: str) -> bytes:
    request = urllib.request.Request(
        url, headers={"Authorization": f"Bearer {token}", "Accept": accept}
    )
    with urllib.request.urlopen(request, timeout=TIMEOUT_S) as response:
        return response.read()


def _amd64_manifest(repository: str, tag: str, token: str) -> dict:
    url = f"{REGISTRY}/{repository}/manifests/{tag}"
    manifest = json.loads(_get(url, token, MANIFEST_TYPES))
    if "manifests" not in manifest:
        return manifest
    digest = next(
        entry["digest"]
        for entry in manifest["manifests"]
        if entry.get("platform", {}).get("architecture") == "amd64"
    )
    return json.loads(
        _get(f"{REGISTRY}/{repository}/manifests/{digest}", token, MANIFEST_TYPES)
    )


def fetch_server(pin: dict, destination: Path) -> None:
    """Extract the pinned server executable from the image layers."""

    repository, tag = pin["server_image"].rsplit(":", 1)
    scope = f"repository:{repository}:pull"
    auth = f"https://auth.docker.io/token?service=registry.docker.io&scope={scope}"
    with urllib.request.urlopen(auth, timeout=TIMEOUT_S) as response:
        token = json.load(response)["token"]
    found = None
    for layer in _amd64_manifest(repository, tag, token)["layers"]:
        blob = _get(f"{REGISTRY}/{repository}/blobs/{layer['digest']}", token, "*/*")
        with tarfile.open(fileobj=io.BytesIO(blob)) as archive:
            for member in archive.getmembers():
                if member.isfile() and member.name.lstrip("./") == pin["server_path"]:
                    handle = archive.extractfile(member)
                    found = handle.read() if handle else None
    if found is None:
        sys.exit(f"{pin['server_image']} has no {pin['server_path']}")
    digest = hashlib.sha256(found).hexdigest()
    if digest != pin["server_sha256"]:
        sys.exit(f"production server digest {digest} differs from the pin")
    destination.write_bytes(found)
    destination.chmod(0o755)


def install_client(pin: dict, venv: Path) -> None:
    """Install the pinned client wheel into its own virtual environment."""

    subprocess.run([sys.executable, "-m", "venv", str(venv)], check=True)
    python = str(venv / "bin" / "python")
    wheels = venv.parent / "wheels"
    pip = [python, "-m", "pip", "--disable-pip-version-check", "-q"]
    subprocess.run(
        [
            *pip,
            "download",
            "--no-deps",
            "--only-binary",
            ":all:",
            "-d",
            str(wheels),
            pin["client_requirement"],
        ],
        check=True,
    )
    (wheel,) = wheels.glob("*.whl")
    digest = hashlib.sha256(wheel.read_bytes()).hexdigest()
    if digest != pin["client_wheel_sha256"]:
        sys.exit(f"production client wheel digest {digest} differs from the pin")
    subprocess.run([*pip, "install", str(wheel)], check=True)


def main() -> None:
    output = Path(sys.argv[1]).resolve()
    output.mkdir(parents=True, exist_ok=True)
    pin = json.loads(PIN.read_text(encoding="utf-8"))
    fetch_server(pin, output / "server")
    install_client(pin, output / "venv")
    print(f"production release {pin['version']} ready under {output}")


if __name__ == "__main__":
    main()
