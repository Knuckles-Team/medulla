//! The archive pass over every durable owner file of a [`RedbBackend`]
//! (`MEDULLA-ENGINE-PARITY-R016.1`).
//!
//! Each live shard and the admin mutation store are reached as `copy_shards`
//! of `backend_backup.rs` reaches them, and each kernel is handed to
//! [`medulla_storage::archive_store_pass`].
use super::*;
use medulla_storage::object::archive::ObjectArchive;
use medulla_storage::{archive_store_pass, ArchivePassReport, ArchivePolicy};

impl RedbBackend {
    /// Run one archive pass over every shard and the admin mutation store,
    /// summing the reports. The first failing store ends the pass with its
    /// `"<wire_code>: <detail>"` text; stores already archived stay archived
    /// and the next pass resumes from each scope's manifest.
    pub async fn archive_pass(
        &self,
        archive: &ObjectArchive,
        tenant: &str,
        policy: &ArchivePolicy,
    ) -> Result<ArchivePassReport, String> {
        let mut total = ArchivePassReport::default();
        for writer in self.shards.iter() {
            let shard = writer
                .shard
                .upgrade()
                .ok_or_else(|| "redb writer thread is gone".to_string())?;
            let report = archive_store_pass(shard.kernel(), archive, tenant, policy).await?;
            total.segments_written += report.segments_written;
            total.records_archived += report.records_archived;
        }
        let report =
            archive_store_pass(self.admin_mutations.kernel(), archive, tenant, policy).await?;
        total.segments_written += report.segments_written;
        total.records_archived += report.records_archived;
        Ok(total)
    }
}

impl RedbBackend {
    /// The last committed sequence of every scope across all shards and the
    /// admin mutation store, keyed by scope key (the larger value wins when a
    /// scope appears in two stores). Restore replay skips the records at or
    /// below it (`MEDULLA-ENGINE-PARITY-R016.2`).
    pub fn last_committed_sequences(
        &self,
    ) -> Result<std::collections::BTreeMap<String, u64>, String> {
        let mut out = std::collections::BTreeMap::<String, u64>::new();
        let mut merge = |found: std::collections::BTreeMap<String, u64>| {
            for (scope, seq) in found {
                let slot = out.entry(scope).or_insert(0);
                *slot = (*slot).max(seq);
            }
        };
        for writer in self.shards.iter() {
            let shard = writer
                .shard
                .upgrade()
                .ok_or_else(|| "redb writer thread is gone".to_string())?;
            merge(medulla_storage::last_committed_sequences(shard.kernel())?);
        }
        merge(medulla_storage::last_committed_sequences(
            self.admin_mutations.kernel(),
        )?);
        Ok(out)
    }
}
