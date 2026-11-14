// SPDX-License-Identifier: CC0-1.0

//! `COPY`: the rows a load really reads out of Amazon S3, the rows it rejects,
//! and what it reports for each.
//!
//! A `COPY` is not a statement the SQL engine runs. It is a read of objects out
//! of Amazon S3 under the IAM role the statement names
//! ([`super::object_store`]), a PARSE of what those objects carry against the
//! columns the target table declares, and an insert of the rows that parsed —
//! so this module owns those three things, in that order, and nothing is
//! reported that did not happen:
//!
//! * a load whose own S3 call is refused fails with S3's answer, raised inside
//!   the cluster's request ([`super::object_store::S3Failure::message`]), and
//!   loads nothing;
//! * a row the target's columns cannot take is REJECTED and recorded in the
//!   cluster's `STL_LOAD_ERRORS` ([`super::system_tables`]) with the published
//!   error code and reason for what was wrong with it;
//! * a load that rejected more rows than `MAXERROR` allows FAILS — "if the load
//!   returns the error count, the load fails" — in the engine's own load-failure
//!   wording ([`load_failure_message`]), and leaves the table as it was: the
//!   rows are parsed before any of them is inserted, so a failed load inserts
//!   none of them.
//!
//! The error codes and the reason each one carries are the ones the Amazon
//! Redshift *Load error reference* publishes ([`LOAD_ERRORS`]); the parsing of a
//! character-delimited or CSV object is done by the CSV reader this crate
//! already depends on, so quoting, embedded delimiters and embedded newlines
//! are a CSV implementation's reading of them and not a text scan of this
//! service's own.

use std::time::SystemTime;

use super::data_plane::DataPlane;
use super::object_store::{self, S3Failure};
use super::system_tables::LoadErrorEntry;
use super::warehouse_statements::{CopyStatement, DataFormat};

/// One published load error: the code `STL_LOAD_ERRORS` reports and the reason
/// it carries.
///
/// Source: the Amazon Redshift *Database Developer Guide*, "Load error
/// reference", which lists each code with the error it stands for. The reason
/// recorded for a rejected row is that published description, so a client
/// reading `err_code`/`err_reason` sees what the real service records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadError {
    /// `err_code`.
    pub code: i64,
    /// `err_reason`.
    pub reason: &'static str,
}

/// The load errors this emulator can detect, each as the reference publishes
/// it.
pub const LOAD_ERRORS: &[LoadError] = &[
    LoadError {
        code: 1202,
        reason: "Input data had fewer columns than were defined in the DDL",
    },
    LoadError {
        code: 1203,
        reason: "Input data had more columns than were defined in the DDL",
    },
    LoadError {
        code: 1204,
        reason: "Input data exceeded the acceptable range for the data type",
    },
    LoadError {
        code: 1205,
        reason: "Date format is invalid",
    },
    LoadError {
        code: 1206,
        reason: "Timestamp format is invalid",
    },
    LoadError {
        code: 1207,
        reason: "Data contained a value outside of the expected range of 0-9",
    },
    LoadError {
        code: 1208,
        reason: "FLOAT data type format error",
    },
    LoadError {
        code: 1209,
        reason: "DECIMAL data type format error",
    },
    LoadError {
        code: 1210,
        reason: "BOOLEAN data type format error",
    },
    LoadError {
        code: 1211,
        reason: "Input line contained no data",
    },
    LoadError {
        code: 1216,
        reason: "Invalid input line",
    },
    LoadError {
        code: 1218,
        reason: "When using JSON, parsing failed",
    },
];

/// The code a value a NUMERIC column could not read is rejected under: 1207,
/// "data contained a value outside of the expected range of 0-9" — the family
/// every numeric parse diagnostic belongs to, whatever width or scale the column
/// declares.
pub const NUMERIC_PARSE_ERROR: i64 = 1207;

/// How many VALUES one load holds: the rows it will insert are kept until the
/// load is decided (a failed load inserts none of them), and a row costs one
/// value per column of the target, so what is bounded is the product — the
/// cells — and not the number of rows.
pub const MAX_LOADED_CELLS: usize = 2_000_000;

/// How many rejected rows one load records. Each one carries what
/// `STL_LOAD_ERRORS` reports about it, so the count is bounded here and each
/// entry's own text is cut to the column it goes in
/// ([`super::system_tables`]).
pub const MAX_LOAD_REJECTIONS: usize = 100_000;

/// The published error of this code.
fn published(code: i64) -> LoadError {
    LOAD_ERRORS
        .iter()
        .copied()
        .find(|error| error.code == code)
        .unwrap_or(LoadError {
            // 1200 is the reference's own "Unknown parse error", which is what
            // an error no narrower code covers is recorded as.
            code: 1200,
            reason: "Unknown parse error",
        })
}

/// What a load that did not load its rows ran into.
#[derive(Debug)]
pub enum LoadFailure {
    /// The cluster's own call into Amazon S3 answered something other than the
    /// objects.
    Objects(S3Failure),
    /// The engine refused the read of the target's columns or the insert.
    Engine(rusqlite::Error),
    /// Rows were rejected past what `MAXERROR` allows, so the load failed: the
    /// rejected rows are recorded, and the table is left as it was.
    Rejected(Vec<LoadErrorEntry>),
    /// The statement names something this load cannot resolve — a column the
    /// target does not declare, or a manifest that is not the document the form
    /// takes. The message is the one the warehouse states for it.
    Refused(String),
}

impl From<rusqlite::Error> for LoadFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Engine(error)
    }
}

/// What a load that ran did.
#[derive(Debug, Default)]
pub struct Loaded {
    /// How many rows it inserted.
    pub rows: i64,
    /// The rows it rejected, which `MAXERROR` allowed it to skip.
    pub rejected: Vec<LoadErrorEntry>,
}

/// What a failed load reports as the statement's error.
///
/// "ERROR: Load into table '<table>' failed.  Check 'stl_load_errors' system
/// table for details." — the engine's own wording for a load whose rows could
/// not be loaded, which is what tells a client to go and read the rejected
/// rows. The two spaces after the first sentence are the engine's.
pub fn load_failure_message(table: &str) -> String {
    format!(
        "ERROR: Load into table '{table}' failed.  Check 'stl_load_errors' system table for \
         details."
    )
}

/// Carry out one `COPY` on `connection`.
///
/// `target` is the ENGINE name of the table the rows go into, `table` its name
/// as the statement spelled it (which is what a failure reports), and
/// `query`/`session`/`table_id` the ids the cluster's system tables record a
/// rejected row against.
pub fn load(
    connection: &rusqlite::Connection,
    plane: &DataPlane,
    statement: &CopyStatement,
    target: &str,
    table_id: i64,
    query: i64,
    session: i64,
) -> Result<Loaded, LoadFailure> {
    let started = SystemTime::now();
    let columns = target_columns(connection, target)?;
    let selected = selected_columns(&columns, &statement.columns)?;
    let objects = source_objects(plane, statement).map_err(LoadFailure::Objects)?;

    let mut rows: Vec<Vec<Value>> = Vec::new();
    let mut rejected: Vec<LoadErrorEntry> = Vec::new();
    // A row costs one value per column of the target, so what the load holds is
    // counted in VALUES: a wide table reads fewer rows than a narrow one for the
    // same budget, and neither is bounded by the bytes the objects carried.
    let width = selected.len().max(1);
    for (key, body) in &objects {
        let records = records(statement, body);
        for (line, record) in records {
            if rows.len().saturating_add(1).saturating_mul(width) > MAX_LOADED_CELLS {
                return Err(LoadFailure::Refused(format!(
                    "ERROR: one load of this emulator reads at most {MAX_LOADED_CELLS} values"
                )));
            }
            if rejected.len() >= MAX_LOAD_REJECTIONS {
                return Err(LoadFailure::Refused(format!(
                    "ERROR: one load of this emulator records at most {MAX_LOAD_REJECTIONS} \
                     rejected rows"
                )));
            }
            let filename = format!("s3://{}/{key}", source_bucket(statement));
            match row(statement, &selected, &record) {
                Ok(values) => rows.push(values),
                Err(rejection) => {
                    let reason = rejection.reason();
                    rejected.push(LoadErrorEntry {
                        query,
                        session,
                        table_id,
                        started,
                        filename,
                        line_number: line,
                        column: rejection.column,
                        column_type: rejection.column_type,
                        column_length: rejection.column_length,
                        position: rejection.position,
                        // Cut to the width of the column it is reported in, so
                        // one entry holds what the system table reports about
                        // the row and not the row itself.
                        raw_line: recorded_text(
                            &record.join(&statement.field_delimiter().to_string()),
                        ),
                        raw_field_value: recorded_text(&rejection.value),
                        code: rejection.error.code,
                        reason,
                    });
                }
            }
        }
    }

    // "MAXERROR … If the load returns the error count, the load fails": a load
    // that rejected more rows than it was allowed to inserts none of them.
    if rejected.len() as u64 > statement.max_error {
        return Err(LoadFailure::Rejected(rejected));
    }
    // "NOLOAD … checks the validity of the data file without actually loading
    // the data": the rows were parsed, and that is all this load does.
    if statement.no_load {
        return Ok(Loaded { rows: 0, rejected });
    }
    let inserted = insert(connection, target, &selected, &rows)?;
    Ok(Loaded {
        rows: inserted,
        rejected,
    })
}

/// The columns of the target table, as the ENGINE declares them: each column's
/// name and the type it was declared with.
fn target_columns(
    connection: &rusqlite::Connection,
    target: &str,
) -> rusqlite::Result<Vec<Column>> {
    let quoted = target.replace('"', "\"\"");
    let probe = connection.prepare(&format!("SELECT * FROM \"{quoted}\" LIMIT 0"))?;
    Ok(probe
        .columns()
        .map(|column| Column {
            name: column.name().to_string(),
            declared: column.decl_type().map(str::to_ascii_lowercase),
        })
        .collect())
}

/// One column of the target table.
#[derive(Debug, Clone)]
struct Column {
    /// Its name, as the engine holds it.
    name: String,
    /// The type it was declared with, lower-cased, or `None` for a column
    /// declared without one.
    declared: Option<String>,
}

/// The columns the load writes, in the order the record's fields map onto them:
/// the ones the statement names, or every column of the table "in their default
/// order" when it names none.
fn selected_columns(
    columns: &[Column],
    named: &[String],
) -> Result<Vec<Column>, LoadFailure> {
    if named.is_empty() {
        return Ok(columns.to_vec());
    }
    named
        .iter()
        .map(|name| {
            columns
                .iter()
                .find(|column| column.name.eq_ignore_ascii_case(name))
                .cloned()
                .ok_or_else(|| {
                    LoadFailure::Refused(format!(
                        "ERROR: column \"{name}\" of relation does not exist"
                    ))
                })
        })
        .collect()
}

/// The objects the `FROM` of a load names, read out of the object store.
///
/// A plain address names "the location of the source data": every object whose
/// key starts with it, which is what makes a prefix load a prefix load. A
/// `MANIFEST` names a document that LISTS the objects instead, so the manifest
/// is read and then the objects its entries name are.
fn source_objects(
    plane: &DataPlane,
    statement: &CopyStatement,
) -> Result<Vec<(String, Vec<u8>)>, S3Failure> {
    let address = object_store::split_uri(&statement.source).ok_or(S3Failure::NoSuchBucket)?;
    let role = statement.role_arn.as_deref();
    if !statement.manifest {
        return object_store::read_objects_under(plane, role, &address);
    }
    let manifest = object_store::read_object(plane, role, &address)?;
    let entries = manifest_entries(&manifest)?;
    let mut objects: Vec<(String, Vec<u8>)> = Vec::new();
    // The manifest is a CALLER-SUPPLIED list that need name no distinct object,
    // so what the load holds is counted as the entries are read — one object and
    // one byte at a time, on the same bounds the prefix read enforces — rather
    // than after the whole list has been copied in.
    let mut bytes = 0_usize;
    for (entry, mandatory) in entries {
        let Some(entry_address) = object_store::split_uri(&entry) else {
            if mandatory {
                return Err(S3Failure::NoSuchKey);
            }
            continue;
        };
        if objects.len() >= object_store::MAX_TRANSFER_OBJECTS {
            return Err(S3Failure::TooMuch(format!(
                "the manifest names more than the {} objects one statement of this emulator \
                 moves",
                object_store::MAX_TRANSFER_OBJECTS
            )));
        }
        match object_store::read_object(plane, role, &entry_address) {
            Ok(body) => {
                bytes = bytes.saturating_add(body.len());
                if bytes > object_store::MAX_TRANSFER_BYTES {
                    return Err(S3Failure::TooMuch(format!(
                        "the objects the manifest names are larger than the {} bytes one \
                         statement of this emulator moves",
                        object_store::MAX_TRANSFER_BYTES
                    )));
                }
                objects.push((entry_address.key, body));
            }
            // "mandatory … specifies whether COPY should return an error if the
            // file is not found": an entry that is not mandatory and not there
            // is skipped.
            Err(S3Failure::NoSuchKey | S3Failure::NoSuchBucket) if !mandatory => continue,
            Err(failure) => return Err(failure),
        }
    }
    if objects.is_empty() {
        return Err(S3Failure::NoSuchKey);
    }
    Ok(objects)
}

/// The bucket the load reads from, for the `filename` a rejected row is
/// recorded against.
fn source_bucket(statement: &CopyStatement) -> String {
    object_store::split_uri(&statement.source)
        .map(|address| address.bucket)
        .unwrap_or_default()
}

/// One text a load error records, cut to the width the system table declares
/// for it: `STL_LOAD_ERRORS` reports a rejected row's line and field as
/// `char(1024)`, so nothing larger than that is kept for one.
fn recorded_text(value: &str) -> String {
    value.chars().take(RECORDED_TEXT_LENGTH).collect()
}

/// That declared width.
const RECORDED_TEXT_LENGTH: usize = 1_024;

/// The `url`s a manifest lists, each with whether the load fails if it is not
/// there.
///
/// The document is the one `COPY`'s `MANIFEST` option publishes: an `entries`
/// array whose members carry a `url` and an optional `mandatory`. At most
/// [`object_store::MAX_TRANSFER_OBJECTS`] of them are read, since that is the
/// most objects one statement moves, so the list this returns is bounded by that
/// rather than by the document.
fn manifest_entries(body: &[u8]) -> Result<Vec<(String, bool)>, S3Failure> {
    let document: ::serde_json::Value =
        ::serde_json::from_slice(body).map_err(|_error| S3Failure::NoSuchKey)?;
    let entries = document
        .get("entries")
        .and_then(::serde_json::Value::as_array)
        .ok_or(S3Failure::NoSuchKey)?;
    if entries.len() > object_store::MAX_TRANSFER_OBJECTS {
        return Err(S3Failure::TooMuch(format!(
            "the manifest names {} objects, more than the {} one statement of this emulator \
             moves",
            entries.len(),
            object_store::MAX_TRANSFER_OBJECTS
        )));
    }
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let url = entry
                .get("url")
                .and_then(::serde_json::Value::as_str)?
                .to_string();
            let mandatory = entry
                .get("mandatory")
                .and_then(::serde_json::Value::as_bool)
                .unwrap_or(false);
            Some((url, mandatory))
        })
        .collect())
}

/// The records one object carries, each with its 1-based line number in that
/// object.
///
/// Read by the CSV reader for both delimited formats, so a quoted field, an
/// embedded delimiter and an embedded newline are read as that implementation
/// reads them; a JSON object is read by the JSON reader, one document per line.
fn records(statement: &CopyStatement, body: &[u8]) -> Vec<(i64, Vec<String>)> {
    if statement.format == DataFormat::Json {
        return json_records(body);
    }
    let quoting = statement.format == DataFormat::Csv || statement.remove_quotes;
    let mut reader = ::csv::ReaderBuilder::new()
        .delimiter(delimiter_byte(statement.field_delimiter()))
        .quote(delimiter_byte(statement.quote))
        .quoting(quoting)
        .double_quote(true)
        // Every line is a record, including the first: `IGNOREHEADER` is what
        // says a leading line is a header, and it is applied below.
        .has_headers(false)
        // A record whose field count differs from the first one's is a record
        // this load REJECTS with the published code for it, so the reader must
        // hand it over rather than refuse to read the object.
        .flexible(true)
        .from_reader(body);
    let mut records: Vec<(i64, Vec<String>)> = Vec::new();
    for (index, record) in reader.records().enumerate() {
        let Ok(record) = record else {
            // A record the reader cannot read at all is an invalid input line,
            // which is recorded as such rather than silently dropped.
            records.push((line_number(index), Vec::new()));
            continue;
        };
        records.push((
            line_number(index),
            record.iter().map(str::to_string).collect(),
        ));
    }
    let skipped = usize::try_from(statement.ignore_header).unwrap_or(usize::MAX);
    records
        .into_iter()
        .skip(skipped)
        .filter(|(_, fields)| {
            !(statement.ignore_blank_lines
                && fields.iter().all(|field| field.trim().is_empty()))
        })
        .collect()
}

/// The 1-based line number of the record at `index`.
fn line_number(index: usize) -> i64 {
    i64::try_from(index.saturating_add(1)).unwrap_or(i64::MAX)
}

/// The records a JSON object carries: one document per line, each field of it
/// kept under its own name, which is what `JSON 'auto'` maps onto the columns of
/// the same name.
fn json_records(body: &[u8]) -> Vec<(i64, Vec<String>)> {
    let text = String::from_utf8_lossy(body);
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            let fields = match ::serde_json::from_str::<::serde_json::Value>(line) {
                Ok(::serde_json::Value::Object(members)) => members
                    .into_iter()
                    .map(|(name, value)| format!("{name}\u{1}{}", json_scalar(&value)))
                    .collect(),
                // A line that is not one JSON object is a record with no
                // fields, which the row reader rejects with the published code
                // for a JSON parse that failed.
                _ => Vec::new(),
            };
            (line_number(index), fields)
        })
        .collect()
}

/// One JSON value as the field value it loads as.
fn json_scalar(value: &::serde_json::Value) -> String {
    match value {
        ::serde_json::Value::Null => String::new(),
        ::serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The byte a delimiter or quote character is read as. The option takes "a
/// single ASCII character", so a character outside ASCII is read as the format's
/// own default rather than as a truncated byte.
fn delimiter_byte(character: char) -> u8 {
    u8::try_from(character as u32).unwrap_or(b'|')
}

/// Why one record was rejected.
#[derive(Debug)]
struct Rejection {
    /// The published error.
    error: LoadError,
    /// The column the error is about, or empty when it is about the line.
    column: String,
    /// That column's declared type.
    column_type: String,
    /// Its declared length, where the type carries one.
    column_length: String,
    /// Where in the field the error is.
    position: i64,
    /// The field value that led to it.
    value: String,
    /// The reason to record instead of the published description, for an error
    /// the engine words from the value it stopped on.
    worded: Option<String>,
}

impl Rejection {
    /// The same rejection, carrying the engine's own wording of why the value
    /// was rejected.
    fn with_reason(mut self, reason: String) -> Self {
        self.worded = Some(reason);
        self
    }

    /// The reason to record: the engine's own wording where it has one, and the
    /// published description of the error otherwise.
    fn reason(&self) -> String {
        self.worded
            .clone()
            .unwrap_or_else(|| self.error.reason.to_string())
    }
}

/// One value a load inserts.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
}

impl rusqlite::ToSql for Value {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(match self {
            Self::Null => rusqlite::types::ToSqlOutput::from(rusqlite::types::Null),
            Self::Integer(value) => rusqlite::types::ToSqlOutput::from(*value),
            Self::Real(value) => rusqlite::types::ToSqlOutput::from(*value),
            Self::Text(value) => rusqlite::types::ToSqlOutput::from(value.as_str()),
        })
    }
}

/// Read one record as the row it loads, or report why it is rejected.
///
/// The rejection is BOXED: it carries everything the load error records about
/// the row, which is far larger than the row's values, and every record read
/// returns one of these.
fn row(
    statement: &CopyStatement,
    columns: &[Column],
    record: &[String],
) -> Result<Vec<Value>, Box<Rejection>> {
    if record.is_empty() {
        return Err(Box::new(Rejection {
            error: published(if statement.format == DataFormat::Json {
                1218
            } else {
                1211
            }),
            column: String::new(),
            column_type: String::new(),
            column_length: String::new(),
            position: 0,
            value: String::new(),
            worded: None,
        }));
    }
    let fields = if statement.format == DataFormat::Json {
        json_fields(columns, record)
    } else {
        match record.len().cmp(&columns.len()) {
            std::cmp::Ordering::Greater => {
                return Err(Box::new(Rejection {
                    error: published(1203),
                    column: String::new(),
                    column_type: String::new(),
                    column_length: String::new(),
                    position: 0,
                    value: record.join(&statement.field_delimiter().to_string()),
                    worded: None,
                }))
            }
            // "FILLRECORD … allows data files to be loaded when contiguous
            // columns are missing at the end of some of the records": without
            // it, a short record is the published fewer-columns error.
            std::cmp::Ordering::Less if !statement.fill_record => {
                return Err(Box::new(Rejection {
                    error: published(1202),
                    column: String::new(),
                    column_type: String::new(),
                    column_length: String::new(),
                    position: 0,
                    value: record.join(&statement.field_delimiter().to_string()),
                    worded: None,
                }))
            }
            _ => {
                let mut fields: Vec<Option<String>> =
                    record.iter().map(|field| Some(field.clone())).collect();
                fields.resize(columns.len(), None);
                fields
            }
        }
    };
    columns
        .iter()
        .zip(fields)
        .map(|(column, field)| value(statement, column, field.as_deref()))
        .collect()
}

/// The fields of a JSON record, mapped onto the columns by NAME — which is what
/// `JSON 'auto'` does: "COPY … maps the data elements in the JSON source data to
/// the columns in the target table … by matching object keys … to the names of
/// columns".
fn json_fields(columns: &[Column], record: &[String]) -> Vec<Option<String>> {
    columns
        .iter()
        .map(|column| {
            record.iter().find_map(|field| {
                let (name, value) = field.split_once('\u{1}')?;
                name.eq_ignore_ascii_case(&column.name)
                    .then(|| value.to_string())
            })
        })
        .collect()
}

/// Read one field as the value it loads into `column`, or report why the row is
/// rejected.
fn value(
    statement: &CopyStatement,
    column: &Column,
    field: Option<&str>,
) -> Result<Value, Box<Rejection>> {
    let declared = column.declared.as_deref().unwrap_or_default();
    let class = TypeClass::of(declared);
    let Some(field) = field else {
        return Ok(Value::Null);
    };
    if is_null(statement, field, class) {
        return Ok(Value::Null);
    }
    let rejected = |code: i64, position: i64| {
        Box::new(Rejection {
            error: published(code),
            column: column.name.clone(),
            column_type: type_name(declared, class).to_string(),
            column_length: declared_length(declared)
                .map(|length| length.to_string())
                .unwrap_or_default(),
            position,
            value: field.to_string(),
            worded: None,
        })
    };
    // A numeric field the column cannot take is rejected with the engine's own
    // parse diagnostic, which names the character it stopped on, where it is and
    // the type it was reading for.
    let numeric = |code: i64| {
        let (reason, position) = numeric_diagnostic(
            field,
            numeric_label(declared, class),
            class != TypeClass::Integer,
        );
        Box::new(
            Rejection {
                error: LoadError {
                    code,
                    reason: "",
                },
                column: column.name.clone(),
                column_type: type_name(declared, class).to_string(),
                column_length: declared_length(declared)
                    .map(|length| length.to_string())
                    .unwrap_or_default(),
                position,
                value: field.to_string(),
                worded: None,
            }
            .with_reason(reason),
        )
    };
    match class {
        // A value a numeric column cannot read is rejected under the code for
        // data "outside of the expected range of 0-9", whichever width the
        // column declares: what the engine reports is the character its parse
        // stopped on, and that is the error that family covers.
        TypeClass::Integer => field
            .trim()
            .parse::<i64>()
            .map(Value::Integer)
            .map_err(|_error| numeric(NUMERIC_PARSE_ERROR)),
        TypeClass::Decimal => field
            .trim()
            .parse::<f64>()
            .map(Value::Real)
            .map_err(|_error| numeric(NUMERIC_PARSE_ERROR)),
        TypeClass::Float => field
            .trim()
            .parse::<f64>()
            .map(Value::Real)
            .map_err(|_error| numeric(NUMERIC_PARSE_ERROR)),
        TypeClass::Boolean => boolean(field)
            .map(Value::Integer)
            .ok_or_else(|| rejected(1210, 0)),
        TypeClass::Date => is_date(field)
            .then(|| Value::Text(field.trim().to_string()))
            .ok_or_else(|| rejected(1205, 0)),
        TypeClass::Timestamp => is_timestamp(field)
            .then(|| Value::Text(field.trim().to_string()))
            .ok_or_else(|| rejected(1206, 0)),
        TypeClass::Character => match declared_length(declared) {
            // "TRUNCATECOLUMNS: truncates data in columns to the appropriate
            // number of characters so that it fits the column specification";
            // without it, a value longer than the column takes is the published
            // out-of-range error.
            Some(length) if field.chars().count() > length => {
                if statement.truncate_columns {
                    Ok(Value::Text(field.chars().take(length).collect()))
                } else {
                    Err(rejected(
                        1204,
                        i64::try_from(length).unwrap_or(i64::MAX),
                    ))
                }
            }
            _ => Ok(Value::Text(field.to_string())),
        },
    }
}

/// Whether a field loads as NULL.
///
/// `NULL AS` names the value that does; `EMPTYASNULL` makes an empty field one
/// and `BLANKSASNULL` a field of only white space. A field that is empty loads
/// as NULL for a column that cannot hold an empty value, which is every column
/// that is not a character column.
fn is_null(statement: &CopyStatement, field: &str, class: TypeClass) -> bool {
    if statement
        .null_as
        .as_deref()
        .is_some_and(|marker| marker == field)
    {
        return true;
    }
    if field.is_empty() {
        return statement.empty_as_null || class != TypeClass::Character;
    }
    statement.blanks_as_null && field.trim().is_empty()
}

/// The coarse class of a declared column type, which is what decides how a
/// field is read into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeClass {
    Integer,
    Decimal,
    Float,
    Boolean,
    Date,
    Timestamp,
    Character,
}

impl TypeClass {
    /// The class of the type a column was declared with. A column declared
    /// without a type takes any value, which is the character class.
    ///
    /// The names are Amazon Redshift's own data types and their documented
    /// aliases (*Data types*): `SMALLINT`/`INT2`, `INTEGER`/`INT`/`INT4`,
    /// `BIGINT`/`INT8`, `DECIMAL`/`NUMERIC`, `REAL`/`FLOAT4`,
    /// `DOUBLE PRECISION`/`FLOAT8`/`FLOAT`, `BOOLEAN`/`BOOL`, `CHAR`/`VARCHAR`
    /// and their aliases, `DATE`, `TIME`, `TIMESTAMP`/`TIMESTAMPTZ`.
    fn of(declared: &str) -> Self {
        let name = declared
            .split(['(', ' '])
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match name.as_str() {
            "smallint" | "int2" | "integer" | "int" | "int4" | "bigint" | "int8" => Self::Integer,
            "decimal" | "numeric" => Self::Decimal,
            "real" | "float4" | "double" | "float8" | "float" => Self::Float,
            "boolean" | "bool" => Self::Boolean,
            "date" => Self::Date,
            "timestamp" | "timestamptz" | "timestamptz_or_timestamp" | "datetime" => {
                Self::Timestamp
            }
            _ => Self::Character,
        }
    }
}

/// The declared length a character type carries — `VARCHAR(16)` is sixteen
/// characters — or `None` for a type that declares none.
fn declared_length(declared: &str) -> Option<usize> {
    let (_, rest) = declared.split_once('(')?;
    let (length, _) = rest.split_once(')')?;
    length.trim().parse().ok()
}

/// Where in a numeric field the parse stopped, and how the engine words it.
///
/// A numeric value is read as a MANTISSA and, for a type that admits one, an
/// EXPONENT: the value is split at its exponent marker (`e`/`E`), the exponent
/// read as `[sign] digits`, and the mantissa as `[sign] digits [. digits]` —
/// which is why a value carrying an `e` and then something that is not an
/// exponent is reported as an `Invalid exponent` on that character, and every
/// other value that is not a number as an `Invalid digit` on the first
/// character that does not fit. Each names the character, its position in the
/// field and the type the value was being read for.
fn numeric_diagnostic(field: &str, label: &str, exponents: bool) -> (String, i64) {
    let characters: Vec<char> = field.trim().chars().collect();
    // The exponent is read FIRST, so a value whose exponent is not one is
    // reported on the exponent rather than on whatever the mantissa carries.
    let marker = exponents.then(|| {
        characters
            .iter()
            .position(|character| *character == 'e' || *character == 'E')
            .filter(|position| *position > 0)
    });
    if let Some(Some(marker)) = marker {
        if let Some(stop) = exponent_stop(&characters, marker) {
            return diagnostic("Invalid exponent", &characters, stop, label);
        }
    }
    let mantissa = match marker {
        Some(Some(marker)) => &characters[..marker],
        _ => &characters[..],
    };
    let stop = mantissa_stop(mantissa).unwrap_or(mantissa.len());
    diagnostic("Invalid digit", &characters, stop, label)
}

/// The position the EXPONENT part of a numeric value stops being one at, or
/// `None` when it is a well-formed exponent.
///
/// The part after the marker is `[sign] digits`: a character that is neither,
/// and an exponent carrying no digit at all, stop it.
fn exponent_stop(characters: &[char], marker: usize) -> Option<usize> {
    let mut index = marker + 1;
    if matches!(characters.get(index), Some('+' | '-')) {
        index += 1;
    }
    let first_digit = index;
    while characters
        .get(index)
        .is_some_and(|character| character.is_ascii_digit())
    {
        index += 1;
    }
    if index == first_digit || index < characters.len() {
        return Some(index);
    }
    None
}

/// The position the MANTISSA stops being a number at — `[sign] digits [. digits]`
/// — or `None` when every character of it fits that form.
fn mantissa_stop(characters: &[char]) -> Option<usize> {
    let mut index = 0_usize;
    if matches!(characters.first(), Some('+' | '-')) {
        index += 1;
    }
    let mut seen_point = false;
    while let Some(character) = characters.get(index) {
        if character.is_ascii_digit() {
            index += 1;
            continue;
        }
        if *character == '.' && !seen_point {
            seen_point = true;
            index += 1;
            continue;
        }
        return Some(index);
    }
    None
}

/// One parse diagnostic: the kind, the character the parse stopped on — a space
/// where it stopped at the end of the value — its position, and the type it was
/// reading for.
fn diagnostic(
    kind: &str,
    characters: &[char],
    stop: usize,
    label: &str,
) -> (String, i64) {
    let character = characters.get(stop).copied().unwrap_or(' ');
    let position = i64::try_from(stop).unwrap_or(i64::MAX);
    (
        format!("{kind}, Value '{character}', Pos {position}, Type: {label}"),
        position,
    )
}

/// The type name the engine's numeric diagnostic names: the Amazon Redshift
/// type the value was being read for, as that diagnostic spells it.
fn numeric_label(declared: &str, class: TypeClass) -> &'static str {
    let name = type_head(declared);
    match (class, name.as_str()) {
        (TypeClass::Integer, "smallint" | "int2") => "Short",
        (TypeClass::Integer, "bigint" | "int8") => "Long",
        (TypeClass::Integer, _) => "Integer",
        (TypeClass::Decimal, _) => "Decimal",
        (TypeClass::Float, "real" | "float4") => "Float",
        (TypeClass::Float, _) => "Double",
        _ => "Char",
    }
}

/// The name `STL_LOAD_ERRORS` reports in its `type` column: the Amazon Redshift
/// data type of the field, under the name that service's catalog gives it
/// (*Data types*).
fn type_name(declared: &str, class: TypeClass) -> &'static str {
    let name = type_head(declared);
    match (class, name.as_str()) {
        (TypeClass::Integer, "smallint" | "int2") => "int2",
        (TypeClass::Integer, "bigint" | "int8") => "int8",
        (TypeClass::Integer, _) => "int4",
        (TypeClass::Decimal, _) => "numeric",
        (TypeClass::Float, "real" | "float4") => "float4",
        (TypeClass::Float, _) => "float8",
        (TypeClass::Boolean, _) => "bool",
        (TypeClass::Date, _) => "date",
        (TypeClass::Timestamp, _) => "timestamp",
        (TypeClass::Character, "char" | "character" | "nchar" | "bpchar") => "char",
        (TypeClass::Character, _) => "varchar",
    }
}

/// The leading name of a declared type, lower-cased and without its parameters:
/// `DECIMAL(10,2)` is a `decimal`.
fn type_head(declared: &str) -> String {
    declared
        .split(['(', ' '])
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// The boolean a field carries, as the engine stores one: Amazon Redshift reads
/// `TRUE`/`t`/`true`/`y`/`yes`/`1` and `FALSE`/`f`/`false`/`n`/`no`/`0` for a
/// `BOOLEAN` column.
fn boolean(field: &str) -> Option<i64> {
    let value = field.trim().to_ascii_lowercase();
    match value.as_str() {
        "1" | "t" | "true" | "y" | "yes" => Some(1),
        "0" | "f" | "false" | "n" | "no" => Some(0),
        _ => None,
    }
}

/// Whether a field is a date the engine takes: the `YYYY-MM-DD` form Amazon
/// Redshift documents as the default `DATEFORMAT`.
fn is_date(field: &str) -> bool {
    ::chrono::NaiveDate::parse_from_str(field.trim(), "%Y-%m-%d").is_ok()
}

/// Whether a field is a timestamp the engine takes: the `YYYY-MM-DD
/// HH:MI:SS[.F]` form Amazon Redshift documents as the default `TIMEFORMAT`,
/// with the `T` separator the ISO form carries.
fn is_timestamp(field: &str) -> bool {
    const FORMATS: [&str; 4] = [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
    ];
    let field = field.trim().trim_end_matches('Z');
    FORMATS
        .iter()
        .any(|format| ::chrono::NaiveDateTime::parse_from_str(field, format).is_ok())
}

/// Insert the rows a load read, through the engine's own insert.
fn insert(
    connection: &rusqlite::Connection,
    target: &str,
    columns: &[Column],
    rows: &[Vec<Value>],
) -> rusqlite::Result<i64> {
    if rows.is_empty() {
        return Ok(0);
    }
    let quoted = target.replace('"', "\"\"");
    let names: Vec<String> = columns
        .iter()
        .map(|column| format!("\"{}\"", column.name.replace('"', "\"\"")))
        .collect();
    let placeholders: Vec<String> = (1..=columns.len()).map(|index| format!("?{index}")).collect();
    let mut statement = connection.prepare(&format!(
        "INSERT INTO \"{quoted}\" ({}) VALUES ({})",
        names.join(", "),
        placeholders.join(", ")
    ))?;
    let mut inserted = 0_i64;
    for row in rows {
        let bound: Vec<&dyn rusqlite::ToSql> =
            row.iter().map(|value| value as &dyn rusqlite::ToSql).collect();
        inserted = inserted.saturating_add(
            i64::try_from(statement.execute(bound.as_slice())?).unwrap_or(i64::MAX),
        );
    }
    Ok(inserted)
}
