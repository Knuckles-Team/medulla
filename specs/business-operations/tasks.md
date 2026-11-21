# MEDULLA-BUSINESS-OPS — Implementation tasks

Governing spec: [spec.md](spec.md). Design: [plan.md](plan.md).

Tasks mirror the rows of requirements.md, producer first: vocabulary, then rules, then wiring, then the digest.

- [ ] T001 [US1] MEDULLA-BUSINESS-OPS-R001 Business operations pack (rollup)
- [x] T002 [US1] MEDULLA-BUSINESS-OPS-R001.1 Profile, brand and governance terms
- [ ] T003 [US1] MEDULLA-BUSINESS-OPS-R001.2 Product, variant, batch and lot traceability
- [x] T004 [US1] MEDULLA-BUSINESS-OPS-R001.3 Order, payment, refund and payout terms
- [x] T005 [US1] MEDULLA-BUSINESS-OPS-R001.4 Procurement and stock terms
- [x] T006 [US1] MEDULLA-BUSINESS-OPS-R001.5 Compliance obligation terms
- [x] T007 [US1] MEDULLA-BUSINESS-OPS-R001.6 Tax terms
- [x] T008 [US1] MEDULLA-BUSINESS-OPS-R001.7 Approval, gate decision, deploy and incident terms
- [ ] T009 [US1] MEDULLA-BUSINESS-OPS-R001.8 Every pack class rolls up to core and carries no product name
- [ ] T010 [US1] MEDULLA-BUSINESS-OPS-R001.9 Wiring and hot-path proof: three sources join in one query
- [ ] T011 [US2] MEDULLA-BUSINESS-OPS-R002 Deterministic rule evaluation through the query path (rollup)
- [ ] T012 [US2] MEDULLA-BUSINESS-OPS-R002.1 Rule records
- [ ] T013 [US2] MEDULLA-BUSINESS-OPS-R002.2 Recurrence and compliance calendar
- [ ] T014 [US2] MEDULLA-BUSINESS-OPS-R002.3 Threshold applicability
- [ ] T015 [US2] MEDULLA-BUSINESS-OPS-R002.4 Reconciliation matching
- [ ] T016 [US2] MEDULLA-BUSINESS-OPS-R002.5 Reorder point and quantity
- [ ] T017 [US2] MEDULLA-BUSINESS-OPS-R002.6 Claim-text policy check
- [ ] T018 [US2] MEDULLA-BUSINESS-OPS-R002.7 Wiring: the UQL `EVALUATE` stage reaches the five kinds
- [ ] T019 [US2] MEDULLA-BUSINESS-OPS-R002.8 Hot-path proof: ask in natural language, get the deterministic answer
- [ ] T020 [US3] MEDULLA-BUSINESS-OPS-R003 Owner digest as stored queries (rollup)
- [ ] T021 [US3] MEDULLA-BUSINESS-OPS-R003.1 Pending approvals and overdue obligations
- [ ] T022 [US3] MEDULLA-BUSINESS-OPS-R003.2 Changed in a window
- [ ] T023 [US3] MEDULLA-BUSINESS-OPS-R003.3 Digest respects scope
- [ ] T024 [US2] MEDULLA-BUSINESS-OPS-R002.9 Cross-architecture byte identity
- [ ] T025 [US2] MEDULLA-BUSINESS-OPS-R002.10 No wall-clock read in rule code
