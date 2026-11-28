//! The reputation relation, split out above the view it feeds.

use super::stat_view::relation;
use medulla_h_decide_base::decide::stat_log_reader::LogReader;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_server_auth::sql_catalog_acl::relations::Relation;
use {
    medulla_h_decide_runlog::decide::stat_reputation::rows,
    medulla_h_decide_runlog::decide::stat_reputation::COLUMNS,
};

/// The `reputation` relation of the reader's visible log.
pub(super) fn reputation_relation(
    store: &AgentLibraryStore,
    reader: &LogReader,
) -> Result<Relation, String> {
    Ok(relation("reputation", COLUMNS, &rows(store, reader)?))
}
