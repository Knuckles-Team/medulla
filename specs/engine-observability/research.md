# MEDULLA-OBSERVE — Research

Governing spec: [spec.md](spec.md). Plan: [plan.md](plan.md).

Each entry states a decision, its rationale and the alternatives considered. All findings are in this platform's terms.

## Techniques

- **Progress witnesses.** A loop proves progress when a monotone value or a cursor changes between passes. A repeated witness is a livelock.
- **Heartbeat watchdogs.** A supervisor compares each job's last progress time with a deadline and flags the silent ones. The flag is a state, not a kill.
- **Structured transition events.** A state machine that logs each accepted and refused transition makes the past of a job readable after the fact.
- **Signal-based sampling.** A timer signal captures each thread's stack at a fixed rate. Folded stacks count identical stacks.
- **Build-id symbolization.** A stripped binary and its separate debug file share an identifier. A tool pairs a stack's offsets with the file of the same identifier.
- **Range seek on an ordered store.** An ordered key-value store resumes a scan from an exclusive key in logarithmic time, so a page costs the page, not the prefix.

## Decisions

### D1. Owner: one new spec; two child-row extensions

- Decision: file the rows in `specs/engine-observability/`, ID `MEDULLA-OBSERVE`. Add `MEDULLA-CONTRACT-R060` for release symbols and `MEDULLA-DURABLE-KERNEL-R873` for the paging guard and seek cursor.
- Rationale: no row owns runtime introspection. Release symbols belong to the release owner. Lazy paging belongs to the durable kernel. Each extension is a new rollup with children, as the spec standard allows.
- Alternative considered: all rows in MEDULLA-OBSERVE. Rejected because it splits one capability across two owners.

### D2. Runtime state is a dedicated admin read, not a graph query

- Decision: serve `Introspect` as one method with a closed section enum.
- Rationale: the data is process state that must answer when the graph store is the thing that is broken. A graph read needs the state lock that the incident held.
- Alternative considered: expose the registries as virtual graphs. Rejected because a stuck state lock blocks the diagnosis.

### D3. A process-wide handle reaches deep crates

- Decision: `medulla_observe::global()` returns the registries. Tests build isolated instances through `Observe::new()`.
- Rationale: `page_in_remaining` lives in `medulla-persistence`. It cannot reach `ServerState`. A global read-mostly handle costs one `OnceLock` load.
- Alternative considered: thread a handle through `ServerState` and every spawn site. Rejected because it touches 150 call sites for no gain in safety.

### D4. The sampler returns offsets, not only names

- Decision: each frame holds module, build-id and offset. Names are added when symbols exist.
- Rationale: the shipped binary is stripped. Offsets plus the symbol artifact of the same build-id resolve offline, with no gdb on the node.
- Alternative considered: ship unstripped binaries. Rejected because the server image grows past the package size limit.

### D5. Seek replaces skip by an exclusive range

- Decision: resume with a range from the exclusive key `(graph, after)` to the exclusive successor of the graph scope `(graph + "\0", "")`.
- Rationale: the key components are `&str`. The smallest string above `graph` is `graph` followed by NUL, so no key of another graph can fall inside the range. The current comment calls this impossible; the bound closes the gap.
- Alternative considered: keep the skip scan and bound it. Rejected because the cost stays quadratic.

### D6. The trace id sits outside the signed envelope

- Decision: `Request.trace_id` is optional and unsigned.
- Rationale: it carries correlation, not authority. Signing it changes the envelope format and breaks every client.
- Alternative considered: put it in the signed claims. Rejected because it forces a coordinated client release.

### D7. Uncoded refusal text stays off the wire

- Decision: keep the rule, add a cause reference.
- Rationale: uncoded text may name request values. The reference lets an operator find the full text in the log without leaking it to the caller.
- Alternative considered: send the text. Rejected for the data-leak risk.
