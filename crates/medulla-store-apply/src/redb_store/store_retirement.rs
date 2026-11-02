use medulla_store::redb_store::store_prelude::*;
use medulla_store::redb_store::*;
use medulla_resource_store::redb_store::capacity_lease;
use medulla_work_store::redb_store::work_item_capability;
use medulla_lane_store::redb_store::development_lane;

/// Retire one graph's owner rows when its scope is retired.
///
/// The payload half of a scope retirement: `MutationKernel::purge_scope_with`
/// and the graft both require it, for the same reason -- retiring a scope's
/// ledger authority while leaving its rows behind would hand the retired
/// generation's data to the next binding of the same logical graph name.
///
/// Every sweep goes through `scoped_owner_table_mut` + `purge_scope_rows`,
/// whose scope comes from the CAPABILITY and never from an argument, so
/// holding one graph's handle can never wipe another's rows in the file they
/// share. It is deliberately not `PhysicalWriteCapability::purge_scoped_rows`:
/// that is the LEDGER sweep, keyed by the 64-hex binding digest, and an owner
/// key leads with the graph NAME -- on these tables it would match nothing and
/// return `Ok(())` having removed no row.
///
/// The twelve file-wide tables are deliberately NOT swept: they belong to the
/// file, not to any one graph, and a retired graph owns no row in them. The
/// catalog row is removed by [`remove_graph_catalog_row`] on the control scope.
pub struct GraphShardRetirement;

impl medulla_storage::OwnerPayloadRetirement<GraphShardOwner> for GraphShardRetirement {
    fn retire_owner_payload(
        &self,
        write: &medulla_storage::PhysicalWriteCapability<'_, GraphShardOwner>,
        _scope: &medulla_mutation_types::mutation_scope::MutationScopeIdentity,
    ) -> Result<(), String> {
        retire_scoped_table(write, NODES)?;
        retire_scoped_table(write, EDGES)?;
        retire_scoped_table(write, LEDGER)?;
        retire_scoped_table(write, SEMANTIC)?;
        retire_scoped_table(write, AUDIT)?;
        retire_scoped_table(write, AUDIT_REQUESTS)?;
        retire_scoped_table(write, PROVENANCE_ANCHOR_MEMBERS)?;
        retire_scoped_table(write, WORK_ITEM_COMMAND_SEQUENCE)?;
        retire_scoped_table(write, RESOURCE_RESERVATIONS)?;
        retire_scoped_table(write, RESOURCE_RESERVATION_TENANT_INDEX)?;
        retire_scoped_table(write, RESOURCE_RESERVATION_ATTEMPTS)?;
        retire_scoped_table(write, RESOURCE_HOSTS)?;
        retire_scoped_table(write, RESOURCE_EXCLUSIVITY)?;
        retire_scoped_table(write, RESOURCE_FAIRNESS)?;
        retire_scoped_table(write, RESOURCE_CONCURRENCY)?;
        retire_scoped_table(write, RESOURCE_ANTI_AFFINITY)?;
        retire_scoped_table(write, RESOURCE_DISK_POLICIES)?;
        retire_scoped_table(write, CHANGE_ENVELOPES)?;
        retire_scoped_table(write, CONTENT_VERSIONS)?;
        retire_scoped_table(write, CHANGE_CURSORS)?;
        retire_scoped_table(write, CHANGE_BLOBS)?;
        retire_scoped_table(write, CHANGE_FEATURES)?;
        retire_scoped_table(write, CHANGE_EVIDENCE)?;
        retire_scoped_table(write, CHANGE_POLICIES)?;
        retire_scoped_table(write, CHANGE_LINEAGE)?;
        capacity_lease::retire_graph_rows(write)?;
        enrichment_budget::retire_graph_rows(write)?;
        work_item_capability::retire_graph_rows(write)?;
        development_lane::retire_graph_rows(write)?;
        Ok(())
    }
}
