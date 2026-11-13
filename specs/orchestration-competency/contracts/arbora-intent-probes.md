# Seam: Intent-surface probes

Provider: medulla MEDULLA-CQ-ORCH-R009.1
Consumer: arbora ARBORA-FLEET-R052.4

## Wire shape

Arbora resolves the plain question through its intent tools and calls the engine path named by the probe row (`NlQuery`, `Decide`, `AgentLibrary` or `Discover`). Medulla returns the answer with provenance; arbora never rewrites it. The arbora companion row `ARBORA-FLEET-R056` proves the same answers through the tools.

## Refusal codes

`USAGE_GUIDANCE_UNKNOWN_CONNECTOR` and `LOAD_SET_NO_DESIGNATED_COMPONENT` from the decision families; an empty graph or an empty library is an empty answer with a premise, never an error. Shared codes are in `MEDULLA-DECISION-ENGINE-R010.100`.

## Authoritative types

The answer types of `crates/medulla-decision-types/src/decision/usage_guidance.rs` and `load_set.rs`, and the `NlQuery` result types of `crates/medulla-protocol-types`.

## Test vectors

The served tests named by the rows of this seam in `test-spec.md` hold the vectors that both sides test against.
