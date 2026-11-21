# MEDULLA-ORCH-VOCAB — Data model

This file names the records and graph terms that the rows add. Each entity has one owner file.

## Entities

### Ontology terms

- Owner: `crates/medulla-core/ontology/orchestration-v1.ttl`, `swarm_topology-v1.ttl`, `capability-v1.ttl` and `sdd-v1.ttl`.
- Rule: a term has one declaration with its domain and range; shapes in the matching `.shapes.ttl` file constrain it.

### RunTrace and StepRecord

- Fields: harness counters, `:terminationReason`, `:runModel`, `:runTaskClass`, `:appliedHarnessConfig`; each step has `step_index`, `kind` and three digests.
- Owner: `crates/medulla-result-types/src/outcome_bundle.rs`.
- Rule: digests come from the one `step_digest` recipe.

### RunObservationWindow and DurableUnit

- Fields: revisioned window with measured-or-unknown fields; a unit with `run_id`, closed `status` and artifact refs.
- Owner: `crates/medulla-work-types/src/run_observation.rs` and `crates/medulla-types/src/durable_unit.rs`.
- Relation: both link to a `:RunTrace`.

### MeasuredModelFact

- Fields: model reference, closed `:measuredProperty`, value, run evidence.
- Relation: a newer fact links to the older one by `:measuredSupersedes`; currency is a champion pointer.

### Definition records

- Records: `PromptTemplate`, `WorkflowPattern`, `OrgChart`, `AgentTwin`, `AgentInstance`, `TeamConfig`, `CapabilityGap`.
- Owner: `crates/medulla-work-types/src/orchestration/`.
- Relation: a stored definition is a revision of the subject named in `MEDULLA-DECISION-ENGINE-R805.10`.

### Harness components and lessons

- Records: `HarnessComponentVersion`, `AdoptedHarness`, `HarnessContrast`, `HarnessLesson`.
- Owner: `crates/medulla-work-types/src/orchestration/mod.rs` and `improvement.rs`.
- Relation: a newer version supersedes an older one by edge; superseded versions stay readable.

### PolicyRule and PolicyCandidate

- Fields: rule id, scope, closed severity, source reference with span, clause digest, status.
- Owner: `crates/medulla-types/src/policy_document.rs`.
- Relation: activation moves the `GovernedSet` pointer of the repository.

## State machine types

- Rows `MEDULLA-ORCH-VOCAB-R811`, `MEDULLA-ORCH-VOCAB-R812`, `MEDULLA-ORCH-VOCAB-R813`: each machine type stores one state field and exposes one `apply` (or `step`) function that returns `TransitionRefused` for an illegal transition.
- Deleted fields: the dual-truth flags and literals named in the rows (for example `terminal`, `tombstone`, `reclaimed`, `cancel_requested`, `active`, `revoked`, `destination_installed`).

### State machine fields

- `WorkItemMachine`. States: `Submitted`, `Ready`, `Leased`, `Running`, `Succeeded`, `Failed`, `Cancelled`, `DeadLetter`. Persisted state: `status` (stored text of `WorkItemStatus`, `crates/medulla-work-types/src/work_item_read.rs:88`). `is_terminal` and `lease_is_live(now)` are derived.

## DMN vocabulary

Owner: MEDULLA-ORCH-VOCAB-R014.3. The lane copies these triples verbatim. Counts: 8 core-foundation triples, 69 module triples (2 header), 32 shape triples.

Append to `crates/medulla-core/ontology/core-foundation-v1.ttl`:

```turtle
:DecisionLogic a owl:Class ;
    rdfs:label "Decision Logic" ;
    rdfs:comment "Governed logic that derives a decision outcome from inputs, such as a decision table." ;
    rdfs:subClassOf :Policy .

:DecisionRule a owl:Class ;
    rdfs:label "Decision Rule" ;
    rdfs:comment "One rule of a decision logic: conditions over inputs and the outcome they select." ;
    rdfs:subClassOf :Policy .
```

`crates/medulla-core/ontology/dmn-v1.ttl`:

```turtle
@prefix : <https://knuckles-team.github.io/ontology/core#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

<https://knuckles-team.github.io/ontology/dmn-v1> a owl:Ontology ;
    rdfs:label "DMN decision classes" .

:DmnDecisionRequirements a owl:Class ;
    rdfs:label "DMN Decision Requirements" ;
    rdfs:comment "A DMN decision requirements graph: the decisions and input data of one model." ;
    rdfs:subClassOf :DecisionLogic .

:DmnDecision a owl:Class ;
    rdfs:label "DMN Decision" ;
    rdfs:comment "One DMN decision: it derives an output from its inputs and the decisions it requires." ;
    rdfs:subClassOf :DecisionLogic .

:DmnDecisionTable a owl:Class ;
    rdfs:label "DMN Decision Table" ;
    rdfs:comment "The decision logic of one decision as ordered rules over input and output clauses." ;
    rdfs:subClassOf :DecisionLogic .

:DmnRule a owl:Class ;
    rdfs:label "DMN Rule" ;
    rdfs:comment "One row of a decision table: an input entry per input clause and an output entry per output clause." ;
    rdfs:subClassOf :DecisionRule .

:DmnInputClause a owl:Class ;
    rdfs:label "DMN Input Clause" ;
    rdfs:comment "One input column of a decision table." ;
    rdfs:subClassOf :Concept .

:DmnOutputClause a owl:Class ;
    rdfs:label "DMN Output Clause" ;
    rdfs:comment "One output column of a decision table." ;
    rdfs:subClassOf :Concept .

:DmnInputData a owl:Class ;
    rdfs:label "DMN Input Data" ;
    rdfs:comment "A named input a decision requires." ;
    rdfs:subClassOf :Concept .

:dmnHitPolicy a owl:DatatypeProperty ;
    rdfs:label "dmnHitPolicy" ;
    rdfs:comment "The hit policy of a decision table: UNIQUE, FIRST, PRIORITY, ANY, COLLECT, RULE ORDER or OUTPUT ORDER. The dmn shapes close the set." ;
    rdfs:domain :DmnDecisionTable ;
    rdfs:range xsd:string .

:dmnAggregation a owl:DatatypeProperty ;
    rdfs:label "dmnAggregation" ;
    rdfs:comment "The aggregation of a COLLECT decision table: SUM, MIN, MAX or COUNT. The dmn shapes close the set." ;
    rdfs:domain :DmnDecisionTable ;
    rdfs:range xsd:string .

:dmnInputEntry a owl:DatatypeProperty ;
    rdfs:label "dmnInputEntry" ;
    rdfs:comment "One input entry of a rule, as its expression text, kept verbatim." ;
    rdfs:domain :DmnRule ;
    rdfs:range xsd:string .

:dmnOutputEntry a owl:DatatypeProperty ;
    rdfs:label "dmnOutputEntry" ;
    rdfs:comment "One output entry of a rule, as its expression text, kept verbatim." ;
    rdfs:domain :DmnRule ;
    rdfs:range xsd:string .

:dmnTypeRef a owl:DatatypeProperty ;
    rdfs:label "dmnTypeRef" ;
    rdfs:comment "The declared type name of a clause or an input data, kept verbatim." ;
    rdfs:range xsd:string .

:dmnRuleIndex a owl:DatatypeProperty ;
    rdfs:label "dmnRuleIndex" ;
    rdfs:comment "The zero-based position of a rule in its decision table." ;
    rdfs:domain :DmnRule ;
    rdfs:range xsd:nonNegativeInteger .

:dmnRequiresDecision a owl:ObjectProperty ;
    rdfs:label "dmnRequiresDecision" ;
    rdfs:comment "A decision requires the output of another decision." ;
    rdfs:domain :DmnDecision ;
    rdfs:range :DmnDecision .

:dmnRequiresInput a owl:ObjectProperty ;
    rdfs:label "dmnRequiresInput" ;
    rdfs:comment "A decision requires an input data." ;
    rdfs:domain :DmnDecision ;
    rdfs:range :DmnInputData .
```

`crates/medulla-core/ontology/dmn-v1.shapes.ttl`:

```turtle
@prefix : <https://knuckles-team.github.io/ontology/core#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

:DmnDecisionTableShape a sh:NodeShape ;
    sh:targetClass :DmnDecisionTable ;
    sh:property [
        sh:path :dmnHitPolicy ;
        sh:maxCount 1 ;
        sh:in ( "UNIQUE" "FIRST" "PRIORITY" "ANY" "COLLECT" "RULE ORDER" "OUTPUT ORDER" ) ;
    ] ;
    sh:property [
        sh:path :dmnAggregation ;
        sh:maxCount 1 ;
        sh:in ( "SUM" "MIN" "MAX" "COUNT" ) ;
    ] .
```
