# Seam contract: autonomous delivery

Governing spec: [spec.md](../spec.md).

Provider: medulla MEDULLA-SDLC-R019.14
Consumer: arbora ARBORA-OPS-R896.7
Consumer: arbora ARBORA-OPS-R896.8
Consumer: arbora ARBORA-OPS-R896.10
Consumer: meristem MERISTEM-SCM-R012.5
Consumer: pipelines PIPE-GATES-R016.116.1

The change-proposal records come from `SCION-SCM-R003.1`. The connector rows `CONN-GITHUB-AGENT-CONNECTOR-CONTENT-R007` and `CONN-GITLAB-API-R004` produce them through their normalize tools. Arbora carries them to the engine unchanged.

## Wire shape

| Method | Request | Answer |
|---|---|---|
| `DeliveryOpen` | `{tenant, repo, dispatch_id, slice_root, scope_rows[], requester, prompted}` | `{delivery_id, assignment_id, idempotent}` or `{would_dispatch, decision_id}` |
| `DeliveryFeedback` | `{tenant, event: ScmFeedbackEvent, admitted_principal?}` | `{event_id, deliveries[], resumed[], verdicts[]}` |
| `DeliveryReport` | `{delivery_id, assignment_id, state, summary, proposal?, question?, checks_run[]}` | `{stage}` |
| `DeliveryStatus` | `{delivery_id}` | `{stage, mandate, proposals[], assignments[], attention?, ended?}` |
| `DeliveryList` | `{tenant, repo?, stage?}` | `[{delivery_id, stage, scope_rows[], proposals[]}]` |
| `DeliveryStop` | `{delivery_id, principal, reason}` | `{outcome: stopped}` |
| `DeliveryRetry` | `{delivery_id, principal}` | `{assignment_id}` |
| `Decide` `MergeArming` | `{delivery_id, proposal, head_sha, approval_ref?}` | `{verdict: arm or disarm or hold, premises[], decision_id}` |

`ScmFeedbackEvent` is `{event_id, provider, kind, proposal, actor, actor_kind, at, body_digest}`. `kind` is one of `check_changed`, `review_submitted`, `review_comment`, `conversation_comment`, `proposal_merged`, `proposal_closed`, `proposal_reopened` and `head_pushed`. `state` is one of `progress`, `waiting`, `ready`, `blocked`, `failed` and `dropped`. The verdicts in a feedback answer are `address`, `stop_addressing` and `close_superseded`.

## Refusal codes

- `DELIVERY_SCOPE_EMPTY`: the slice names no row.
- `MODE_REFERENCE_NO_PROPOSALS`: the codebase is in mode `reference`.
- `FEEDBACK_SOURCE_UNKNOWN`: no registered connector provides the event.
- `FEEDBACK_PROPOSAL_UNKNOWN`: no delivery or record knows the proposal.
- `FEEDBACK_EVENT_INVALID`: the event misses a field or holds an unknown kind.
- `MANDATE_REVOKED`: a resume failed revalidation.
- `ASSIGNMENT_ENDED`: a report names an ended assignment.
- `REPORT_EVIDENCE_MISSING`: a `ready` report lists no checks run.
- `QUESTION_TOO_LONG`: the question exceeds 600 characters.
- `ARMING_APPROVAL_MISSING`: no owner approval names the head commit.
- `ARMING_HEAD_MISMATCH`: the head moved after the approval.
- `DELIVERY_NOT_FOUND`: the delivery is unknown or belongs to another tenant.
- `PERMISSION_DENIED`: the caller lacks `lifecycle:write`.

## Authoritative types

- `crates/medulla-source-types/src/delivery.rs`: `Delivery`, `DeliveryMandate`, `DeliveryAssignment`, `DeliveryReportState`, `DeliveryOutcome`, `FeedbackKind` and `ArmingVerdict`.
- `crates/medulla-protocol-types/src/protocol/method/method_10.rs`: the delivery methods.
- scion-sdk change-proposal records of `SCION-SCM-R003.1`.

## Test vectors

- `tests/sdlc/test_delivery_served.py::test_slice_to_merged_proposal_through_feedback` holds the journey vector.
- `crates/medulla-sdlc/tests/delivery_arming.rs::arm_needs_owner_approval_on_head_and_passing_gates` holds the arming vector.
