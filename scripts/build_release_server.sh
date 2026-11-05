#!/usr/bin/env bash
# Build the release `medulla-server` executable for the wheel being built and
# copy it to dist-server-bin/, where the folded-wheel action injects it into the
# medulla wheel as medulla/_bin/medulla-server (scripts/inject_server_binary.py).
#
# On Linux the folded-wheel action sources this inside the manylinux container
# of the engine wheel build (after the linker setup), so the server links
# against the same glibc baseline as the extension modules. On macOS and Windows
# it runs natively. A binary already present in dist-server-bin/ is reused, so
# the reproduction pass of a dispatch builds the server once.
medulla_build_release_server() {
    local target exe target_dir out=dist-server-bin
    target="${MEDULLA_WHEEL_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
    [ -n "$target" ] || { echo 'cannot determine the server target' >&2; return 1; }
    exe=medulla-server
    case "$target" in *-windows-*) exe=medulla-server.exe ;; esac
    if [ -f "$out/$exe" ]; then
        echo "reusing $out/$exe"
        return 0
    fi
    (
        # The fat-LTO, one-codegen-unit link outgrows the hosted ARM64 Linux and
        # macOS runners; those targets link with thin LTO and parallel codegen.
        case "$target" in
            aarch64-unknown-linux-gnu | *-apple-darwin)
                export CARGO_PROFILE_RELEASE_SERVER_LTO=thin
                export CARGO_PROFILE_RELEASE_SERVER_CODEGEN_UNITS=16
                ;;
        esac
        cargo build --locked --profile release-server --features full,ast-extended \
            --bin medulla-server --target "$target"
    ) || return 1
    target_dir="${CARGO_TARGET_DIR:-}"
    if [ -z "$target_dir" ]; then
        target_dir="$(cargo metadata --no-deps --format-version 1 \
            | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
    fi
    if command -v cygpath >/dev/null 2>&1; then
        target_dir="$(cygpath -u "$target_dir")"
    fi
    mkdir -p "$out" || return 1
    cp "$target_dir/$target/release-server/$exe" "$out/$exe" || return 1
    chmod 0755 "$out/$exe" || return 1
    ls -l "$out/$exe"
}
medulla_build_release_server || { return 1 2>/dev/null || exit 1; }
