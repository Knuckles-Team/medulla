//! Avro reads of the metastore tier (MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.3): the writer schema
//! in each file header is resolved against the reader schema from the metastore columns. The
//! reader schema wins: rows come back in the reader's columns and types, and a reader column
//! the writer never had (an optional field added later) reads as null. Avro files carry no
//! stripe statistics, so partition pruning is the only skip and every opened file is read whole.

use std::sync::Arc;

use apache_avro::schema::Schema;
use apache_avro::types::Value;
use apache_avro::Reader;
use arrow::array::{
    ArrayRef, BinaryBuilder, BooleanBuilder, Float32Builder, Float64Builder, Int32Builder,
    Int64Builder, RecordBatch, StringBuilder,
};
use arrow::datatypes::{DataType, Field, Schema as ArrowSchema};

use super::orc::FileBytes;
use super::scan::{resolve_schema, DataFileSource, OpenedFile, RowFileSource};
use super::{FileStats, MetastoreColumn, MetastoreError};

fn unreadable(detail: impl core::fmt::Display) -> MetastoreError {
    MetastoreError::SchemaIncompatible(format!("avro file is unreadable: {detail}"))
}

/// The hive type name of one Avro schema; a `["null", T]` union is the optional `T`.
fn hive_type(schema: &Schema) -> Option<&'static str> {
    match schema {
        Schema::Boolean => Some("boolean"),
        Schema::Int => Some("int"),
        Schema::Long => Some("bigint"),
        Schema::Float => Some("float"),
        Schema::Double => Some("double"),
        Schema::String => Some("string"),
        Schema::Bytes => Some("binary"),
        Schema::Union(union) => {
            let mut inner = union.variants().iter().filter(|v| **v != Schema::Null);
            match (inner.next(), inner.next()) {
                (Some(only), None) => hive_type(only),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The column names and hive types an Avro file declares in its header.
pub fn avro_schema(file: &[u8]) -> Result<Vec<MetastoreColumn>, MetastoreError> {
    let reader = Reader::new(file).map_err(unreadable)?;
    let Schema::Record(record) = reader.writer_schema() else {
        return Err(unreadable("the writer schema is not a record"));
    };
    record
        .fields
        .iter()
        .map(|field| {
            hive_type(&field.schema)
                .map(|data_type| MetastoreColumn {
                    name: field.name.clone(),
                    data_type: data_type.to_string(),
                })
                .ok_or_else(|| {
                    MetastoreError::SchemaIncompatible(format!(
                        "column {} has an Avro type the reader cannot map",
                        field.name
                    ))
                })
        })
        .collect()
}

fn arrow_type(hive: &str) -> Result<DataType, MetastoreError> {
    Ok(match hive {
        "boolean" => DataType::Boolean,
        "int" => DataType::Int32,
        "bigint" => DataType::Int64,
        "float" => DataType::Float32,
        "double" => DataType::Float64,
        "string" => DataType::Utf8,
        "binary" => DataType::Binary,
        other => {
            return Err(MetastoreError::SchemaIncompatible(format!(
                "reader type {other} has no Avro projection"
            )))
        }
    })
}

/// A value with its union wrapper removed.
fn plain(value: &Value) -> &Value {
    match value {
        Value::Union(_, inner) => plain(inner),
        other => other,
    }
}

fn mismatch(column: &MetastoreColumn) -> MetastoreError {
    MetastoreError::SchemaIncompatible(format!(
        "column {} holds a value that does not resolve to {}",
        column.name, column.data_type
    ))
}

/// One reader column built from the decoded records, promoting writer values to the reader
/// type and reading an absent field as null.
fn column_array(
    column: &MetastoreColumn,
    records: &[Vec<(String, Value)>],
) -> Result<ArrayRef, MetastoreError> {
    fn field<'a>(record: &'a [(String, Value)], name: &str) -> Option<&'a Value> {
        record
            .iter()
            .find(|(field_name, _)| field_name == name)
            .map(|(_, value)| plain(value))
            .filter(|value| !matches!(value, Value::Null))
    }
    macro_rules! build {
        ($builder:expr, $convert:expr) => {{
            let mut builder = $builder;
            for record in records {
                match field(record, &column.name) {
                    None => builder.append_null(),
                    Some(value) => {
                        builder.append_value($convert(value).ok_or_else(|| mismatch(column))?)
                    }
                }
            }
            Arc::new(builder.finish()) as ArrayRef
        }};
    }
    Ok(match arrow_type(&column.data_type)? {
        DataType::Boolean => build!(BooleanBuilder::new(), |v: &Value| match v {
            Value::Boolean(b) => Some(*b),
            _ => None,
        }),
        DataType::Int32 => build!(Int32Builder::new(), |v: &Value| match v {
            Value::Int(i) => Some(*i),
            _ => None,
        }),
        DataType::Int64 => build!(Int64Builder::new(), |v: &Value| match v {
            Value::Int(i) => Some(i64::from(*i)),
            Value::Long(l) => Some(*l),
            _ => None,
        }),
        DataType::Float32 => build!(Float32Builder::new(), |v: &Value| match v {
            Value::Int(i) => Some(*i as f32),
            Value::Long(l) => Some(*l as f32),
            Value::Float(f) => Some(*f),
            _ => None,
        }),
        DataType::Float64 => build!(Float64Builder::new(), |v: &Value| match v {
            Value::Int(i) => Some(f64::from(*i)),
            Value::Long(l) => Some(*l as f64),
            Value::Float(f) => Some(f64::from(*f)),
            Value::Double(d) => Some(*d),
            _ => None,
        }),
        DataType::Utf8 => build!(StringBuilder::new(), |v: &Value| match v {
            Value::String(s) => Some(s.clone()),
            Value::Bytes(b) => String::from_utf8(b.clone()).ok(),
            _ => None,
        }),
        _ => build!(BinaryBuilder::new(), |v: &Value| match v {
            Value::Bytes(b) => Some(b.clone()),
            Value::String(s) => Some(s.clone().into_bytes()),
            _ => None,
        }),
    })
}

/// Read one Avro file in the reader schema. `projection` names the reader columns to return;
/// empty means all of `reader_schema`.
pub fn read_avro(
    file: &[u8],
    reader_schema: &[MetastoreColumn],
    projection: &[String],
) -> Result<(RecordBatch, OpenedFile), MetastoreError> {
    resolve_schema(reader_schema, &avro_schema(file)?)?;
    let columns: Vec<&MetastoreColumn> = reader_schema
        .iter()
        .filter(|c| projection.is_empty() || projection.contains(&c.name))
        .collect();
    let mut records = Vec::new();
    for value in Reader::new(file).map_err(unreadable)? {
        match value.map_err(unreadable)? {
            Value::Record(fields) => records.push(fields),
            _ => return Err(unreadable("a datum is not a record")),
        }
    }
    let fields = columns
        .iter()
        .map(|c| arrow_type(&c.data_type).map(|t| Field::new(&c.name, t, true)))
        .collect::<Result<Vec<_>, _>>()?;
    let arrays = columns
        .iter()
        .map(|c| column_array(c, &records))
        .collect::<Result<Vec<_>, _>>()?;
    let batch =
        RecordBatch::try_new(Arc::new(ArrowSchema::new(fields)), arrays).map_err(unreadable)?;
    let opened = OpenedFile {
        stripes_total: 0,
        stripes_read: 0,
        bytes_read: file.len() as u64,
    };
    Ok((batch, opened))
}

/// Avro data files of one table: the `DataFileSource` of an Avro table.
pub struct AvroFiles<'a> {
    pub bytes: &'a dyn FileBytes,
    /// The reader schema: the metastore's data columns.
    pub reader_schema: Vec<MetastoreColumn>,
    /// Reader columns to return; empty means all.
    pub projection: Vec<String>,
}

impl DataFileSource for AvroFiles<'_> {
    fn writer_schema(&self, file: &FileStats) -> Result<Vec<MetastoreColumn>, MetastoreError> {
        avro_schema(&self.bytes.read(&file.path)?)
    }

    fn open(&self, file: &FileStats) -> Result<OpenedFile, MetastoreError> {
        RowFileSource::read(self, file).map(|(_, opened)| opened)
    }
}

impl RowFileSource for AvroFiles<'_> {
    fn read(&self, file: &FileStats) -> Result<(Vec<RecordBatch>, OpenedFile), MetastoreError> {
        let bytes = self.bytes.read(&file.path)?;
        read_avro(&bytes, &self.reader_schema, &self.projection)
            .map(|(batch, opened)| (vec![batch], opened))
    }
}
