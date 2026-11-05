# Seam: skill upgrade admission

Provider: medulla MEDULLA-TYPED-PACKS-R100.101
Consumer: arbora ARBORA-FLEET-R034.9

## Wire shape

Arbora invoke certification calls the served `AgentComponentOp::AdmitSkillUpgrade` with the prior and new content digests, the consumer-declared contract revisions, the observed consumer cohort and, for a breaking change, a declared migration with its receipt. Medulla returns a `SkillAdmission` whose `verdict` is `Compatible`, `BreakingWithMigration` or `BreakingRefused`, with the migration id, the migration digest and the `admission_revision`. Arbora owns its invocation gate; Medulla owns the verdict.

## Refusal codes

`SKILL_BREAKING_NO_MIGRATION`, `SKILL_CONSUMER_CONTRACT_CHANGED` and `SKILL_MIGRATION_RECEIPT_INVALID`, registered in `ServerErrorCode`.

## Authoritative types

`SkillAdmission` and `admit_skill_upgrade` in `crates/medulla-catalog-types/src/skill_admission.rs` (new); the op variant in `crates/medulla-agent-types/src/agent_component.rs`.

## Test vectors

The served vector is named by the test cell of `MEDULLA-TYPED-PACKS-R100.101` in `test-spec.md`.
