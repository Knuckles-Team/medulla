//! Read-only SQLite `.db` importer: walk `sqlite_schema` (page 1), list user tables,
//! parse their `CREATE TABLE` columns, and depth-first scan their table b-trees in rowid
//! order (resolving overflow chains). Rowid tables only — WITHOUT ROWID and index b-trees
//! are rejected as `Unsupported`, never mis-decoded.
//!
//! The whole file is read into memory once; the caller enforces its own size cap before
//! calling in (mirroring how the facade already bounds `MEDULLA_SQLITE_MAX_BYTES`).

use std::path::Path;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::header::DatabaseHeader;
use crate::overflow::table_leaf_split;
use crate::page::{PageHeader, PageType};
use crate::record::decode_record;
use crate::schema::{
    index_leading_column, is_without_rowid, parse_columns, rowid_alias, SchemaRow,
};
use crate::value::{ColumnDef, Row};
use crate::varint::read_varint;

const SCHEMA_ROOT_PAGE: u32 = 1;

pub struct Reader {
    bytes: Vec<u8>,
    header: DatabaseHeader,
    schema: Vec<SchemaRow>,
}

impl Reader {
    /// Open and validate a `.db` file, reading it fully into memory.
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        Self::from_bytes(bytes)
    }

    /// Open `main` through its `-wal` file as one consistent snapshot, without taking a
    /// lock and without writing either file (MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.1). The
    /// WAL pages overlay the main file's pages, then the one page decoder reads them. A
    /// checkpoint racing the read retries until `busy_timeout`, then refuses with
    /// `SQLITE_SNAPSHOT_UNSTABLE`.
    pub fn open_with_wal(main: &Path, wal: &Path, busy_timeout: Duration) -> Result<Self> {
        Self::open_with_wal_observed(main, wal, busy_timeout, &mut |_| {})
    }

    /// [`Reader::open_with_wal`] with `observe(attempt)` called after each read and before
    /// the salts are re-checked, so a test can race a checkpoint into the read.
    pub fn open_with_wal_observed(
        main: &Path,
        wal: &Path,
        busy_timeout: Duration,
        observe: &mut dyn FnMut(u32),
    ) -> Result<Self> {
        Self::from_bytes(crate::wal::read_stable(main, wal, busy_timeout, observe)?)
    }

    /// Parse an in-memory `.db` image (used by tests and `open`).
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let header = DatabaseHeader::decode(&bytes)?;
        let page_size = header.page_size as usize;
        if !bytes.len().is_multiple_of(page_size) || bytes.len() < page_size {
            return Err(Error::corrupt("file length not a whole number of pages"));
        }
        let mut reader = Reader {
            bytes,
            header,
            schema: Vec::new(),
        };
        // sqlite_schema is a table b-tree rooted at page 1.
        let mut schema = Vec::new();
        for (_, payload) in reader.collect_table_rows(SCHEMA_ROOT_PAGE)? {
            let cols = decode_record(&payload)?;
            schema.push(SchemaRow::from_record(&cols)?);
        }
        reader.schema = schema;
        Ok(reader)
    }

    fn usable(&self) -> usize {
        self.header.usable_space()
    }

    /// Byte slice for 1-indexed `page_no`.
    fn page_slice(&self, page_no: u32) -> Result<&[u8]> {
        if page_no == 0 {
            return Err(Error::corrupt("page 0 is invalid"));
        }
        let page_size = self.header.page_size as usize;
        let start = (page_no as usize - 1) * page_size;
        let end = start + page_size;
        self.bytes
            .get(start..end)
            .ok_or_else(|| Error::corrupt(format!("page {page_no} out of range")))
    }

    fn header_offset(page_no: u32) -> usize {
        if page_no == SCHEMA_ROOT_PAGE {
            100
        } else {
            0
        }
    }

    /// The user tables (skip `sqlite_*`), sorted for determinism — mirrors the SQL filter
    /// `type='table' AND name NOT LIKE 'sqlite_%'`.
    pub fn list_tables(&self) -> Result<Vec<String>> {
        let mut names: Vec<String> = self
            .schema
            .iter()
            .filter(|r| r.kind == "table" && !r.name.starts_with("sqlite_"))
            .map(|r| r.name.clone())
            .collect();
        names.sort();
        Ok(names)
    }

    fn schema_row(&self, table: &str) -> Result<&SchemaRow> {
        let row = self
            .schema
            .iter()
            .find(|r| r.kind == "table" && r.name == table)
            .ok_or_else(|| Error::corrupt(format!("table `{table}` not found")))?;
        if is_without_rowid(&row.sql) {
            return Err(Error::unsupported(format!(
                "table `{table}` is WITHOUT ROWID"
            )));
        }
        Ok(row)
    }

    /// The declared columns of `table`, parsed from its stored `CREATE TABLE` SQL.
    pub fn table_columns(&self, table: &str) -> Result<Vec<ColumnDef>> {
        let row = self.schema_row(table)?;
        parse_columns(&row.sql)
    }

    /// Row count of `table` (walks the b-tree, counting leaf cells).
    pub fn table_row_count(&self, table: &str) -> Result<u64> {
        let root = self.schema_row(table)?.root_page as u32;
        self.count_table_rows(root)
    }

    /// Every row of `table` in rowid order.
    pub fn scan_table(&self, table: &str) -> Result<Vec<Row>> {
        let root = self.schema_row(table)?.root_page as u32;
        let mut out = Vec::new();
        for (_, payload) in self.collect_table_rows(root)? {
            out.push(decode_record(&payload)?);
        }
        Ok(out)
    }

    /// The columns that lead an index on `table`, plus the `INTEGER PRIMARY KEY` rowid
    /// alias, which is indexed by construction.
    pub fn indexed_leading_columns(&self, table: &str) -> Result<Vec<String>> {
        let schema = self.schema_row(table)?;
        let mut out: Vec<String> = rowid_alias(&schema.sql).into_iter().collect();
        out.extend(
            self.schema
                .iter()
                .filter(|r| r.kind == "index" && r.tbl_name == table)
                .filter_map(|r| index_leading_column(&r.sql)),
        );
        Ok(out)
    }

    /// Every row of `table` with its rowid, in rowid order. A column declared
    /// `INTEGER PRIMARY KEY` aliases the rowid: SQLite stores NULL in its record slot, so
    /// that slot is filled from the rowid here, as `SELECT` would return it.
    pub fn scan_table_with_rowids(&self, table: &str) -> Result<Vec<(i64, Row)>> {
        let schema = self.schema_row(table)?;
        let columns = parse_columns(&schema.sql)?;
        let alias =
            rowid_alias(&schema.sql).and_then(|name| columns.iter().position(|c| c.name == name));
        let mut out = Vec::new();
        for (rowid, payload) in self.collect_table_rows(schema.root_page as u32)? {
            let mut row = decode_record(&payload)?;
            if let Some(slot) = alias.filter(|slot| *slot < row.len()) {
                if row[slot] == crate::value::Value::Null {
                    row[slot] = crate::value::Value::Integer(rowid);
                }
            }
            out.push((rowid, row));
        }
        Ok(out)
    }

    /// Depth-first collect `(rowid, payload_bytes)` for every row under `page_no`.
    fn collect_table_rows(&self, page_no: u32) -> Result<Vec<(i64, Vec<u8>)>> {
        let mut out = Vec::new();
        self.walk_table_page(page_no, &mut out, 0)?;
        Ok(out)
    }

    fn walk_table_page(
        &self,
        page_no: u32,
        out: &mut Vec<(i64, Vec<u8>)>,
        depth: usize,
    ) -> Result<()> {
        if depth > 64 {
            return Err(Error::corrupt("b-tree deeper than 64 levels"));
        }
        let page = self.page_slice(page_no)?;
        let hdr = PageHeader::parse(page, Self::header_offset(page_no))?;
        match hdr.page_type {
            PageType::TableLeaf => {
                for idx in 0..hdr.cell_count as usize {
                    let start = hdr.cell_pointer(page, idx)?;
                    let (rowid, payload) = self.read_table_leaf_cell(page, start)?;
                    out.push((rowid, payload));
                }
            }
            PageType::TableInterior => {
                for idx in 0..hdr.cell_count as usize {
                    let start = hdr.cell_pointer(page, idx)?;
                    let cell = page
                        .get(start..)
                        .ok_or_else(|| Error::corrupt("interior cell out of range"))?;
                    if cell.len() < 4 {
                        return Err(Error::corrupt("truncated interior cell"));
                    }
                    let child = u32::from_be_bytes([cell[0], cell[1], cell[2], cell[3]]);
                    self.walk_table_page(child, out, depth + 1)?;
                }
                let rightmost = hdr
                    .rightmost_ptr
                    .ok_or_else(|| Error::corrupt("interior page missing rightmost pointer"))?;
                self.walk_table_page(rightmost, out, depth + 1)?;
            }
            PageType::IndexLeaf | PageType::IndexInterior => {
                return Err(Error::unsupported("index b-tree page in a table scan"));
            }
        }
        Ok(())
    }

    fn count_table_rows(&self, page_no: u32) -> Result<u64> {
        let page = self.page_slice(page_no)?;
        let hdr = PageHeader::parse(page, Self::header_offset(page_no))?;
        match hdr.page_type {
            PageType::TableLeaf => Ok(hdr.cell_count as u64),
            PageType::TableInterior => {
                let mut total = 0u64;
                for idx in 0..hdr.cell_count as usize {
                    let start = hdr.cell_pointer(page, idx)?;
                    let cell = page
                        .get(start..start + 4)
                        .ok_or_else(|| Error::corrupt("interior cell out of range"))?;
                    let child = u32::from_be_bytes([cell[0], cell[1], cell[2], cell[3]]);
                    total += self.count_table_rows(child)?;
                }
                let rightmost = hdr
                    .rightmost_ptr
                    .ok_or_else(|| Error::corrupt("interior page missing rightmost pointer"))?;
                total += self.count_table_rows(rightmost)?;
                Ok(total)
            }
            _ => Err(Error::unsupported("index b-tree page in a table scan")),
        }
    }

    /// Read one table-leaf cell at byte offset `start` within `page`, resolving overflow.
    fn read_table_leaf_cell(&self, page: &[u8], start: usize) -> Result<(i64, Vec<u8>)> {
        let cell = page
            .get(start..)
            .ok_or_else(|| Error::corrupt("cell offset out of range"))?;
        let (payload_size, n1) = read_varint(cell)?;
        let (rowid, n2) = read_varint(&cell[n1..])?;
        let payload_size = payload_size as usize;
        let body = &cell[n1 + n2..];

        let (overflows, local) = table_leaf_split(payload_size, self.usable());
        if !overflows {
            let payload = body
                .get(..payload_size)
                .ok_or_else(|| Error::corrupt("local payload out of range"))?
                .to_vec();
            return Ok((rowid as i64, payload));
        }
        let local_bytes = body
            .get(..local)
            .ok_or_else(|| Error::corrupt("local overflow prefix out of range"))?;
        let ptr_off = local;
        let ptr = body
            .get(ptr_off..ptr_off + 4)
            .ok_or_else(|| Error::corrupt("overflow pointer out of range"))?;
        let first_overflow = u32::from_be_bytes([ptr[0], ptr[1], ptr[2], ptr[3]]);
        let mut payload = Vec::with_capacity(payload_size);
        payload.extend_from_slice(local_bytes);
        self.read_overflow_chain(first_overflow, payload_size - local, &mut payload)?;
        Ok((rowid as i64, payload))
    }

    fn read_overflow_chain(
        &self,
        first: u32,
        mut remaining: usize,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        let chunk = self.usable() - 4;
        let mut page_no = first;
        while remaining > 0 {
            if page_no == 0 {
                return Err(Error::corrupt("overflow chain ended early"));
            }
            let page = self.page_slice(page_no)?;
            let next = u32::from_be_bytes([page[0], page[1], page[2], page[3]]);
            let take = remaining.min(chunk);
            let data = page
                .get(4..4 + take)
                .ok_or_else(|| Error::corrupt("overflow chunk out of range"))?;
            out.extend_from_slice(data);
            remaining -= take;
            page_no = next;
        }
        Ok(())
    }
}
