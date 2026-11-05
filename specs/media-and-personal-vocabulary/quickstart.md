# MEDULLA-MEDIA-VOCAB — Quickstart

The shortest proof of the vocabulary loads the ontology files with the native parser, checks the declared terms and runs the SHACL fixtures. The served surfaces are proved through the production dispatch of the served-tests crate.

## Scenario

1. Load `media-v1.ttl`, `content-v1.ttl` and their shapes with the native parser.
2. Apply one connector mapping to its recorded response and answer the single-source question from the resulting nodes.
3. For a served surface, start the served harness of `crates/medulla-server-served-tests`, send the request under a tenant and principal context, and read the result back through a separately authorized query.

## Commands

Run the vocabulary and mapping proofs:

```text
cargo test -p medulla-core --test media_vocab_a --test media_vocab_b
cargo test -p medulla-rdf --test media_vocab_shapes --test media_vocab_cross_source
```

Run one served proof once its test file exists:

```text
cargo test -p medulla-server-served-tests authorized_search_returns_only_visible_tags
```
