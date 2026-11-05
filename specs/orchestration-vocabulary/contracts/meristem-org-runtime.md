# Seam: organization runtime and teams

Provider: medulla MEDULLA-ORCH-VOCAB-R750.751
Consumer: meristem MERISTEM-HARNESS-CYCLE-R074

## Wire shape

Meristem asks `Decide` with kind `recruit` for a role and moves instance state through `Method::Statechart`. Medulla returns the ranked employee or the top legal market offer with its priority bucket, and every excluded gap with its exclusion code.

## Refusal codes

`TWIN_PIN_MISSING`, `STATECHART_NO_TRANSITION`, `INSTANCE_DEFINITION_UNREGISTERED`, the `OfferExclusion` codes of the work market and the team codes `SPECIALIST_NO_EVIDENCE`, `TEAM_CONFIG_DUPLICATE`, `PROMOTION_RECEIPT_MISMATCH` and `SOCIAL_KIND_UNKNOWN`.

## Authoritative types

`OrgChart`, `AgentTwin` and `AgentInstance` in `crates/medulla-work-types/src/orchestration/org.rs`; `TeamConfig` in `crates/medulla-work-types/src/orchestration/teams.rs`.

## Test vectors

The served journeys of R750.753 and R751.753.
