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

## Term decisions

Every term a competency row reads is either reused from an open vocabulary (W3C, schema.org, PROV) or minted in the owning core module. A reused term is not declared again. A minted term sits in the registered module named below, every new class extends a core class, and a `skos:exactMatch` or `skos:closeMatch` mapping names the open-vocabulary term where one exists.

| Term | Rows | Decision |
|---|---|---|
| `opportunityAmount` | R054.1 | mint in commercial-v1.ttl on `:Opportunity`; `skos:closeMatch schema:price` |
| `opportunityOwner` | R054.1 | mint in commercial-v1.ttl on `:Opportunity`; `skos:closeMatch prov:wasAttributedTo` |
| `opportunityStage` | R054.1 | mint in commercial-v1.ttl on `:Opportunity`; no equivalent |
| `contactEmail` | R063.2 | reuse `schema:email` on `:Person`; nothing minted |
| `hostAddress` | R061.1 | mint in infrastructure-v1.ttl on `:Host`; no equivalent |
| `inZone` | R061.1 | mint in infrastructure-v1.ttl on `:DnsRecord`; no equivalent |
| `heartbeatStatus` | R064.1 (read by R057.2, R064.2) | mint in it4it_delivery-v1.ttl on `:Heartbeat`; no equivalent |
| `lastRouteErrorAt` | R064.2 | mint in infrastructure-v1.ttl on `:ProxyRoute`; no equivalent |
| `dp:storedVectorDimension` | R065.1 | mint in data_platform-v1.ttl on `dp:VectorCollection`; no equivalent |
| `sessionOf` | R066.1 | mint in nutrition-v1.ttl on `:WorkoutSession`; no equivalent |
| `performedExercise` | R066.1 | mint in nutrition-v1.ttl on `:WorkoutSession`; no equivalent |
| `completedAt` | R066.1, R066.2 (class declared by R081.8) | reuse `prov:endedAtTime` on `:WorkoutSession`; nothing minted |
| `plannedCalories` | R066.2 | mint in nutrition-v1.ttl on `:MealPlanEntry`; `skos:closeMatch schema:calories` |
| `calorieTarget` | R066.2 | mint in nutrition-v1.ttl on `:MealPlanEntry`; no equivalent |
| `usesImage` (container-to-image link) | R013.1 | mint in infrastructure-v1.ttl, `:Container` to `:ContainerImage`; no equivalent |
| `dp:metricValue` | R014.1 | mint in data_platform-v1.ttl on `dp:EvaluationResult` (`xsd:decimal`); no equivalent |
| `:Fan` (fan term) | R020.1 | mint class in infrastructure-v1.ttl extending `kg:HardwareNode`; no equivalent |
