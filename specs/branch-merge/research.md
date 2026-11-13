# Research: branch merge

## R006 time travel: how a past version is read

- Question: the engine keeps only the current state of a graph, so how does `BranchReadAt` answer a version in the past, and what is the diff built from?
- Decision: each branch commit carries the record that produced it as its payload. `BranchWrite` stores the canonical named MessagePack of the mutation (the bytes its manifest digest already covers); a source changeset stores the changeset. The state at a version is the fold of the payloads up to the commit whose `engine_version` is asked for. The diff is the changes of the commits between two versions, each listed against the state before it. One chain, one record per commit, no retained engine snapshots and no second history store.
- Alternative refused: pinning a retained engine snapshot per commit. It costs storage per commit and needs a retention policy the engine does not have; the payload is already the durable record.
- Addressing: versions are engine versions on the chain, not commit ids, because the chain commit names no branch and the engine version is what a reader holds from a write receipt. Two commits at one version resolve to the newest. `from_version` `None` is the empty baseline, `to_version` `None` is the head.
- Entities the feed enumerates: nodes, edges and changeset paths. Any other mutation is named by its method (`other`), so no commit is silently empty.
- Page bound: knob `branch.diff_page_size`.
