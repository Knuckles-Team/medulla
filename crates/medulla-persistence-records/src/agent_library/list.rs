//! Bounded, cursor-paged listing of one tenant's Agent Library heads
//! (MEDULLA-TYPED-PACKS-R011.1).

use medulla_agent_types::agent_library::list::{
    decode_list_cursor, encode_list_cursor, list_page_limit, validate_list_request,
};
use medulla_agent_types::agent_library::{
    AgentLibraryLifecycle, AgentLibraryListEntry, AgentLibraryPage,
};

use super::*;

/// Most heads one listing page may examine, whatever the filter matches. The bound applies to
/// rows examined, not rows matched, so a sparse filter over a large tenant makes progress across
/// pages instead of doing unbounded work in one.
const MAX_AGENT_LIBRARY_LIST_SCAN: usize = 4_096;
/// Most revision-row bytes one listing page may decode.
const MAX_AGENT_LIBRARY_LIST_BYTES: usize = 8 * 1024 * 1024;

impl AgentLibraryStore {
    /// One page of the tenant's agents, in agent id order, each as its head revision.
    ///
    /// A page stops at whichever comes first: the caller's `limit`, the scan bound or the byte
    /// bound. None of them refuses; each returns a cursor that resumes strictly after the last
    /// head the page consumed. The tenant half of every key is the request's, so no cursor can
    /// walk out of the tenant prefix.
    pub fn list(
        &self,
        tenant_id: &str,
        lifecycle: Option<AgentLibraryLifecycle>,
        limit: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<AgentLibraryPage, String> {
        validate_list_request(tenant_id, limit, cursor)?;
        let resume_after = cursor
            .map(|cursor| decode_list_cursor(tenant_id, cursor))
            .transpose()?;
        let read = self.read()?;
        scan_list_page(
            &read,
            tenant_id,
            lifecycle,
            list_page_limit(limit),
            resume_after.as_deref(),
        )
    }
}

fn scan_list_page(
    read: &ScopedRead<'_, medulla_storage::AgentLibraryOwner>,
    tenant_id: &str,
    lifecycle: Option<AgentLibraryLifecycle>,
    limit: usize,
    resume_after: Option<&str>,
) -> Result<AgentLibraryPage, String> {
    let heads = read.open_owner_table(medulla_storage::AGENT_LIBRARY_HEADS)?;
    let revisions = read.open_owner_table(medulla_storage::AGENT_LIBRARY_REVISIONS)?;
    let mut entries = Vec::new();
    let mut scanned = 0usize;
    let mut bytes = 0usize;
    // The last head this page CONSUMED, so the cursor always resumes strictly after a head the
    // caller has been shown. Every bound is checked BEFORE a head is consumed.
    let mut last_consumed: Option<String> = None;
    let mut truncated = false;
    let start = resume_after.unwrap_or("");
    for row in heads
        .range((tenant_id, start)..)
        .map_err(|error| error.to_string())?
    {
        let (key, head_revision) = row.map_err(|error| error.to_string())?;
        let (row_tenant, agent_id) = key.value();
        if row_tenant != tenant_id {
            break;
        }
        // The cursor is EXCLUSIVE; the range start is inclusive.
        if resume_after == Some(agent_id) {
            continue;
        }
        if entries.len() >= limit
            || scanned >= MAX_AGENT_LIBRARY_LIST_SCAN
            || (scanned > 0 && bytes >= MAX_AGENT_LIBRARY_LIST_BYTES)
        {
            truncated = true;
            break;
        }
        scanned += 1;
        let head_revision = head_revision.value();
        let value = revisions
            .get((tenant_id, agent_id, head_revision))
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "agent library head points to a missing revision".to_string())?;
        bytes = bytes.saturating_add(value.value().len());
        let entry = decode_entry(value.value())?;
        validate_entry_key(&entry, tenant_id, agent_id, head_revision)?;
        if lifecycle.is_none_or(|wanted| entry.lifecycle == wanted) {
            entries.push(AgentLibraryListEntry::from(&entry));
        }
        last_consumed = Some(agent_id.to_string());
    }
    let next_cursor = match (truncated, last_consumed) {
        (true, Some(agent_id)) => Some(encode_list_cursor(tenant_id, &agent_id)),
        _ => None,
    };
    Ok(AgentLibraryPage {
        entries,
        next_cursor,
    })
}
