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
