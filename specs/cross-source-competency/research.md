# cross-source-competency — Research

## Decision: Fragment trace carries authority detail

**Decision.** `FragmentTrace` carries the authority row, rank, mapping generation and pushed fields (`MEDULLA-CQ-XSRC-R005.6`). Source selection passes `Premise::Authority` through to federated `EXPLAIN`.

## R004.4: service resolution approval path
Approval of a link between two attached records is one served method, `ApproveEntityLink`, in the query gateway beside the other staged-graph writes. The server rebuilds the proposal from the two record nodes (node id `<application>:<entity id>`, properties `name` and `host`) with the declared Fellegi-Sunter weights; the caller never supplies a match class or score. The verified caller is the approver. The calibrated probability `1 / (1 + 2^-score)` is classified by the knobs `entity_link.match_min` and `entity_link.nonmatch_max`.
