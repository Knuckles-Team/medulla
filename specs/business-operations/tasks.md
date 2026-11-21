# MEDULLA-BUSINESS-OPS — Implementation tasks

Governing spec: [spec.md](spec.md). Design: [plan.md](plan.md).

Tasks mirror the rows of requirements.md, producer first: vocabulary, then rules, then wiring, then the digest.

- [ ] T001 [US1] MEDULLA-BUSINESS-OPS-R001 Business operations pack (rollup)
- [x] T002 [US1] MEDULLA-BUSINESS-OPS-R001.1 Profile, brand and governance terms
- [x] T003 [US1] MEDULLA-BUSINESS-OPS-R001.2 Add `:Product`, `:ProductVariant`, `:ProductionBatch`, `:InventoryLot`, `:variantOfProduct`, `:batchOfVariant`, `:lotOfBatch`, `:madeFromLot`, `:lotSupplier`, `:lotQuantityUnit`, `:lotStatus` to `business_ops-v1.ttl` (the quantity reuses `:lotQuantity` of `finance-v1.ttl`, not redeclared), range `:lineLot` and the four variant properties, add `kg:InventoryLotShape`, and write `lot_traceability_closure_answers_both_directions`
- [x] T004 [US1] MEDULLA-BUSINESS-OPS-R001.3 Order, payment, refund and payout terms
- [x] T005 [US1] MEDULLA-BUSINESS-OPS-R001.4 Procurement and stock terms
- [x] T006 [US1] MEDULLA-BUSINESS-OPS-R001.5 Compliance obligation terms
- [x] T007 [US1] MEDULLA-BUSINESS-OPS-R001.6 Tax terms
- [x] T008 [US1] MEDULLA-BUSINESS-OPS-R001.7 Approval, gate decision, deploy and incident terms
- [x] T009 [US1] MEDULLA-BUSINESS-OPS-R001.8 Every pack class rolls up to core and carries no product name
- [ ] T010 [US1] MEDULLA-BUSINESS-OPS-R001.9 RETIRED: three-source join proof
- [ ] T011 [US2] MEDULLA-BUSINESS-OPS-R002 Deterministic rule evaluation through the query path (rollup)
- [x] T012 [US2] MEDULLA-BUSINESS-OPS-R002.1 Rule records
- [x] T013 [US2] MEDULLA-BUSINESS-OPS-R002.2 Recurrence and compliance calendar
- [x] T014 [US2] MEDULLA-BUSINESS-OPS-R002.3 Threshold applicability
- [x] T015 [US2] MEDULLA-BUSINESS-OPS-R002.4 Reconciliation matching
- [x] T016 [US2] MEDULLA-BUSINESS-OPS-R002.5 Reorder point and quantity
- [x] T017 [US2] MEDULLA-BUSINESS-OPS-R002.6 Claim-text policy check
- [x] T018 [US2] MEDULLA-BUSINESS-OPS-R002.7 Wiring: the UQL `EVALUATE RULE` stage evaluates one stored rule over the rows it is given
- [ ] T019 [US2] MEDULLA-BUSINESS-OPS-R002.8 RETIRED: natural-language entry to the rule stage
- [ ] T020 [US3] MEDULLA-BUSINESS-OPS-R003 Owner digest as stored queries (rollup)
- [x] T021 [US3] MEDULLA-BUSINESS-OPS-R003.1 Pending approvals and overdue obligations
- [ ] T022 [US3] MEDULLA-BUSINESS-OPS-R003.2 Changed in a window
- [ ] T023 [US3] MEDULLA-BUSINESS-OPS-R003.3 Digest respects scope
- [x] T024 [US2] MEDULLA-BUSINESS-OPS-R002.9 Cross-architecture byte identity
- [x] T025 [US2] MEDULLA-BUSINESS-OPS-R002.10 No wall-clock read in rule code
