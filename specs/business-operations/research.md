# Research: business operations rules

## Where the rule kinds are hosted

- Question: which existing authority hosts deterministic business rules, so that a small model reaches them through the query path and no new method is needed?
- Decision: a rule registry of its own, `crates/medulla-numeric/src/business/registry.rs::evaluate_rule`, one match arm per rule kind, reached only by the UQL `EVALUATE RULE` stage. The array kernels of `NumericEvaluateOp` take inline arrays, while a rule reads graph nodes and must name their revisions, so the two do not share an op enum. The stage is reachable from natural-language queries because they compile to UQL.
- Alternative refused: one served method per question. It breaks the rule that the graph answers questions and it multiplies wire methods.
- Alternative refused: rules inside the ontology as inference rules. The recurrence, matching and reorder kinds need ordered arithmetic that the entailment engine does not provide.
- Determinism: the caller supplies the as-of time; no rule reads a clock or a random source.
