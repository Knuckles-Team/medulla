# MEDULLA-SDLC — Data model

Governing plan: [plan.md](plan.md). This file lists the records that the delivery rows (R019) add. Each ontology class extends an engine core class.

## Ontology classes

| Class | Parent | Meaning |
|---|---|---|
| `:Delivery` | `:Ticket` | One slice on its way to a merged pull request |
| `:DeliveryAssignment` | `bfo:0000015` | One run bound to a delivery |
| `:DeliveryReport` | `bfo:0000031` | The state an assignment reports |
| `:DeliveryMandate` | `bfo:0000031` | The immutable scope and basis of a delivery |
| `:FeedbackEvent` | `bfo:0000015` | One check, review, comment or merge event of the code host |
| `:Review` | `bfo:0000015` | One review of a proposal by a person or an automated reviewer |
| `:ReviewThread` | `bfo:0000031` | One review thread on a proposal |
| `:MergeArming` | `bfo:0000015` | One arming verdict for a proposal head |

## Relations

| Property | Domain | Range |
|---|---|---|
| `:deliversRequirement` | `:Delivery` | `sdd:Requirement` |
| `:opensProposal`, `:followsProposal` | `:Delivery` | `:CodeChangeProposal` |
| `:assignmentOf` | `:DeliveryAssignment` | `:Delivery` |
| `:assignedRun` | `:DeliveryAssignment` | `:RunTrace` |
| `:reportOf` | `:DeliveryReport` | `:DeliveryAssignment` |
| `:mandateOf` | `:DeliveryMandate` | `:Delivery` |
| `:feedbackFor` | `:FeedbackEvent` | `:Delivery` |
| `:feedbackOn` | `:FeedbackEvent` | `:CodeChangeProposal` |
| `:reviewOf` | `:Review` | `:CodeChangeProposal` |
| `:threadOf` | `:ReviewThread` | `:CodeChangeProposal` |
| `:checkRunOf` | `:CheckRun` | `:CodeChangeProposal` |
| `:armingOf` | `:MergeArming` | `:CodeChangeProposal` |
| `:armedAtHead` | `:MergeArming` | `xsd:string` |

## Rust records

- `Delivery{id, tenant, repo, slice_root, scope_rows, mandate, proposals, follows, assignments, inputs, attention, ended, created_at}`.
- `DeliveryMandate{strength, basis, policy_version, actor, scope_rows, project_mode}`.
- `DeliveryAssignment{id, run_trace_id, state, report, started_at, ended_at, ended_reason}`.
- Closed sets: `DeliveryReportState`, `DeliveryOutcome`, `FeedbackKind` and `ArmingVerdict`.

The derived stage is not a record. `derive_stage` computes it on each read (R019.7).
