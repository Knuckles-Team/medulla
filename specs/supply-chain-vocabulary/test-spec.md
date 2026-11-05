# MEDULLA-SUPPLY-VOCAB — Test specification

Governing spec: [spec.md](spec.md).

| Test ID | Requirement | Level | Setup and input | Expected observation | Evidence |
|---|---|---|---|---|---|
| T-001 | FR-001 | Reasoner | Four-edge physical chain fixture | `suppliesService` inferred for a typed Service via serviceRunsOn. Bare container/pod runsOn does not infer it or a Service type. Nameless supplier refused | PENDING |
| T-001.1 | MEDULLA-SUPPLY-VOCAB-R001.1.1 | Native parser | Exact shared ontology file | Class and superclass triples, labels, suppliesItem domain/range, no suppliedBy redeclaration | `crates/medulla-rdf/tests/supply_chain_declarations.rs` |
| T-001.2 | MEDULLA-SUPPLY-VOCAB-R001.1.2 | Catalog and reasoner | Default catalog plus supplier, item and vendor instances | Valid registered document. Vendor and domain/range entailment. No reverse Supplier inference | PENDING |
| T-002 | FR-002 | Reasoner | Service, workload, image, version fixture | `usesComponent` inferred; versionless component and bad severity refused | PENDING |
| T-003 | FR-003 | Shape | Contract fixtures: valid, no customer, no service; commercial vocabulary scan | Faults refused. The commercial vocabulary does not redeclare `:ChangeRequest`. Canonical `kg:Change` retains its `:BusinessProcess` hierarchy. `affectsService` accepts `kg:Change`. | PENDING |
| T-003.1 | MEDULLA-SUPPLY-VOCAB-R003.7 | Native parser | `commercial-v1.ttl` and fixtures | Test asserts both `Customer` and `Supplier` entail `Organization` and no `:Party` class exists. | `crates/medulla-core/tests/supply_chain_vocab.rs::supplier_and_customer_entail_organization_without_a_party_class` |
| T-003.2 | MEDULLA-SUPPLY-VOCAB-R003.8 | Native parser | `commercial-v1.ttl` and fixtures | Test lists the class; it is declared exactly once. | `crates/medulla-rdf/tests/commercial_declarations.rs::purchase_order_listed_with_exactly_one_declaration` |
| T-003.3 | MEDULLA-SUPPLY-VOCAB-R003.9 | Native parser | `commercial-v1.ttl` and fixtures | Test lists the class and property with domain and range. | `crates/medulla-core/tests/supply_chain_vocab.rs::purchase_invoice_and_invoice_supplier_declared` |
| T-003.4 | MEDULLA-SUPPLY-VOCAB-R003.10 | Native parser | `commercial-v1.ttl` and fixtures | Test lists the class and property with domain and range. | `crates/medulla-rdf/tests/commercial_declarations.rs::opportunity_terms_listed_with_domain_and_range` |
| T-003.5 | MEDULLA-SUPPLY-VOCAB-R003.11 | Native parser | `commercial-v1.ttl` and fixtures | Test lists `contactOf` with domain and range; no `Contact` class exists. | `crates/medulla-core/tests/supply_chain_vocab.rs::contact_of_declared_with_no_contact_class` |
| T-003.6 | MEDULLA-SUPPLY-VOCAB-R003.12 | Native parser | `commercial-v1.ttl` and fixtures | Test lists the property; a fixture with a plain string is refused. | `crates/medulla-core/tests/supply_chain_vocab.rs::last_activity_at_declared_and_datatyped` |
| T-004 | FR-004 | Shape | `HostLimit` fixtures: valid, no limit, unknown kind | Faults refused; no stored `OverLimit` | PENDING |
| T-005 | FR-005 | Shape | Capability link from a service, from a non-service, via `kg:supports` | Only the service form accepted | PENDING |
| T-006 | FR-006 | Contract | D4 table and recorded responses | Every concept has an authority row or a typed abstention | PENDING |
| T-007 | FR-007 | Integration | Blocking-key fixture and a name-only candidate | Proposal inactive until approved; join empty before | PENDING |
| T-008 | SC-001 | Parse | All new `.ttl`, `.rq` | Parse clean; query uses classes only | PENDING |
| T-009 | FR-009 | Unit and served | Five-package chain with one diamond; unknown package | Every dependent with path and severity; typed refusal | PENDING |

## Commercial registry and shape acceptance

Named tests below exist in the tree; the evidence column records no passing result.

| Test ID | Requirement | Level | Setup and input | Expected observation | Evidence |
|---|---|---|---|---|---|
| T-COM-013 | MEDULLA-SUPPLY-VOCAB-R003.13 | Contract | `commercial_vocabulary::commercial_module_loads` in `crates/medulla-core/tests/commercial_vocabulary.rs` | Load through CoreSpec; assert module and catalog registration. Reject a missing or duplicate registration. | PENDING |
| T-COM-014 | MEDULLA-SUPPLY-VOCAB-R003.14 | Contract | `commercial_vocabulary::commercial_shapes_load` in `crates/medulla-core/tests/commercial_vocabulary.rs` | Load the registered companion and existing contract/capability shapes. Reject missing registration or overwritten existing shapes. | PENDING |
| T-COM-015 | MEDULLA-SUPPLY-VOCAB-R003.15 | Contract | `commercial_vocabulary::commercial_shared_namespace` in `crates/medulla-core/tests/commercial_vocabulary.rs` | Check exact IRIs; reject duplicate incompatible declarations and accidental domain narrowing. Existing supplier and customer fixtures remain valid. | PENDING |
| T-COM-016 | MEDULLA-SUPPLY-VOCAB-R003.16 | Contract | `commercial_vocabulary::commercial_close_date_declared` in `crates/medulla-core/tests/commercial_vocabulary.rs` | Assert canonical property IRI, datatype-property type and dateTime range. Reject an incompatible existing declaration before adding a shape. | PENDING |
| T-COM-017 | MEDULLA-SUPPLY-VOCAB-R003.17 | Contract | `commercial_vocabulary::commercial_opportunity_date_shape` in `crates/medulla-core/tests/commercial_vocabulary.rs` | Load the registered shape. Accept a valid dated opportunity; reject missing closeDate and an invalid date datatype. | PENDING |

## Negative and boundary cases

Unapproved mapping makes a class uncovered. Unconnected procurement source gives an abstention. A name-only supplier match across erpnext and dockerhub is never joined before approval. A limit with a reading but no limit is refused.

## Quality and release proof

`cargo test -p medulla-core` for the ontology and shape tests and `uvx pipelines-hook spec-status` and `spec-decomposition`; record revision and output at landing. A fixture test is not served acceptance; served acceptance is `MEDULLA-CQ-XSRC-R007`.
Ecosystem risk: a finding on a leaf package reaches every transitive dependent with its path; an unknown package is refused.

## Planned tests

Each entry below is planned. The test file or function does not exist in the tree yet; the owning change adds it at this exact path.

- `MEDULLA-SUPPLY-VOCAB-R005.9`: `crates/medulla-core/tests/supply_chain_competency.rs::uncovered_contract_abstains_until_link_is_approved` (planned, L3).
- `MEDULLA-SUPPLY-VOCAB-R005`, `MEDULLA-SUPPLY-VOCAB-R006`: `tests/connector_competency/test_competency_served.py::test_case` (planned, L3), owned by the connector competency spec.
- `MEDULLA-SUPPLY-VOCAB-R009.3`: `crates/medulla-h-mining/src/mining/impact/tests.rs::served_impact_lists_dependents` (planned, L3).
- `MEDULLA-SUPPLY-VOCAB-R009`, `MEDULLA-SUPPLY-VOCAB-R009.6`: `crates/medulla-h-mining/src/mining/impact/tests.rs::served_impact_over_dependency_chain` (planned, L3).
- `MEDULLA-SUPPLY-VOCAB-R001.4`, `MEDULLA-SUPPLY-VOCAB-R002.7`: after the shapes companion is wired into the `supply_chain` core source, the existing shape tests in `crates/medulla-core/tests/supply_chain_vocab.rs` also load it through `GraphSchemaSources::default()` (planned extension of `supplier_and_item_shapes_require_a_label`).

