#!/usr/bin/env bash
# MEDULLA-FEDERATED-QUERY-R053.100: build gate for generated Cypher property keys.
# Runs the renderer over its fixtures (plain, reserved, spaced, Unicode, embedded
# backtick, one unescaped key) and checks each against the openCypher escaped
# symbolic-name production. Any unquoted or malformed key fails the build.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo test --locked -p medulla-federation --features federation --lib \
  federation_cypher_unbound::tests::key_quoting_fixtures -- --exact
