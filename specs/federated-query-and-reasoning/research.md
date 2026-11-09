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

## Evidence forests for context selection

Rows `MEDULLA-FEDERATED-QUERY-R736` to `MEDULLA-FEDERATED-QUERY-R736.5` adopt this technique.

**Decision.** The engine builds a candidate graph from the seed memories. It follows schema relations and semantic relations for a small number of hops. It scores each candidate twice. The first score is direct support: how well the candidate answers the query. The second score is verification cost: how costly it is to check the relation that links the candidate to its parent. The engine keeps the two scores apart so a cheap, well supported candidate wins over a costly one.

**Technique.** Selection is greedy by gain per token. Gain is support minus verification cost. The engine lowers the gain of a candidate that repeats chosen evidence. It zeroes the gain of a candidate that conflicts with chosen evidence. Each chosen candidate hangs from one chosen parent. The result is a forest, and each tree shows how its evidence connects to a seed. Selection stops at the token budget.

**Why.** A flat top-k list repeats near-duplicate facts. It also hides how a fact connects to the query. A forest is compact and explains itself. The sufficiency judge of `MEDULLA-FEDERATED-QUERY-R081` reads the same rows, so no second consumer is needed.

**Rejected.**
- A flat top-k list with a larger k. It spends the budget on duplicates.
- A model call to verify every relation. It adds latency and cost per candidate.
- A second retrieval store for forests. The candidates are graph facts in the one engine.

**Values.** Two hops, a 2000-candidate cap and the penalty weights are starting defaults. The nightly benchmark group `retrieval/evidence_forest` replaces them with measured values. The target is at least 20 percent fewer tokens than top-k at equal recall, p99 150 ms and 128 MiB.
