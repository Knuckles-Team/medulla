# MEDULLA-CONTRACT — Research

## Release as a fixed point

- Unknown: whether a release can be checked without trusting the build host.
- Finding: a build that takes only pinned inputs is a function of the source. Building the same tag twice and comparing digests tests that claim directly. Equal digests mean the build holds no hidden input such as time, path or host.
- Finding: wheels already pass this test. The sdist and the server image did not, so a timestamp or path leak in them would go unseen.
- Decision: MEDULLA-CONTRACT-R053.14 extends the check to all three artifact kinds and gates publication on it.

## Content-addressed identity

- Question: is every durable artifact identified by a digest of its content?
- Finding: yes for artifacts. Blobs (MEDULLA-SCM-R001), canonical workflows (MEDULLA-CQ-ORCH-R701.2), seed imports (MEDULLA-TYPED-PACKS-R104), skill packages (MEDULLA-DECISION-ENGINE-R160.1) and workflow registry entries (MEDULLA-DECISION-ENGINE-R166.1) carry a content digest.
- Finding: graph facts are identified by assigned ids with bitemporal versions, and an edge by its source, target and relation triple (MEDULLA-REPO-INGEST-R009.1). A content-hash id would break supersession, because a corrected fact would become a different entity.
- Decision: no new row. Artifact identity is covered. Fact identity stays assigned by design.

## Decision: Method-body vectors are generated, not committed

**Decision.** The vectors the Go and JavaScript clients replay are produced at test time from the Rust codec (`MEDULLA-CONTRACT-R061`). A committed vector file would be a second copy of the codec output that can drift. The Go and JavaScript vector tests consume the generated file and run in the lane that has those toolchains.

## Codec size budget

- Question: why did the method-codec module outgrow its size budget, and what should the budget be?
- Finding: the old budget, 10,946,464 bytes, was the size of the committed module before the pinned `wasm-opt -Oz` step was added. After that step the module was 9,706,258 bytes at release 3.3.3.
- Finding: at release 3.3.4 the module was 12,484,079 bytes after `wasm-opt`, 2.77 MB more than at 3.3.3. No heavy dependency caused this. Two crates were added to the codec's dependency tree, but none of their code reaches the module, and non-serde code grew by about 5.5 KB. The growth is the serde code for about 95 new request methods and request variants. The largest were the typed decision parameter (about 35 new variants), the decision-log operation, family answers and retrieval-round requests.
- Finding: each wire type is expensive. The request flattens a method enum tagged with separate tag and params fields, so every type decodes through serde's buffered path, which is about half of the module. Each struct variant is decoded through `deserialize_any`, which compiles about 14 small error functions per type. The encoder compiles the `Serialize` code twice: once for the outer writer and once for the buffer it uses for collections of unknown length. Decoding already compiles one copy per type, the fewest serde allows here.
- Finding: measured savings after `wasm-opt`. Encoding the canonical body with an owned `Vec` writer gives the same bytes and saves about 100 KB. A `wasm-opt --outlining` pass saves about 212 KB. Running `-Oz` twice, `--converge`, `--gufa`, `--merge-similar-functions` and `opt-level = "s"` gave no saving or a larger module.
- Decision: both savings are adopted. On 2026-11-16 the owner set the budget to 14,000,000 bytes, from the measured size after `wasm-opt`. The budget may be lowered freely; it is raised only by an owner-approved change to MEDULLA-CONTRACT-R006. Removing wire surface or replacing serde for these types was rejected: the codec must decode every method exactly as the engine does.
