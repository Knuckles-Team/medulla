# Seam: arbora deployment flow

Provider: medulla MEDULLA-PLACEMENT-R001
Consumer: arbora ARBORA-DEPLOY-R017

## Wire shape

Served `Decide` with `QuestionKind::Placement`: the request body holds the workload demand, the constraints, the candidate option ids (`placement:<target>`) and the priority; the reply is a sealed typed record (chosen target as top `Advisory`, premises, one why-not per non-chosen target) or a typed abstention. Served `AgentAssemble` then takes the chosen option id as the template key `deployment:<target>` and returns the ordered step graph with a certificate. Both methods write nothing.

## Refusal codes

`UNSUPPORTED_PLACEMENT_TARGET`, `CAPACITY_UNAVAILABLE`, `POLICY_LOOSENING`; typed abstentions `Infeasible` (naming the binding constraint), `PolicyLoosening` and `UnknownFact`; `UncoveredCapability` on assembly. A refusal takes no lease and changes no durable state.

## Test vectors

The worked example in plan.md (section A) is the vector for the served journey tests of the placement family.
