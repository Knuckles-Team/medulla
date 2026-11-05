# MEDULLA-CQ-CONN — Data model

This spec adds ontology terms to registered modules and one served case schema. Entities that it reuses are owned by the modules named in each row.

## Entities

### Competency case

- Fields: `id`, `connector`, `uql`, `columns`, `min_rows`, `recordings`, `abstention_variants`.
- Owner: the case loader of `MEDULLA-CQ-CONN-R067.7`, file `tests/connector_competency/cases/<connector>.json`.
- Rule: a case names no connector in its `uql`; the statement runs over ontology classes only.

### Fixture source

- Fields: one recorded response set per connector, replayed by `FixtureSource` (`MEDULLA-CQ-CONN-R067.8`).
- Relation: a case references the recordings of one or more fixture sources.

### Answer row

- Fields: the case columns plus `source`, `record_id` and `as_of`.
- Rule: a row without these provenance fields is a failure.

### Ontology terms

- Fields: class, datatype property or object property, with domain, range and module.
- Owner: the registered module named in each vocabulary row (`MEDULLA-CQ-CONN-R068` to `MEDULLA-CQ-CONN-R082`); a term lands only in a module that `GraphSchemaSources::default()` registers.

## Proposed terms (owner review)

New ontology terms are an owner decision. Each row keeps its state SPECIFIED and marks the term as a proposed term (owner review). The table lists every proposed term, the rows that read it and the proposed module and class.

| Term | Rows | Proposed module and class |
|---|---|---|
| `opportunityAmount` | R054.1 | commercial-v1.ttl, `:Opportunity` |
| `opportunityOwner` | R054.1 | commercial-v1.ttl, `:Opportunity` |
| `opportunityStage` | R054.1 | commercial-v1.ttl, `:Opportunity` |
| `contactEmail` | R063.2 | commercial-v1.ttl, contact class |
| `hostAddress` | R061.1 | infrastructure-v1.ttl, `:Host` |
| `inZone` | R061.1 | infrastructure-v1.ttl, `:DnsRecord` |
| `heartbeatStatus` | R064.1 (read by R057.2, R064.2) | infrastructure-v1.ttl, `:Heartbeat` |
| `lastRouteErrorAt` | R064.2 | infrastructure-v1.ttl, route or monitor class |
| `dp:storedVectorDimension` | R065.1 | data_platform-v1.ttl, `dp:VectorCollection` |
| `sessionOf` | R066.1 | nutrition-v1.ttl, `:WorkoutSession` |
| `performedExercise` | R066.1 | nutrition-v1.ttl, `:WorkoutSession` |
| `completedAt` | R066.1, R066.2 (class declared by R081.8) | nutrition-v1.ttl, `:WorkoutSession` |
| `plannedCalories` | R066.2 | nutrition-v1.ttl, `:MealPlanEntry` |
| `calorieTarget` | R066.2 | nutrition-v1.ttl, `:MealPlanEntry` |
| container-to-image link | R013.1 | infrastructure-v1.ttl, `:Container` to `:ContainerImage` |
| metric value on `dp:EvaluationResult` | R014.1 | data_platform-v1.ttl, `dp:EvaluationResult` |
| fan term | R020.1 | infrastructure-v1.ttl, fan class or signal kind |
