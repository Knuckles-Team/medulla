# unified-data-plane — Research

## Decision: Registration is synchronous, probing is an async state machine

**Decision.** Registration stays a synchronous record write. Probing is a separate async job with the explicit state machine Registered, Probing, Baselined, Drifted or ProbeFailed (`MEDULLA-UNIFIED-DATA-PLANE-R014.2.3`). The SCHEMA_DRIFT baseline is the schema digest captured at the first successful probe. The L3 test runs against containerised MySQL and MariaDB in the nightly lane.
