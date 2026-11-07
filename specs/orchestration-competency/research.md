# MEDULLA-CQ-ORCH — Research

## Process compilation as a pipeline of typed forms

- Unknown: whether one canonical workflow can be a checkable normal form for many source formats.
- Finding: a compile pipeline of four stages, each a function over one typed form, composes into one function. Equal results from two sources then follow from equal intermediate forms, and the check is a digest comparison.
- Finding: the diagram of source, canonical and export arrows commutes when each round trip and each cross-format path keeps the digest. Each equality is a finite test per fixture.
- Finding: an idempotent allocation stage is the normal-form test: allocating an already allocated process changes nothing, which is what admission checks.
- Decision: MEDULLA-CQ-ORCH-R701.11 names the stages and forms. MEDULLA-CQ-ORCH-R701.12 proves convergence over a fixture corpus. The existing rules of R701.2 stay as they are.
