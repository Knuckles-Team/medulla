# MEDULLA-CQ-ORCH — Research

## Process compilation as a pipeline of typed forms

- Unknown: whether one canonical workflow can be a checkable normal form for many source formats.
- Finding: a compile pipeline of four stages, each a function over one typed form, composes into one function. Equal results from two sources then follow from equal intermediate forms, and the check is a digest comparison.
- Finding: the diagram of source, canonical and export arrows commutes when each round trip and each cross-format path keeps the digest. Each equality is a finite test per fixture.
- Finding: an idempotent allocation stage is the normal-form test: allocating an already allocated process changes nothing, which is what admission checks.
- Decision: MEDULLA-CQ-ORCH-R701.11 names the stages and forms. MEDULLA-CQ-ORCH-R701.12 proves convergence over a fixture corpus. The existing rules of R701.2 stay as they are.
- Decision: the architecture import commits as one `BatchUpdate` through the commit gateway (the same path as the factor record), so it is durable, replayed and replicated like every other graph write. The export is a read over the caller's projected graph. The canonical model digest excludes the model identifier and name, so the same model exported from two tenants has one digest; the document digest is the SHA-256 of the exact bytes and is what each imported node keeps as `importedFrom`.
- Decision: the document size and depth limits are the registry knobs `process.document_max_bytes` and `process.document_max_depth` (MEDULLA-CQ-ORCH-R702.2); the scheduler-config fields of the same name are removed when MEDULLA-DURABLE-KERNEL-R076.2 moves scheduler settings to knobs.
