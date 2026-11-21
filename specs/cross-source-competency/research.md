# cross-source-competency — Research

## Decision: the authority premise is served by source coverage, not the fragment trace

**Decision.** A foreign fragment reads the source its statement names and makes no authority choice, so `FragmentTrace` carries no authority row or rank. The authority row, rank and alternates are served once, by `Method::SourceCoverage` (`MEDULLA-SRC-AUTH-R004.6.1`) and the `sources_used` of `Method::CrossSourceQuestion`. `MEDULLA-CQ-XSRC-R005.6` is retired. `FragmentTrace` keeps `mapping_digest` and the pushed request fields.

## R004.4: service resolution approval path
Approval of a link between two attached records is one served method, `ApproveEntityLink`, in the query gateway beside the other staged-graph writes. The server rebuilds the proposal from the two record nodes (node id `<application>:<entity id>`, properties `name` and `host`) with the declared Fellegi-Sunter weights; the caller never supplies a match class or score. The verified caller is the approver. The calibrated probability `1 / (1 + 2^-score)` is classified by the knobs `entity_link.match_min` and `entity_link.nonmatch_max`.
