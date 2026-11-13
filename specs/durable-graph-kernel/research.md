# Research: durable graph kernel decisions

## RESP listener enters the served KV methods (MEDULLA-DURABLE-KERNEL-R860.3)

Decision: the RESP listener keeps no store. Every served string command enters the
dispatch through one verified request context built after `AUTH`
(`VerifiedRequestContext::authenticated_resp_actor`, grant `kv:read` and `kv:write`
only, never `kg:*` or `broker:*`), and reaches `KvGet`, `KvPut`, `KvDelete` and `KvCas`.
The principal's private KV scope is the keyspace, so isolation is the engine's own.

Why: one admission and policy path for every wire; one durable store; no second
`kv.redb` with its own physical identity.

Consequences, decided from the north star (one engine, no sync; hard cutover):
- `MULTI`/`EXEC`/`DISCARD` are refused. The served methods commit one key at a time, so
  an atomic multi-command transaction would be a false promise.
- `SET` options (`EX`, `PX`, `NX`, `XX`) are refused; expiry and conditional writes
  have no served KV method.
- `DECR`, `INCRBY`, `MSET`, `SCAN` and `TYPE` are refused rather than half-served.
- The `INCR` retry bound is the registry knob `resp.incr_retry_limit`.
- Request ids start from the process start time in microseconds so the durable mutation
  ledger never sees a repeated idempotency key after a restart.
