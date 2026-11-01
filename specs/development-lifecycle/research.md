# MEDULLA-SDLC — Research

Governing plan: [plan.md](plan.md). Each entry resolves one Phase 0 unknown of the delivery rows (R019). The entries describe capabilities and techniques. A measurement on this platform replaces each starting value.

## 1. Which events reach a delivery

**Decision.** A delivery follows the proposals it opened. Every event on a followed proposal becomes an input of the delivery. Check and automated-reviewer events never resume an ended delivery. A person's event resumes an ended delivery after mandate revalidation.

**Technique.** Separate routing from authorization. A subscription of the delivery to its proposal routes events. The immutable mandate holds the authority. A follow-up therefore needs no new admission decision, and it grants no new scope. Deduplicate by the provider event ID, because code hosts redeliver events.

**Rejected.** A new admission decision for each follow-up. It costs a model call per comment and adds no safety, because the mandate already bounds the scope.

## 2. Which facts allow merge arming

**Decision.** The verdict is `arm` only with five facts: the delivery opened the proposal, every required gate passed on the head, the required check passed on the head, an owner approval names the head commit, and the change size stays under a ceiling. A moved head or a change request gives `disarm`. The platform principal never merges.

**Technique.** Bind the approval to the head commit digest, not to the proposal. A later push then voids the approval without a second rule. Ask the code host to arm with an expected head, so a race between push and arming fails closed. Withdraw arming before each push.

**Starting value.** The change-size ceiling starts at 400 changed lines. The value covers a typical row with its test.

## 3. How many automated review rounds to address

**Decision.** A delivery addresses at most three rounds per automated reviewer and proposal. A round is one distinct review run, keyed by its run identifier, not one comment.

**Technique.** Treat automated review as fallible input. Verify each finding against the code before a change. Resolve a fixed finding's thread without a reply. Answer a declined finding once with a reason. Count rounds across assignments and restarts, so a new run does not reset the budget.

**Starting value.** Three rounds. Person reviews have no limit.

## 4. How to recover after a restart

**Decision.** An assignment whose lease expired ends as `host_restarted`. A new assignment starts with a recovery note. The note lists each recorded outside effect and its receipt state.

**Technique.** Record each outside effect before it runs and record its receipt after it runs. An effect without a receipt is uncertain. Reconcile it against the code host before any retry. Move unread inputs to the new assignment exactly once.

## 5. How an owner gains trust before unprompted delivery

**Decision.** In mode `development` the loop records `would_dispatch` for an unprompted slice and starts nothing. The owner reads these records and then approves promotion to `evolving`.

**Technique.** Run the decision in shadow before it acts. The shadow record holds the same evidence as a real dispatch, so the owner can compare it with the work the owner would choose.
