# Research: business operations rules

## Where the rule kinds are hosted

- Question: which existing authority hosts deterministic business rules, so that a small model reaches them through the query path and no new method is needed?
- Decision: the numeric registry of `crates/medulla-numeric` hosts one entry per rule kind, and the UQL `EVALUATE` stage calls it. The registry already serves the shared numeric kernels and is reachable from natural-language queries.
- Alternative refused: one served method per question. It breaks the rule that the graph answers questions and it multiplies wire methods.
- Alternative refused: rules inside the ontology as inference rules. The recurrence, matching and reorder kinds need ordered arithmetic that the entailment engine does not provide.
- Determinism: the caller supplies the as-of time; no rule reads a clock or a random source.
