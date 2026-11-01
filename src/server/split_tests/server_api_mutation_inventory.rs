//! Cross-crate check: the server-api mutation ledger against raft's native consensus methods (moved up from
//! medulla-server-api because it needs medulla-raft; MEDULLA-CONTRACT-R056).
#![allow(unused_imports)]
use medulla_server_api::mutation::*;
use medulla_server_api::mutation::plan::{self, *};
use std::collections::BTreeSet;

/// Clustered admission has no deferred mutation family. The union of the
/// graph state-machine gateway, bounded native commands, specialized governed
/// envelope/modality commands, self-routed admin handlers, and process
/// shutdown exactly equals the current mutating capability ledger.
#[cfg(feature = "raft")]
#[test]
fn clustered_mutation_inventory_is_complete() {
    use std::collections::BTreeSet;

    let expected: BTreeSet<&'static str> = medulla_capabilities::method_policy_entries()
        .filter(|(_, policy, _)| policy.mutates)
        .map(|(name, _, _)| name)
        .collect();
    let mut covered: BTreeSet<&'static str> = GATEWAY_ROUTED.iter().copied().collect();
    covered.extend(medulla_raft::raft::NATIVE_CONSENSUS_METHODS.iter().copied());
    covered.extend(CONSENSUS_FANOUT_METHODS.iter().copied());
    covered.extend(SELF_ROUTED_ADMIN_METHODS.iter().copied());
    covered.extend(plan::LOCAL_ONLY_METHODS.iter().copied());
    covered.insert("ApplyChangeEnvelope");
    // Drafts become ApplyChangeEnvelope(s) at the request boundary, before
    // consensus routing, so the cluster sees only the governed form.
    covered.insert("ApplyChangeEnvelopeDraft");
    covered.insert("ApplyChangeEnvelopeDrafts");
    covered.insert("SourceIngest");
    covered.insert("IndexRepository");
    covered.insert("EnrichDocument");
    covered.insert("ServedModality");
    covered.insert("Shutdown");
    covered.insert("KgDelegate");
    // RF-020's four agent layers are no longer inserted here: X7 gave them
    // the typed `LOCAL_ONLY_METHODS` classification they always had in
    // substance, so `plan::LOCAL_ONLY_METHODS` above already covers them
    // and a second, silent insert would hide a future regression in it.
    // RF-019. Self-routes in router.rs and commits into its own semantic
    // owner through `medulla-transaction`, so it never reaches a
    // `NativeMutationCommand` or the gateway -- same reason as the four
    // agent layers above.
    covered.insert("SemanticIndex");
    covered.insert("PlacementAdmin");
    // Self-translates into a gateway-routed `Method::AddNode` before this
    // classifier ever runs on it -- see `cluster_mutation_route`'s
    // `RegisterServer` arm (`VolatileControl`) for the full explanation.
    covered.insert("RegisterServer");
    // Same self-translation into gateway-routed graph primitives.
    covered.insert("FleetCatalog");
    covered.insert("PolicyEvolution");
    covered.insert("TelemetryDerive");

    let missing: Vec<_> = expected.difference(&covered).copied().collect();
    let stale: Vec<_> = covered.difference(&expected).copied().collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "cluster mutation inventory drift: missing={missing:?}, stale={stale:?}"
    );

    // The same discriminator explained on `SELF_ROUTED_ADMIN_METHODS`: NOT
    // ALSO present in `raft::NATIVE_CONSENSUS_METHODS`, or `cluster_mutation_route`
    // would be ambiguous about which route wins.
    let native_consensus: BTreeSet<&'static str> = medulla_raft::raft::NATIVE_CONSENSUS_METHODS
        .iter()
        .copied()
        .collect();
    for name in SELF_ROUTED_ADMIN_METHODS
        .iter()
        .chain(plan::LOCAL_ONLY_METHODS)
    {
        assert!(
            !native_consensus.contains(name),
            "'{name}' is both self-routed or local-only and in \
             raft::NATIVE_CONSENSUS_METHODS -- cluster_mutation_route would be ambiguous"
        );
    }
}
