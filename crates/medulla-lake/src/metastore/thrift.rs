//! Production Hive-metastore client (MEDULLA-UNIFIED-DATA-PLANE-R038.4.3): the small subset of
//! the metastore Thrift service a scan needs (`get_all_tables`, `get_table_req`, `get_partitions`,
//! `get_partitions_by_filter`) over the Thrift binary protocol on a plain TCP connection, plus a
//! directory lister that turns a partition location into its data files.
//!
//! The codec is the strict binary protocol, written out here so the lake crate takes no new
//! dependency. Unknown fields of a reply are skipped, so a newer metastore can add fields.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use super::{
    FileStats, LakeFileFormat, MetastoreClient, MetastoreColumn, MetastoreError, MetastoreTable,
    PartitionInfo, TablePage,
};

const VERSION_1: u32 = 0x8001_0000;
const MSG_CALL: u32 = 1;
const MSG_REPLY: u32 = 2;
const MSG_EXCEPTION: u32 = 3;

const T_STOP: u8 = 0;
const T_BOOL: u8 = 2;
const T_BYTE: u8 = 3;
const T_DOUBLE: u8 = 4;
const T_I16: u8 = 6;
const T_I32: u8 = 8;
const T_I64: u8 = 10;
const T_STRING: u8 = 11;
const T_STRUCT: u8 = 12;
const T_MAP: u8 = 13;
const T_SET: u8 = 14;
const T_LIST: u8 = 15;

/// Deepest nesting a reply may have before it is refused as malformed.
const MAX_DEPTH: u32 = 32;
/// Largest string or collection a reply may declare.
const MAX_LEN: usize = 64 * 1024 * 1024;

/// A decoded Thrift value: only the shapes the metastore subset uses.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Text(String),
    List(Vec<Value>),
    Struct(Vec<(i16, Value)>),
    /// A map, set or double: read and kept opaque.
    Other,
}

impl Value {
    fn field(&self, id: i16) -> Option<&Value> {
        match self {
            Value::Struct(fields) => fields.iter().find(|(i, _)| *i == id).map(|(_, v)| v),
            _ => None,
        }
    }

    fn text(&self, id: i16) -> String {
        match self.field(id) {
            Some(Value::Text(t)) => t.clone(),
            _ => String::new(),
        }
    }

    fn list(&self, id: i16) -> &[Value] {
        match self.field(id) {
            Some(Value::List(items)) => items,
            _ => &[],
        }
    }
}

/// Encoder of one call.
#[derive(Default)]
struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    fn i16(&mut self, v: i16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    fn string(&mut self, s: &str) {
        self.i32(s.len() as i32);
        self.buf.extend_from_slice(s.as_bytes());
    }
    fn field(&mut self, ty: u8, id: i16) {
        self.buf.push(ty);
        self.i16(id);
    }
    fn stop(&mut self) {
        self.buf.push(T_STOP);
    }
    fn message(&mut self, name: &str) {
        self.i32((VERSION_1 | MSG_CALL) as i32);
        self.string(name);
        self.i32(1);
    }
}

/// Decoder over the bytes of one reply.
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

type Decoded<T> = Result<T, String>;

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Decoded<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.data.len());
        match end {
            Some(end) => {
                let out = &self.data[self.at..end];
                self.at = end;
                Ok(out)
            }
            None => Err("reply ended early".to_string()),
        }
    }
    fn u8(&mut self) -> Decoded<u8> {
        Ok(self.take(1)?[0])
    }
    fn i16(&mut self) -> Decoded<i16> {
        let b = self.take(2)?;
        Ok(i16::from_be_bytes([b[0], b[1]]))
    }
    fn i32(&mut self) -> Decoded<i32> {
        let b = self.take(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i64(&mut self) -> Decoded<i64> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(i64::from_be_bytes(a))
    }
    fn len(&mut self) -> Decoded<usize> {
        let n = self.i32()?;
        usize::try_from(n)
            .ok()
            .filter(|n| *n <= MAX_LEN)
            .ok_or_else(|| "reply declares an invalid length".to_string())
    }
    fn string(&mut self) -> Decoded<String> {
        let n = self.len()?;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }

    fn value(&mut self, ty: u8, depth: u32) -> Decoded<Value> {
        if depth > MAX_DEPTH {
            return Err("reply nests too deeply".to_string());
        }
        Ok(match ty {
            T_BOOL => Value::Bool(self.u8()? != 0),
            T_BYTE => Value::Int(i64::from(self.u8()? as i8)),
            T_DOUBLE => {
                self.take(8)?;
                Value::Other
            }
            T_I16 => Value::Int(i64::from(self.i16()?)),
            T_I32 => Value::Int(i64::from(self.i32()?)),
            T_I64 => Value::Int(self.i64()?),
            T_STRING => Value::Text(self.string()?),
            T_STRUCT => {
                let mut fields = Vec::new();
                loop {
                    let field_type = self.u8()?;
                    if field_type == T_STOP {
                        break;
                    }
                    let id = self.i16()?;
                    fields.push((id, self.value(field_type, depth + 1)?));
                }
                Value::Struct(fields)
            }
            T_LIST | T_SET => {
                let element = self.u8()?;
                let n = self.len()?;
                let mut items = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    items.push(self.value(element, depth + 1)?);
                }
                if ty == T_SET {
                    Value::Other
                } else {
                    Value::List(items)
                }
            }
            T_MAP => {
                let key = self.u8()?;
                let value = self.u8()?;
                let n = self.len()?;
                for _ in 0..n {
                    self.value(key, depth + 1)?;
                    self.value(value, depth + 1)?;
                }
                Value::Other
            }
            other => return Err(format!("unknown thrift type {other}")),
        })
    }
}

/// The metastore client of one registered catalog. `database_filter` bounds the databases a
/// caller may name; the endpoint is never part of any refusal.
pub struct HmsClient {
    endpoint: String,
    source_label: String,
    database_filter: Option<String>,
    timeout: Duration,
}

impl HmsClient {
    pub fn new(
        endpoint: impl Into<String>,
        source_label: impl Into<String>,
        database_filter: Option<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            source_label: source_label.into(),
            database_filter,
            timeout,
        }
    }

    fn unreachable(&self) -> MetastoreError {
        MetastoreError::SourceUnreachable {
            source_label: self.source_label.clone(),
        }
    }

    fn admit_database(&self, db: &str) -> Result<(), MetastoreError> {
        match &self.database_filter {
            Some(allowed) if allowed != db => Err(MetastoreError::SpecInvalid(format!(
                "database {db} is outside the source's database filter"
            ))),
            _ => Ok(()),
        }
    }

    /// One call: send the encoded message, read one reply struct and return it. A thrift
    /// application exception or a declared metastore exception (reply field 1) is a refusal.
    fn call(&self, name: &str, args: Writer) -> Result<Value, MetastoreError> {
        let address = self
            .endpoint
            .to_socket_addrs()
            .map_err(|_| self.unreachable())?
            .next()
            .ok_or_else(|| self.unreachable())?;
        let mut stream =
            TcpStream::connect_timeout(&address, self.timeout).map_err(|_| self.unreachable())?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|_| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| self.unreachable())?;
        let mut message = Writer::default();
        message.message(name);
        message.buf.extend_from_slice(&args.buf);
        stream
            .write_all(&message.buf)
            .map_err(|_| self.unreachable())?;
        let mut raw = Vec::new();
        // The reply is one message; the metastore keeps the connection open, so read until
        // the reply struct is complete rather than to end of stream.
        let mut chunk = [0u8; 16 * 1024];
        loop {
            let n = stream.read(&mut chunk).map_err(|_| self.unreachable())?;
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..n]);
            if let Some(reply) = self.try_decode(name, &raw)? {
                return Ok(reply);
            }
        }
        match self.try_decode(name, &raw)? {
            Some(reply) => Ok(reply),
            None => Err(self.unreachable()),
        }
    }

    /// `Ok(None)` while the reply is incomplete.
    fn try_decode(&self, name: &str, raw: &[u8]) -> Result<Option<Value>, MetastoreError> {
        let malformed = |detail: String| {
            MetastoreError::UnsupportedLayout(format!("metastore reply to {name}: {detail}"))
        };
        let mut reader = Reader { data: raw, at: 0 };
        let decoded: Decoded<(u32, Value)> = (|| {
            let head = reader.i32()? as u32;
            if head & 0xffff_0000 != VERSION_1 {
                return Err("not a strict binary protocol reply".to_string());
            }
            let _name = reader.string()?;
            let _seq = reader.i32()?;
            let result = reader.value(T_STRUCT, 0)?;
            Ok((head & 0xff, result))
        })();
        match decoded {
            Err(detail) if detail == "reply ended early" => Ok(None),
            Err(detail) => Err(malformed(detail)),
            Ok((MSG_EXCEPTION, _)) => Err(malformed("the metastore raised an exception".into())),
            Ok((MSG_REPLY, result)) => match result.field(0) {
                Some(value) => Ok(Some(value.clone())),
                None => {
                    // Field 1.. carry the declared exceptions (NoSuchObject, MetaException).
                    Err(malformed("the metastore refused the call".into()))
                }
            },
            Ok(_) => Err(malformed("unexpected message type".into())),
        }
    }

    fn describe(
        &self,
        table: &Value,
        db: &str,
        name: &str,
        partitions: Vec<PartitionInfo>,
    ) -> MetastoreTable {
        let sd = table.field(7).cloned().unwrap_or(Value::Struct(Vec::new()));
        let column = |field: &Value| MetastoreColumn {
            name: field.text(1),
            data_type: field.text(2),
        };
        let format = format_of(&sd.text(3));
        MetastoreTable {
            database: db.to_string(),
            name: name.to_string(),
            columns: sd.list(1).iter().map(column).collect(),
            partition_keys: table.list(8).iter().map(|f| f.text(1)).collect(),
            format,
            location: sd.text(2),
            partitions,
        }
    }

    fn table_value(&self, db: &str, name: &str) -> Result<Value, MetastoreError> {
        let mut args = Writer::default();
        args.field(T_STRUCT, 1);
        args.field(T_STRING, 1);
        args.string(db);
        args.field(T_STRING, 2);
        args.string(name);
        args.stop();
        args.stop();
        let result = self.call("get_table_req", args)?;
        result
            .field(1)
            .cloned()
            .ok_or_else(|| MetastoreError::SpecInvalid(format!("table {db}.{name} does not exist")))
    }

    fn partitions_call(
        &self,
        method: &str,
        db: &str,
        name: &str,
        filter: Option<&str>,
    ) -> Result<Vec<Value>, MetastoreError> {
        let mut args = Writer::default();
        args.field(T_STRING, 1);
        args.string(db);
        args.field(T_STRING, 2);
        args.string(name);
        if let Some(filter) = filter {
            args.field(T_STRING, 3);
            args.string(filter);
        }
        args.field(T_I16, if filter.is_some() { 4 } else { 3 });
        args.i16(-1);
        args.stop();
        match self.call(method, args)? {
            Value::List(items) => Ok(items),
            _ => Ok(Vec::new()),
        }
    }

    fn partition_info(&self, partition: &Value) -> Result<PartitionInfo, MetastoreError> {
        let location = partition.field(6).map(|sd| sd.text(2)).unwrap_or_default();
        Ok(PartitionInfo {
            values: partition
                .list(1)
                .iter()
                .map(|v| match v {
                    Value::Text(t) => t.clone(),
                    _ => String::new(),
                })
                .collect(),
            files: list_files(&location, &self.source_label)?,
            location,
        })
    }
}

fn format_of(input_format: &str) -> LakeFileFormat {
    if input_format.contains("Orc") {
        LakeFileFormat::Orc
    } else if input_format.contains("Avro") {
        LakeFileFormat::Avro
    } else {
        LakeFileFormat::Parquet
    }
}

/// The local path of a warehouse location (`file:` URIs and plain paths).
pub fn local_path(location: &str) -> &str {
    location
        .strip_prefix("file://")
        .or_else(|| location.strip_prefix("file:"))
        .unwrap_or(location)
}

/// The data files below a partition location, recursive; names starting with `_` or `.` are
/// metadata, never data. A location that cannot be listed names the source, never the path.
pub fn list_files(location: &str, source_label: &str) -> Result<Vec<FileStats>, MetastoreError> {
    let unreachable = || MetastoreError::SourceUnreachable {
        source_label: source_label.to_string(),
    };
    let mut files = Vec::new();
    let mut pending = vec![std::path::PathBuf::from(local_path(location))];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|_| unreachable())? {
            let entry = entry.map_err(|_| unreachable())?;
            let hidden = entry.file_name().to_string_lossy().starts_with(['_', '.']);
            let meta = entry.metadata().map_err(|_| unreachable())?;
            if meta.is_dir() {
                if !hidden {
                    pending.push(entry.path());
                }
            } else if !hidden {
                files.push(FileStats {
                    path: entry.path().to_string_lossy().into_owned(),
                    bytes: meta.len(),
                    rows: 0,
                    column_min_max: Default::default(),
                });
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

impl MetastoreClient for HmsClient {
    fn list_tables(
        &self,
        db: &str,
        page: Option<&str>,
        page_size: u64,
    ) -> Result<TablePage, MetastoreError> {
        self.admit_database(db)?;
        let mut args = Writer::default();
        args.field(T_STRING, 1);
        args.string(db);
        args.stop();
        let mut names: Vec<String> = match self.call("get_all_tables", args)? {
            Value::List(items) => items
                .into_iter()
                .filter_map(|v| match v {
                    Value::Text(t) => Some(t),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        names.sort();
        let rest: Vec<String> = names
            .into_iter()
            .filter(|n| page.is_none_or(|cursor| n.as_str() > cursor))
            .collect();
        let size = page_size as usize;
        let tables: Vec<String> = rest.iter().take(size).cloned().collect();
        let next = (rest.len() > size)
            .then(|| tables.last().cloned())
            .flatten();
        Ok(TablePage { tables, next })
    }

    fn table(&self, db: &str, name: &str) -> Result<MetastoreTable, MetastoreError> {
        self.admit_database(db)?;
        let table = self.table_value(db, name)?;
        let keys = table.list(8).len();
        let partitions = if keys == 0 {
            vec![self.unpartitioned(&table)?]
        } else {
            self.partitions_call("get_partitions", db, name, None)?
                .iter()
                .map(|p| self.partition_info(p))
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(self.describe(&table, db, name, partitions))
    }

    fn partitions_by_filter(
        &self,
        db: &str,
        name: &str,
        filter: &str,
    ) -> Result<Vec<PartitionInfo>, MetastoreError> {
        self.admit_database(db)?;
        let table = self.table_value(db, name)?;
        if table.list(8).is_empty() {
            return Ok(vec![self.unpartitioned(&table)?]);
        }
        let method = if filter.is_empty() {
            "get_partitions"
        } else {
            "get_partitions_by_filter"
        };
        self.partitions_call(method, db, name, (!filter.is_empty()).then_some(filter))?
            .iter()
            .map(|p| self.partition_info(p))
            .collect()
    }
}

impl HmsClient {
    /// The databases the source may name: its filter, or every database the metastore holds.
    pub fn databases(&self) -> Result<Vec<String>, MetastoreError> {
        if let Some(db) = &self.database_filter {
            return Ok(vec![db.clone()]);
        }
        let mut args = Writer::default();
        args.stop();
        let mut names: Vec<String> = match self.call("get_all_databases", args)? {
            Value::List(items) => items
                .into_iter()
                .filter_map(|v| match v {
                    Value::Text(t) => Some(t),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        names.sort();
        Ok(names)
    }

    /// An unpartitioned table is one partition without key values at the table location.
    fn unpartitioned(&self, table: &Value) -> Result<PartitionInfo, MetastoreError> {
        let location = table.field(7).map(|sd| sd.text(2)).unwrap_or_default();
        Ok(PartitionInfo {
            values: Vec::new(),
            files: list_files(&location, &self.source_label)?,
            location,
        })
    }
}
