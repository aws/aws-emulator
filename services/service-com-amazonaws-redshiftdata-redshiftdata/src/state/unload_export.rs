// SPDX-License-Identifier: CC0-1.0

//! `UNLOAD`: the objects an export really writes to Amazon S3, and the manifest
//! that lists them.
//!
//! "UNLOAD … Unloads the result of a query to one or more text files on Amazon
//! S3": the query is RUN on the engine, its rows are rendered in the format the
//! statement asked for, and the result is written into the object store as the
//! objects a reader then fetches ([`super::object_store`]) — so an export
//! reports what it wrote and a client that reassembles the manifest's parts gets
//! the rows the query produced.
//!
//! Two things the statement decides are this module's own:
//!
//! * the NAMES the objects are written under. "Amazon Redshift … automatically
//!   creates files … the file names are written as
//!   `<object_path>/<name_prefix><slice-number>_part_<file-number>`", and a
//!   `MANIFEST` adds "an explicit manifest file … named
//!   `<object_path>/<name_prefix>manifest`" that lists them with the byte count
//!   of each. This emulator runs one slice, so one object carries the rows;
//!   `PARALLEL` therefore changes nothing a reader of the manifest sees.
//! * whether the destination may be written at all: "by default, UNLOAD fails
//!   if it finds files that it would possibly overwrite", and `ALLOWOVERWRITE`
//!   is what lets it.

use super::data_plane::DataPlane;
use super::object_store::{self, S3Failure};
use super::warehouse_statements::{DataFormat, UnloadStatement};

/// What an export that did not write its objects ran into.
#[derive(Debug)]
pub enum ExportFailure {
    /// The cluster's own call into Amazon S3 answered something other than a
    /// write.
    Objects(S3Failure),
    /// The engine refused or interrupted the query being unloaded.
    Engine(rusqlite::Error),
    /// The statement cannot be carried out as written, in the warehouse's own
    /// wording for it.
    Refused(String),
}

impl From<rusqlite::Error> for ExportFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Engine(error)
    }
}

/// What an export wrote.
#[derive(Debug, Default)]
pub struct Exported {
    /// How many rows it wrote, which is what the statement reports as the rows
    /// it affected.
    pub rows: i64,
    /// The keys it wrote, in the order it wrote them.
    pub keys: Vec<String>,
}

/// What an `UNLOAD` whose destination already holds objects reports.
///
/// The engine's own wording for it, which names the option that would let the
/// export overwrite them.
pub fn destination_not_empty_message() -> String {
    "ERROR: Specified unload destination on S3 is not empty. Consider using a different bucket / \
     prefix, manually removing the target files in S3, or using the ALLOWOVERWRITE option."
        .to_string()
}

/// Carry out one `UNLOAD` on `connection`.
pub fn export(
    connection: &rusqlite::Connection,
    plane: &DataPlane,
    statement: &UnloadStatement,
) -> Result<Exported, ExportFailure> {
    let address =
        object_store::split_uri(&statement.destination).ok_or(ExportFailure::Objects(
            S3Failure::NoSuchBucket,
        ))?;
    // Asked BEFORE the query runs, as the real service asks it: an export that
    // would overwrite objects writes nothing and says so.
    if !statement.allow_overwrite && object_store::holds_objects_under(plane, &address) {
        return Err(ExportFailure::Refused(destination_not_empty_message()));
    }
    let (body, written) = rendered_body(connection, statement)?;
    let data_key = format!(
        "{}{SLICE_PREFIX}{}",
        address.key,
        statement.extension.clone().unwrap_or_default()
    );
    let mut objects: Vec<(String, Vec<u8>)> = vec![(data_key.clone(), body.into_bytes())];
    if statement.manifest {
        let manifest = manifest(&address.bucket, &objects);
        objects.push((format!("{}manifest", address.key), manifest.into_bytes()));
    }
    object_store::write_objects(
        plane,
        statement.role_arn.as_deref(),
        &address.bucket,
        &objects,
    )
    .map_err(ExportFailure::Objects)?;
    Ok(Exported {
        rows: written,
        keys: objects.into_iter().map(|(key, _)| key).collect(),
    })
}

/// The name one slice's data object carries after the statement's own prefix:
/// `<slice-number>_part_<file-number>`, with the one slice this emulator runs
/// and the one file it writes.
const SLICE_PREFIX: &str = "0000_part_00";

/// Run the query being unloaded and RENDER its rows into the body of the object
/// as they arrive, answering that body and how many rows it carries.
///
/// Rendered as it is read, so what the bytes bound measures is the OBJECT: a
/// row is written into the body and the body is then checked, and nothing larger
/// than [`object_store::MAX_TRANSFER_BYTES`] is ever held — whatever the format
/// adds per row (a JSON object's column names, a CSV field's quotes, the header
/// line). A single value is measured off the engine's own buffer before it is
/// copied, so one oversized value is refused rather than materialised first.
fn rendered_body(
    connection: &rusqlite::Connection,
    statement: &UnloadStatement,
) -> Result<(String, i64), ExportFailure> {
    let mut prepared = connection.prepare(statement.query.trim())?;
    let columns: Vec<String> = prepared
        .columns()
        .map(|column| column.name().to_string())
        .collect();
    let width = columns.len();
    let null = statement.null_as.clone().unwrap_or_default();
    let delimiter = statement.field_delimiter();
    let mut body = String::new();
    // "HEADER: Adds a header line containing column names at the top of each
    // output file."
    if statement.header && statement.format != DataFormat::Json {
        body.push_str(&line(statement, columns.clone(), delimiter));
        body.push('\n');
    }
    let mut written = 0_i64;
    let mut answered = prepared.query([])?;
    while let Some(row) = answered.next()? {
        // Bounded by ROWS as well as by bytes: a row that renders to almost
        // nothing still costs a line, so a query that produces them without end
        // is stopped by the row count.
        if written >= exported_row_bound() {
            return Err(too_much(format!(
                "this export would write more than the {MAX_EXPORTED_ROWS} rows one statement of \
                 this emulator writes"
            )));
        }
        let mut values: Vec<Option<String>> = Vec::with_capacity(width);
        // What the row has cost so far, counted from the body it will be
        // appended to: every value of the row is charged against the SAME
        // running total, so a wide row of large values is refused at the value
        // that passes the budget rather than after the whole row is resident.
        let mut charged = body.len();
        for index in 0..width {
            let held = row.get_ref(index)?;
            // Measured BEFORE it is copied: a value that cannot fit in what is
            // left of the budget is refused rather than materialised.
            charged = charged.saturating_add(value_bytes(held)).saturating_add(1);
            if charged > object_store::MAX_TRANSFER_BYTES {
                return Err(oversized());
            }
            values.push(cell(held));
        }
        if statement.format == DataFormat::Json {
            body.push_str(&json_line(&columns, &values, &null));
        } else {
            let fields: Vec<String> = values
                .into_iter()
                .map(|value| value.unwrap_or_else(|| null.clone()))
                .collect();
            body.push_str(&line(statement, fields, delimiter));
        }
        body.push('\n');
        // What the budget bounds is the object: the row is rendered and the
        // BODY is then measured, so every byte the format adds is counted.
        if body.len() > object_store::MAX_TRANSFER_BYTES {
            return Err(oversized());
        }
        written = written.saturating_add(1);
    }
    Ok((body, written))
}

/// What an export that would write more than one statement moves reports.
fn oversized() -> ExportFailure {
    too_much(format!(
        "this export would write more than the {} bytes one statement of this emulator writes",
        object_store::MAX_TRANSFER_BYTES
    ))
}

/// Report that an export would write more than one statement of this emulator
/// writes.
fn too_much(detail: String) -> ExportFailure {
    ExportFailure::Objects(S3Failure::TooMuch(detail))
}

/// [`MAX_EXPORTED_ROWS`] as the counter compares against it.
fn exported_row_bound() -> i64 {
    i64::try_from(MAX_EXPORTED_ROWS).unwrap_or(i64::MAX)
}

/// How many bytes one value takes in the object an export writes, read off the
/// engine's own buffer without copying it: a text or a blob its own length (a
/// blob is written hexadecimal, so twice its bytes), and a number the digits it
/// renders as.
fn value_bytes(value: rusqlite::types::ValueRef<'_>) -> usize {
    match value {
        rusqlite::types::ValueRef::Null => 0,
        rusqlite::types::ValueRef::Integer(_) | rusqlite::types::ValueRef::Real(_) => {
            RENDERED_NUMBER_BYTES
        }
        rusqlite::types::ValueRef::Text(text) => text.len(),
        rusqlite::types::ValueRef::Blob(bytes) => bytes.len().saturating_mul(2),
    }
}

/// How many bytes a rendered number is charged: the widest decimal rendering of
/// a 64-bit integer or float, so a number is never under-counted.
const RENDERED_NUMBER_BYTES: usize = 32;

/// How many rows ONE export writes. The rows are a query's, so their number is
/// not bounded by the statement's own length: this is the bound on what one
/// export holds and writes, beside the byte bound every transfer has
/// ([`object_store::MAX_TRANSFER_BYTES`]).
pub const MAX_EXPORTED_ROWS: usize = 1_000_000;

/// One value as the text an export writes for it; `None` for NULL, which the
/// statement's `NULL AS` decides the spelling of.
fn cell(value: rusqlite::types::ValueRef<'_>) -> Option<String> {
    match value {
        rusqlite::types::ValueRef::Null => None,
        rusqlite::types::ValueRef::Integer(number) => Some(number.to_string()),
        rusqlite::types::ValueRef::Real(number) => Some(number.to_string()),
        rusqlite::types::ValueRef::Text(text) => {
            Some(String::from_utf8_lossy(text).into_owned())
        }
        rusqlite::types::ValueRef::Blob(bytes) => Some(::hex::encode(bytes)),
    }
}

/// One rendered line of fields.
///
/// `CSV` quotes a field that carries the delimiter, a quote or a newline, and
/// doubles an embedded quote; `ADDQUOTES` quotes every field. A delimited field
/// that needs no quoting is written as it stands.
fn line(statement: &UnloadStatement, fields: Vec<String>, delimiter: char) -> String {
    let quoted: Vec<String> = fields
        .into_iter()
        .map(|field| {
            let needs_quotes = statement.add_quotes
                || (statement.format == DataFormat::Csv
                    && (field.contains(delimiter)
                        || field.contains('"')
                        || field.contains('\n')
                        || field.contains('\r')));
            if needs_quotes {
                format!("\"{}\"", field.replace('"', "\"\""))
            } else {
                field
            }
        })
        .collect();
    quoted.join(&delimiter.to_string())
}

/// One row of a `FORMAT JSON` export: one JSON object keyed by the column
/// names, which is what Amazon Redshift writes for that format.
fn json_line(columns: &[String], values: &[Option<String>], null: &str) -> String {
    let mut document = ::serde_json::Map::new();
    for (column, value) in columns.iter().zip(values) {
        let value = match value {
            Some(value) => ::serde_json::Value::String(value.clone()),
            None if null.is_empty() => ::serde_json::Value::Null,
            None => ::serde_json::Value::String(null.to_string()),
        };
        document.insert(column.clone(), value);
    }
    ::serde_json::Value::Object(document).to_string()
}

/// The manifest document an export writes beside its data objects.
///
/// "The manifest file is a text file in JSON format that lists the URL of each
/// file that was written to Amazon S3", with the byte count of each
/// (`meta.content_length`), which is what lets a reader fetch the parts and
/// reassemble the unloaded result.
fn manifest(bucket: &str, objects: &[(String, Vec<u8>)]) -> String {
    let entries: Vec<::serde_json::Value> = objects
        .iter()
        .map(|(key, body)| {
            ::serde_json::json!({
                "url": format!("s3://{bucket}/{key}"),
                "meta": {"content_length": body.len()},
            })
        })
        .collect();
    ::serde_json::json!({"entries": entries}).to_string()
}
