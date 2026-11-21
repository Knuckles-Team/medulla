//! The row-page segment: one graph's rows as the paged export form
//! (MEDULLA-OBJECT-STORAGE-BACKEND-R002, R003.2).
//!
//! A [`RowBatch`] is one page of a graph's node, edge and table rows, the unit the
//! graph copy writes as a segment. The version-zero snapshot of a graph the root
//! bootstraps from local data is the same form: the page segments of that one
//! graph, concatenated. The restore reads both through [`decode_row_pages`], so a
//! copied root and a bootstrapped root restore by one decoder.

use serde::{Deserialize, Serialize};

pub use crate::owner::graph_rows::TableRows;

/// Rows of one page.
pub const BATCH_ROWS: usize = 1_000;

/// One page of a graph's rows: `(node_id, node_blob)` and
/// `(source_id, target_id, edge_blob)`, unsealed and in key order, and the rows
/// of every other graph-scoped owner table (`tables`, never `nodes` or `edges`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowBatch {
    pub nodes: Vec<(String, Vec<u8>)>,
    pub edges: Vec<(String, String, Vec<u8>)>,
    pub tables: Vec<TableRows>,
}

impl RowBatch {
    /// Node, edge and table rows this page holds.
    pub fn rows(&self) -> u64 {
        (self.nodes.len()
            + self.edges.len()
            + self.tables.iter().map(|table| table.rows.len()).sum::<usize>()) as u64
    }
}

/// Split every table's rows into pages of at most [`BATCH_ROWS`] rows: the pages
/// that follow a graph's node and edge pages in its snapshot segment.
pub fn table_row_pages(tables: Vec<TableRows>) -> Vec<RowBatch> {
    let mut pages = Vec::new();
    let mut current: Vec<TableRows> = Vec::new();
    let mut budget = BATCH_ROWS;
    for table in tables {
        let mut rows = table.rows.into_iter().peekable();
        while rows.peek().is_some() {
            let chunk: Vec<(Vec<u8>, Vec<u8>)> = rows.by_ref().take(budget).collect();
            budget -= chunk.len();
            current.push(TableRows {
                table: table.table.clone(),
                rows: chunk,
            });
            if budget == 0 {
                pages.push(RowBatch {
                    nodes: Vec::new(),
                    edges: Vec::new(),
                    tables: std::mem::take(&mut current),
                });
                budget = BATCH_ROWS;
            }
        }
    }
    if !current.is_empty() {
        pages.push(RowBatch {
            nodes: Vec::new(),
            edges: Vec::new(),
            tables: current,
        });
    }
    pages
}

/// Append one page to a snapshot segment: the page as a named-field MessagePack
/// map, back to back with the pages before it.
pub fn append_row_page(segment: &mut Vec<u8>, page: &RowBatch) -> Result<(), String> {
    rmp_serde::encode::write_named(segment, page)
        .map_err(|error| format!("row page encode: {error}"))
}

/// Whether a segment is made of row pages. A row page is a MessagePack map; a
/// `BatchSegment` of commit entries is a MessagePack array.
pub fn is_row_page_segment(bytes: &[u8]) -> bool {
    matches!(bytes.first(), Some(0x80..=0x8f | 0xde | 0xdf))
}

/// Decode every row page of a segment, in order. An empty segment holds none.
pub fn decode_row_pages(bytes: &[u8]) -> Result<Vec<RowBatch>, String> {
    let mut cursor = std::io::Cursor::new(bytes);
    let mut pages = Vec::new();
    while (cursor.position() as usize) < bytes.len() {
        let mut decoder = rmp_serde::Deserializer::new(&mut cursor);
        pages.push(
            RowBatch::deserialize(&mut decoder)
                .map_err(|error| format!("row page decode: {error}"))?,
        );
    }
    Ok(pages)
}
