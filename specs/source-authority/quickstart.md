# MEDULLA-SRC-AUTH — Quickstart

The shortest served proof attaches two authority rows, asks a class-only question and reads which source answered and why.

## Scenario

1. Attach the virtual-graph vocabulary and two operator-origin schema sources: ServiceNow at rank 1 and PagerDuty at rank 2 for `:Incident`, both with approved mappings.
2. Send `Method::SourceCoverage` with the class `:Incident` and read the verdict and `audit`.
3. Swap the two ranks with a second attach, send the same request bytes and read `audit` again; the winner changes and the request does not.

## Commands

Run the library proof of the selection order and the audit:

```text
cargo test -p medulla-virtual-graph source_selection
cargo test -p medulla-rdf --test source_authority_fixture
```

Run the shape fixtures and the vocabulary test:

```text
cargo test -p medulla-shacl --test source_authority_shapes
cargo test -p medulla-core --test source_authority_vocab
```

Run the served tests of the family:

```text
uv run pytest tests/federated -k "source_coverage or source_role or trust_mark or source_authority"
```
