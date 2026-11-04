# MEDULLA-FEDERATED-QUERY — Research

Governing plan: [plan.md](plan.md). Each entry describes a capability that rows `MEDULLA-FEDERATED-QUERY-R058.6` to `MEDULLA-FEDERATED-QUERY-R058.9` adopt. The entries describe techniques, not products.

## 1. How an NL answer stays consistent with approved meaning

**Decision.** A governed term carries one certified definition and its lineage. The engine checks each candidate query against the certified definitions of the terms it uses before it runs the query. The result lists those terms, so the caller sees which approved meaning produced the answer (`MEDULLA-FEDERATED-QUERY-R058.6`).

**Technique.** Treat the check as one more typed feedback step in the existing bounded correction loop. A mismatch costs one planner step and never a separate service.

**Rejected.** A second metadata store for definitions. The definitions are graph facts on `:GlossaryTerm`, in the one engine.

## 2. How approved corrections improve later answers

**Decision.** A certifier records a correction as a fact: the question, the rejected query and the approved query (`MEDULLA-FEDERATED-QUERY-R058.7`). Planning context includes matching approved pairs as examples.

**Technique.** Corrections ground planning; they never edit a certified definition. A definition changes only by its own certification.

**Rejected.** Learning from every answer the caller did not reject. Silence is not approval.

## 3. How a multi-turn question becomes plannable

**Decision.** A follow-up is rewritten into one standalone question before planning, and the result discloses the rewrite (`MEDULLA-FEDERATED-QUERY-R058.8`).

**Technique.** One extra planner call, bounded by a turn count. Without history the step is skipped.

## 4. How two sources that disagree on a term are handled

**Decision.** The engine records a conflict fact and treats the term as uncertified until a certifier resolves it (`MEDULLA-FEDERATED-QUERY-R058.9`).

**Rejected.** Automatic merging of definitions. A silent merge produces answers whose meaning nobody approved.
