# Engine observability: symbolizing a stack

A released server ships stripped. Each release uploads one symbol artifact per platform,
named by the build identity of the shipped executable:
`symbols-<platform>-<build-id>.tar.zst` with a `.sha256` file beside it. `<platform>` is the
release matrix name (`linux-aarch64`, `windows-x86_64`, `macos-aarch64`).

## Symbolize a stack

Start from the offsets of the stack and the build-id the running server prints with
`--build-id`:

```sh
scripts/symbolize.sh --build-id <build-id> <tag> <platform> <offset>...
```

The command downloads the symbol artifact of the tag with `gh release download`, verifies its
sha256, checks that the symbol file inside carries the build-id of the running server, and
resolves each offset with `addr2line`. It prints one line per offset:
`<offset> <function> <file:line>`.

`--artifact-dir <dir>` uses a symbol artifact and its `.sha256` file that are already in `<dir>`
instead of downloading them.

### Worked example

```sh
$ medulla-server --build-id
3f1c9a0e5b7d2c48a6e1f0b9d4c7a2e85b6d1f03
$ scripts/symbolize.sh --build-id 3f1c9a0e5b7d2c48a6e1f0b9d4c7a2e85b6d1f03 \
      v3.3.0 linux-aarch64 0x2a4f10 0x2a5b8c
0x2a4f10 medulla_storage::kernel::StorageKernel::commit crates/medulla-storage/src/kernel.rs:812
0x2a5b8c medulla_transaction::kernel::MutationKernel::admit crates/medulla-transaction/src/kernel.rs:204
```

### Refusals

| Code | Meaning |
|---|---|
| `SYMBOL_ARTIFACT_MISSING` | The tag has no symbol artifact for this platform and build-id, or its `.sha256` file is absent. |
| `SYMBOL_ARTIFACT_DIGEST_MISMATCH` | The archive does not match its `.sha256` file. |
| `SYMBOL_FILE_MISSING` | The archive holds no `.debug` symbol file; only the Linux symbol file is read by `addr2line`. |
| `BUILD_ID_MISMATCH` | The symbol file carries a different build-id than the server printed. |

The command writes only a temporary directory, removed on exit. It resolves a stack in well
under 30 seconds on a 100 Mbit link.
