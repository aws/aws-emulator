// SPDX-License-Identifier: CC0-1.0

//! Converting a buffer's records from JSON to the columnar format an extended Amazon S3
//! destination asks for, against the column information of an Amazon Web Services Glue
//! table.
//!
//! "Specifies that you want Firehose to convert data from the JSON format to the Parquet or
//! ORC format before writing it to Amazon S3. Firehose uses the serializer and deserializer
//! that you specify, in addition to the column information from the Amazon Web Services
//! Glue table, to deserialize your input data from JSON and then serialize it to the
//! Parquet or ORC format" (`DataFormatConversionConfiguration`). So a destination with
//! conversion in force writes, instead of the records' own bytes concatenated, ONE columnar
//! file per delivered object whose columns are the bound Glue table's — and the records it
//! could not deserialize into those columns are failed records, which go to the
//! destination's `ErrorOutputPrefix` ("a prefix that Firehose evaluates and adds to failed
//! records before writing them to S3") while their well-formed siblings in the same buffer
//! are still delivered.
//!
//! WHERE THE SCHEMA COMES FROM. `SchemaConfiguration` names a Glue database and table
//! ("specifies the Amazon Web Services Glue table that contains the column information that
//! constitutes your data schema"), and the one Data Catalog in this emulator is the Glue
//! service's own store: the columns are read out of it at the moment of the flush, so a
//! table whose columns change is picked up by the next delivery ("if you set it to `LATEST`,
//! Firehose uses the most recent version … any updates to the table are automatically
//! picked up" — `SchemaConfiguration$VersionId`). The catalog is read and the values are
//! cloned out of it before anything is written, so the two services' stores are never
//! locked at once.
//!
//! WHAT IS WRITTEN. The Parquet file is written by the Apache implementation of the format
//! ([`parquet`]) through its row-group writer, honouring the serializer's own parameters
//! (`ParquetSerDe`: `Compression`, `WriterVersion`, `PageSizeBytes`,
//! `EnableDictionaryCompression`), so what lands in the bucket is a Parquet file any
//! Parquet reader reads — not bytes assembled here to look like one.
//!
//! WHAT A COLUMN MAY BE. A Glue column's `Type` is a Hive data type, which nests: the tree an
//! `array<…>`, a `struct<…>` or a `map<…,…>` declares is written as the groups the Parquet
//! format's own LIST and MAP annotations describe, with the definition and repetition level of
//! every value — [`nested`].
//!
//! WHY A RECORD FAILS. Each record is read against those columns and either converts or does
//! not, and the two ways it can fail are not the same failure: bytes that are not one JSON
//! object cannot be READ at all ([`Rejection::Unparseable`]), while a record that was read but
//! carries a value the column at that point of the tree does not hold is MALFORMED against the
//! schema ([`Rejection::Malformed`]). The conversion reports which, so the error output names a
//! conversion error code per record and a consumer of the destination's `ErrorOutputPrefix`
//! can tell a schema mismatch from a JSON parse failure.
//!
//! WHAT IS NOT. A serializer or a column type this module cannot write — `OrcSerDe`, for
//! which no maintained Rust writer is available here, and the Hive types outside
//! [`HiveType::of_declared`] — leaves the buffer UNDELIVERED and owed again rather than
//! written in a format the destination did not ask for or reported as a record failure that
//! the real service would have converted: a record this emulator cannot place is never
//! dropped and never mislabelled ([`Converted::Unwritable`]).

mod nested;

use std::collections::BTreeMap;

use ::parquet::basic::{Compression, ConvertedType, GzipLevel, Type as PhysicalType};
use ::parquet::file::properties::{WriterProperties, WriterVersion};
use ::parquet::file::writer::SerializedFileWriter;

use crate::ops::types::{DataFormatConversionConfiguration, Deserializer, ParquetSerDe};
use crate::state::StoredRecord;

use self::nested::{HiveType, Value};

/// `ParquetSerDe$Compression` when the serializer names none: "The possible values are
/// `UNCOMPRESSED`, `SNAPPY`, and `GZIP`, with the default being `SNAPPY`."
const DEFAULT_PARQUET_COMPRESSION: &str = "SNAPPY";

/// `ParquetSerDe$WriterVersion` when the serializer names none: "The possible values are
/// `V1` and `V2`. The default is `V1`."
const DEFAULT_PARQUET_WRITER_VERSION: &str = "V1";

/// The `ParquetSerDe$WriterVersion` value naming the second row format.
const PARQUET_WRITER_VERSION_V2: &str = "V2";

/// The `SchemaConfiguration$VersionId` a binding that names none is read at: "If you don't
/// specify this version ID, or if you set it to `LATEST`, Firehose uses the most recent
/// version."
const LATEST_TABLE_VERSION: &str = "LATEST";

/// The special `HiveJsonSerDe$TimestampFormats` value: "You can also use the special value
/// `millis` to parse timestamps in epoch milliseconds."
const EPOCH_MILLIS_FORMAT: &str = "millis";

/// How a timestamp string is read when no `TimestampFormats` asks for epoch milliseconds:
/// "If you don't specify a format, Firehose uses `java.sql.Timestamp::valueOf` by default",
/// whose form is `yyyy-mm-dd hh:mm:ss` with an optional fractional second.
const TIMESTAMP_VALUE_OF_FORMATS: &[&str] = &["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"];

/// How a date string is read: the ISO calendar date, which is the form Hive writes a `date`
/// value in JSON as.
const DATE_FORMAT: &str = "%Y-%m-%d";

/// Seconds in one day, which turns the instant a date starts at into the whole days since
/// the epoch a Parquet `DATE` leaf carries.
const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

/// How many columns of a bound table one conversion writes.
///
/// A CHOSEN bound, and the one thing about this work a producer does not decide: the records
/// of a flush are bounded by the stream's own buffer budget, but the COLUMNS come from the
/// Data Catalog, so this is what keeps the leaves one conversion writes bounded whatever a
/// table declares. Far above the columns a Firehose stream's records carry, so a schema a
/// client would really bind is never refused by it.
const MAX_CONVERTED_COLUMNS: usize = 1_000;

/// How many rows one row group of a converted object holds.
///
/// A CHOSEN bound, beside [`nested::MAX_CELLS_PER_ROW_GROUP`]: the rows of a chunk are
/// deserialized, written as one row group and dropped before the next chunk is read, so the
/// values in memory at once are one chunk's and not the whole flush's. A Parquet file is a
/// sequence of row groups, so writing a long buffer as several of them is what the format is
/// for.
const MAX_ROWS_PER_ROW_GROUP: usize = 500;

/// The largest `ParquetSerDe$PageSizeBytes` this writer is given.
///
/// A CHOSEN bound, well above the member's own documented default of 1 MiB: the page size
/// decides how much a column chunk buffers before it writes a page, so clamping it is what
/// keeps a configured value from deciding how much one conversion holds.
const MAX_PARQUET_PAGE_SIZE: usize = 8 * 1024 * 1024;

/// The smallest `ParquetSerDe$PageSizeBytes` this writer is given: the member's own
/// published minimum, "The minimum value is 64 KiB".
const MIN_PARQUET_PAGE_SIZE: usize = 64 * 1024;

/// The conversion a destination's `DataFormatConversionConfiguration` asks for, read off
/// that configuration as it stands at the moment of the flush.
#[derive(Debug, Clone)]
pub struct FormatConversion {
    /// The Glue table the column information comes from.
    schema: SchemaBinding,
    /// How a record's JSON is read into those columns — the deserializer
    /// `InputFormatConfiguration` names.
    deserializer: JsonDeserializer,
    /// The format the columns are serialized to — the serializer
    /// `OutputFormatConfiguration` names.
    output: OutputFormat,
}

/// The Glue table a `SchemaConfiguration` binds the conversion to.
#[derive(Debug, Clone, Default)]
struct SchemaBinding {
    /// `RoleARN`, "the role that Firehose can use to access Amazon Web Services Glue".
    role_arn: Option<String>,
    /// `CatalogId`, or `None` for the documented default: "If you don't supply this, the
    /// Amazon Web Services account ID is used by default."
    catalog_id: Option<String>,
    /// `DatabaseName`, empty when the configuration names none.
    database_name: String,
    /// `TableName`, empty when the configuration names none.
    table_name: String,
    /// `Region`, or `None` for the documented default: "If you don't specify an Amazon Web
    /// Services Region, the default is the current Region."
    region: Option<String>,
    /// `VersionId`, the table version the output data schema is read at.
    version_id: Option<String>,
}

/// How a record's JSON object is matched to the table's columns.
#[derive(Debug, Clone)]
struct JsonDeserializer {
    /// `OpenXJsonSerDe$CaseInsensitive`: "When set to `true`, which is the default, Firehose
    /// converts JSON keys to lowercase before deserializing them." The native Hive
    /// deserializer carries no such parameter, so its keys are matched as written.
    case_insensitive: bool,
    /// `OpenXJsonSerDe$ConvertDotsInJsonKeysToUnderscores`: "specifies that the names of the
    /// keys include dots and that you want Firehose to replace them with underscores … The
    /// default is `false`."
    dots_to_underscores: bool,
    /// `OpenXJsonSerDe$ColumnToJsonKeyMappings`: "Maps column names to JSON keys that aren't
    /// identical to the column names", keyed by COLUMN name — the model's example maps the
    /// column `ts` to the JSON key `timestamp`.
    column_to_json_key: BTreeMap<String, String>,
    /// `HiveJsonSerDe$TimestampFormats`, of which the special `millis` value is the one this
    /// module reads; the default form is [`TIMESTAMP_VALUE_OF_FORMATS`].
    epoch_millis_timestamps: bool,
}

impl Default for JsonDeserializer {
    fn default() -> Self {
        Self {
            case_insensitive: true,
            dots_to_underscores: false,
            column_to_json_key: BTreeMap::new(),
            epoch_millis_timestamps: false,
        }
    }
}

/// The columnar format the records are serialized to.
#[derive(Debug, Clone)]
enum OutputFormat {
    /// `ParquetSerDe`, with the writer parameters it carries.
    Parquet(ParquetSerDe),
    /// `OrcSerDe` — a format this emulator has no writer for.
    Orc,
}

/// Why one record of a buffer did not convert: the CLASS of the failure, which is the
/// conversion error code its error-output document names, and what failed within that class.
///
/// The two classes are what makes a record the conversion could not read at all distinguishable
/// from one whose values the bound columns do not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// The record's bytes are not one JSON object, so there was nothing to read into the
    /// columns.
    Unparseable(String),
    /// The record was read, but a value it carries is not one the column it stands in holds.
    Malformed(String),
}

impl Rejection {
    /// What failed, as the record's own error-output document states it.
    #[must_use]
    pub fn fault(&self) -> &str {
        match self {
            Self::Unparseable(fault) | Self::Malformed(fault) => fault,
        }
    }
}

/// What one group's records converted to.
pub enum Converted<'r> {
    /// The columnar file holding every record that converted — EMPTY when none did, which
    /// is an object the delivery does not write — beside the records that did not, each with
    /// WHY it did not: the class of the failure and the column and value that could not be
    /// deserialized, which is what the error output reports for that record rather than one
    /// wording for every failure.
    Rows {
        body: Vec<u8>,
        failed: Vec<(&'r StoredRecord, Rejection)>,
    },
    /// The bound schema could not be read at all: the catalog holds no such table, or the
    /// table carries no column information. Every record of the group is then a failed
    /// record, exactly as one whose own JSON could not be read is.
    NoSchema,
    /// This emulator cannot write what the configuration asks for, so the delivery is owed
    /// again: nothing is written and no record is reported as failed.
    Unwritable,
}

impl FormatConversion {
    /// The conversion a destination configuration asks for, or `None` where none is in
    /// force.
    ///
    /// "Defaults to `true`. Set it to `false` if you want to disable format conversion while
    /// preserving the configuration details" (`DataFormatConversionConfiguration$Enabled`),
    /// so a configuration that carries the member at all converts unless it says `false` —
    /// and one that names no serializer at all is not a conversion this reads as configured.
    #[must_use]
    pub fn of(config: Option<&DataFormatConversionConfiguration>) -> Option<Self> {
        let config = config?;
        if config.enabled == Some(false) {
            return None;
        }
        let serializer = config
            .output_format_configuration
            .as_ref()
            .and_then(|output| output.serializer.as_ref())?;
        let output = match (&serializer.parquet_ser_de, &serializer.orc_ser_de) {
            (Some(parquet), _) => OutputFormat::Parquet(parquet.clone()),
            (None, Some(_)) => OutputFormat::Orc,
            (None, None) => return None,
        };
        let schema = config
            .schema_configuration
            .as_ref()
            .map(|schema| SchemaBinding {
                role_arn: schema.role_arn.clone().filter(|role| !role.is_empty()),
                catalog_id: schema
                    .catalog_id
                    .clone()
                    .filter(|catalog| !catalog.is_empty()),
                database_name: schema.database_name.clone().unwrap_or_default(),
                table_name: schema.table_name.clone().unwrap_or_default(),
                region: schema.region.clone().filter(|region| !region.is_empty()),
                version_id: schema
                    .version_id
                    .clone()
                    .filter(|version| !version.is_empty()),
            })
            .unwrap_or_default();
        let deserializer = config
            .input_format_configuration
            .as_ref()
            .and_then(|input| input.deserializer.as_ref())
            .map(JsonDeserializer::of)
            .unwrap_or_default();
        Some(Self {
            schema,
            deserializer,
            output,
        })
    }

    /// The Data Catalog table this conversion reads its columns from, as the error output
    /// names it: every member of the binding's `SchemaConfiguration`, and `null` for one the
    /// configuration names no value for — except the table version, which is the one the
    /// member documents as its default ("If you don't specify this version ID, or if you set
    /// it to `LATEST`, Firehose uses the most recent version" —
    /// `SchemaConfiguration$VersionId`), because that IS the version the columns were read
    /// at.
    ///
    /// A record the conversion failed on is written under the destination's
    /// `ErrorOutputPrefix` with this beside the failure, so a consumer of that prefix learns
    /// WHICH schema the record did not match and not only that one did not.
    #[must_use]
    pub fn data_catalog_table(&self) -> ::serde_json::Value {
        ::serde_json::json!({
            "catalogId": self.schema.catalog_id,
            "databaseName": self.schema.database_name,
            "tableName": self.schema.table_name,
            "region": self.schema.region,
            "versionId": self
                .schema
                .version_id
                .clone()
                .unwrap_or_else(|| LATEST_TABLE_VERSION.to_string()),
            "roleArn": self.schema.role_arn,
        })
    }
}

impl JsonDeserializer {
    /// How the deserializer `InputFormatConfiguration` names reads a record's keys.
    ///
    /// Of the two the model declares, the OpenX one carries the key parameters and the
    /// native Hive one carries the timestamp formats; a `Deserializer` naming neither is
    /// read with the defaults above, which are the ones the members themselves document.
    fn of(deserializer: &Deserializer) -> Self {
        if let Some(open_x) = deserializer.open_x_json_ser_de.as_ref() {
            return Self {
                case_insensitive: open_x.case_insensitive != Some(false),
                dots_to_underscores: open_x.convert_dots_in_json_keys_to_underscores
                    == Some(true),
                column_to_json_key: open_x
                    .column_to_json_key_mappings
                    .iter()
                    .flatten()
                    .map(|(column, key)| (column.clone(), key.clone()))
                    .collect(),
                ..Self::default()
            };
        }
        if let Some(hive) = deserializer.hive_json_ser_de.as_ref() {
            return Self {
                // The native Hive deserializer declares neither the case fold nor the dot
                // rewriting, so its keys are matched exactly as the record wrote them.
                case_insensitive: false,
                epoch_millis_timestamps: hive
                    .timestamp_formats
                    .iter()
                    .flatten()
                    .any(|format| format == EPOCH_MILLIS_FORMAT),
                ..Self::default()
            };
        }
        Self::default()
    }
}

/// Convert `records` — each with the bytes the destination's data processing left of it —
/// into the one columnar object the destination's configuration asks for.
///
/// The Glue table's column information is read out of the Glue service's own store through
/// `engine`, the one shared engine every mounted service reaches its state through, and
/// cloned out of it before any conversion runs, so that store is not held while this works.
///
/// ASYNC because deserializing and serializing a buffer's records is CPU-bound: the loop
/// hands the runtime back between records, between the leaves of a row group and between row
/// groups, so a timeout or a cancellation can land while a long buffer is being converted
/// instead of the whole run holding the worker.
///
/// WHAT THIS HOLDS is bounded by [`nested::MAX_CELLS_PER_ROW_GROUP`] values, whatever a
/// producer puts and whatever the bound table declares: the rows of one chunk are
/// deserialized, written as one row group and dropped before the next chunk is read, and what
/// that chunk may read those rows into is charged down as the records are read — so neither
/// the records of the flush, nor the columns of the table, nor the entries a record carries in
/// a list or a map times the fields a struct declares, decide how many values are in memory at
/// once.
pub async fn convert<'r, Eng: Send + Sync + 'static>(
    engine: &Eng,
    conversion: &FormatConversion,
    records: &[(&'r StoredRecord, Vec<u8>)],
) -> Converted<'r> {
    let OutputFormat::Parquet(parquet) = &conversion.output else {
        ::tracing::warn!(
            "the destination asks for a data format this emulator cannot serialize, so this \
             delivery was left for a later one"
        );
        return Converted::Unwritable;
    };
    let Some(columns) = table_columns(engine, &conversion.schema) else {
        return Converted::NoSchema;
    };
    if columns.len() > MAX_CONVERTED_COLUMNS {
        ::tracing::warn!(
            columns = columns.len(),
            "the bound Glue table declares more columns than one conversion writes here, so \
             this delivery was left for a later one"
        );
        return Converted::Unwritable;
    }
    let mut typed = Vec::with_capacity(columns.len());
    for (name, declared) in &columns {
        match HiveType::of_declared(declared) {
            Some(column_type) => typed.push((name.clone(), column_type)),
            // A Hive type this module has no leaf for is not a record failure: the real
            // service converts such a column, so reporting one here would mislabel every
            // record of the buffer.
            None => {
                ::tracing::warn!(
                    column = %name,
                    "the bound Glue table declares a column type this emulator cannot \
                     convert against, so this delivery was left for a later one"
                );
                return Converted::Unwritable;
            }
        }
    }
    // A column that nests holds more than one leaf, so what the columns are WRITTEN as is what
    // the conversion is bounded by beside the columns themselves.
    let leaves = nested::leaf_count(&typed);
    if leaves > nested::MAX_CONVERTED_LEAVES {
        ::tracing::warn!(
            leaves,
            "the bound Glue table's columns hold more Parquet leaves than one conversion writes \
             here, so this delivery was left for a later one"
        );
        return Converted::Unwritable;
    }
    let mut writer = match parquet_writer(&conversion.schema.table_name, &typed, parquet) {
        Ok(writer) => writer,
        // The writer refused the file: nothing of this buffer is written and the whole of
        // it is owed again, like any other delivery this emulator could not work out.
        Err(error) => {
            ::tracing::warn!(%error, "the Parquet writer refused this delivery's object");
            return Converted::Unwritable;
        }
    };
    let mut failed = Vec::new();
    let mut written = 0usize;
    for chunk in records.chunks(MAX_ROWS_PER_ROW_GROUP) {
        // One value per leaf per record of THIS chunk, so a leaf is written as one batch of
        // values with the definition and repetition level of each row — which is how an
        // OPTIONAL Parquet leaf carries a null, a Glue column holds one wherever the record
        // named no value, and a repeated group carries several entries. What the chunk may
        // read those values into is charged down as the records are read, so neither the
        // entries a record carries nor the fields the table declares decide how much this
        // holds.
        let mut budget = nested::CellBudget::of_row_group();
        let mut rows: Vec<Vec<Value>> = Vec::with_capacity(chunk.len());
        for (record, data) in chunk {
            ::tokio::task::yield_now().await;
            match nested::row_of(data, &typed, &conversion.deserializer, &mut budget) {
                Ok(row) => rows.push(row),
                Err(nested::RowFault::Rejected(rejection)) => {
                    // The record id and nothing of the record's own data: WHAT failed,
                    // which names the column and the value, is carried to the error output
                    // where the producer already holds its own bytes, not to this process's
                    // log.
                    ::tracing::debug!(
                        record_id = %record.record_id,
                        "a record could not be converted to the destination's data format"
                    );
                    failed.push((*record, rejection));
                }
                // The records of this chunk hold more values than one row group reads here:
                // this emulator's bound and not a verdict on the record, so nothing is
                // written and the whole delivery is owed again rather than a record failure
                // the real service would have converted being reported.
                Err(nested::RowFault::TooManyCells) => {
                    ::tracing::warn!(
                        cells = nested::MAX_CELLS_PER_ROW_GROUP,
                        "this buffer's records hold more values than one row group reads \
                         here, so this delivery was left for a later one"
                    );
                    return Converted::Unwritable;
                }
            }
        }
        if rows.is_empty() {
            continue;
        }
        written += rows.len();
        if let Err(error) = nested::write_row_group(&mut writer, &typed, &rows).await {
            ::tracing::warn!(%error, "the Parquet writer refused this delivery's row group");
            return Converted::Unwritable;
        }
        ::tokio::task::yield_now().await;
    }
    // The footer the schema is read back from, which is what makes the bytes a complete
    // Parquet file; a file no record converted into is not an object the delivery writes.
    let body = match writer.into_inner() {
        Ok(buffer) => buffer,
        Err(error) => {
            ::tracing::warn!(%error, "the Parquet writer refused this delivery's object");
            return Converted::Unwritable;
        }
    };
    Converted::Rows {
        body: if written == 0 { Vec::new() } else { body },
        failed,
    }
}

/// The bound table's column information, as `(name, declared Hive type)` in table order.
///
/// The columns come from the table version the binding PINS: "Specifies the table version
/// for the output data schema. If you don't specify this version ID, or if you set it to
/// `LATEST`, Firehose uses the most recent version" (`SchemaConfiguration$VersionId`), so an
/// unpinned binding reads the live definition and a pinned one reads the definition that was
/// current under that version.
///
/// `None` when the catalog holds no such table, holds no such version of it, or the version
/// read carries no columns — the schema the conversion needs cannot be read, which is a
/// failure of every record of the buffer.
fn table_columns<Eng: Send + Sync + 'static>(
    engine: &Eng,
    schema: &SchemaBinding,
) -> Option<Vec<(String, String)>> {
    let catalog_id = ::service_com_amazonaws_glue_awsglue::state::resolve_catalog_id(
        schema.catalog_id.as_ref(),
    );
    let state = ::service_com_amazonaws_glue_awsglue::state::service_state(engine)?;
    let store = state.lock();
    let table = store
        .database(&catalog_id, &schema.database_name)?
        .tables
        .get(&::service_com_amazonaws_glue_awsglue::state::fold_name(
            &schema.table_name,
        ))?;
    let pinned = schema
        .version_id
        .as_deref()
        .filter(|version| !version.eq_ignore_ascii_case(LATEST_TABLE_VERSION));
    let descriptor = match pinned {
        // The most recent version, which is the table's live definition.
        None => table.storage_descriptor.as_ref(),
        // The version the binding pins: the live definition where that IS the version it
        // names, else the one the catalog archived under it.
        Some(version) if version == table.version_id => table.storage_descriptor.as_ref(),
        Some(version) => table
            .archived_versions
            .get(&version.parse::<u64>().ok()?)?
            .definition
            .storage_descriptor
            .as_ref(),
    };
    let columns: Vec<(String, String)> = descriptor
        .and_then(|descriptor| descriptor.columns.as_ref())
        .into_iter()
        .flatten()
        .map(|column| {
            (
                column.name.clone(),
                column.r#type.clone().unwrap_or_default(),
            )
        })
        .collect();
    (!columns.is_empty()).then_some(columns)
}

/// The Parquet leaf one Glue column is written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnType {
    Boolean,
    /// `tinyint`, `smallint` and `int`: a 32-bit leaf, with the width its Hive type admits.
    Integer(IntegerWidth),
    BigInt,
    Float,
    Double,
    /// `string`, `char(n)` and `varchar(n)`.
    Text,
    /// `binary`, carried in JSON as base64 like every other blob on an AWS wire.
    Binary,
    /// `date`: whole days since the epoch.
    Date,
    /// `timestamp`: milliseconds since the epoch.
    Timestamp,
}

/// The width of an integer Hive type, which decides both the logical type of the leaf and
/// the values the column admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntegerWidth {
    /// `tinyint`: an 8-bit signed integer.
    Tiny,
    /// `smallint`: a 16-bit signed integer.
    Small,
    /// `int`: a 32-bit signed integer.
    Int,
}

impl IntegerWidth {
    /// The range this width admits, inclusive.
    fn range(self) -> (i64, i64) {
        match self {
            Self::Tiny => (i64::from(i8::MIN), i64::from(i8::MAX)),
            Self::Small => (i64::from(i16::MIN), i64::from(i16::MAX)),
            Self::Int => (i64::from(i32::MIN), i64::from(i32::MAX)),
        }
    }

    /// The logical (converted) type that says how the 32-bit leaf's values are read.
    fn converted(self) -> ConvertedType {
        match self {
            Self::Tiny => ConvertedType::INT_8,
            Self::Small => ConvertedType::INT_16,
            Self::Int => ConvertedType::NONE,
        }
    }
}

impl ColumnType {
    /// The leaf a column whose Glue `Type` is `declared` is written as, or `None` for a Hive
    /// type this module has no leaf for.
    ///
    /// The names are Hive's own data types, which is what a Glue column's `Type` carries
    /// (`Column$Type`: "the data type of the Column"): the parameters of a `char(n)` /
    /// `varchar(n)` do not change the leaf, so they are dropped, and the case is folded
    /// because Hive type names are case-insensitive.
    fn of_declared(declared: &str) -> Option<Self> {
        let declared = declared.trim().to_ascii_lowercase();
        let bare = declared
            .split_once('(')
            .map_or(declared.as_str(), |(name, _)| name)
            .trim();
        match bare {
            "boolean" => Some(Self::Boolean),
            "tinyint" | "byte" => Some(Self::Integer(IntegerWidth::Tiny)),
            "smallint" | "short" => Some(Self::Integer(IntegerWidth::Small)),
            "int" | "integer" => Some(Self::Integer(IntegerWidth::Int)),
            "bigint" | "long" => Some(Self::BigInt),
            "float" => Some(Self::Float),
            "double" => Some(Self::Double),
            "string" | "char" | "varchar" => Some(Self::Text),
            "binary" => Some(Self::Binary),
            "date" => Some(Self::Date),
            "timestamp" => Some(Self::Timestamp),
            _ => None,
        }
    }

    /// This column's Parquet physical type and the logical (converted) type that says how
    /// its values are read.
    fn physical(self) -> (PhysicalType, ConvertedType) {
        match self {
            Self::Boolean => (PhysicalType::BOOLEAN, ConvertedType::NONE),
            Self::Integer(width) => (PhysicalType::INT32, width.converted()),
            Self::BigInt => (PhysicalType::INT64, ConvertedType::NONE),
            Self::Float => (PhysicalType::FLOAT, ConvertedType::NONE),
            Self::Double => (PhysicalType::DOUBLE, ConvertedType::NONE),
            Self::Text => (PhysicalType::BYTE_ARRAY, ConvertedType::UTF8),
            Self::Binary => (PhysicalType::BYTE_ARRAY, ConvertedType::NONE),
            Self::Date => (PhysicalType::INT32, ConvertedType::DATE),
            Self::Timestamp => (PhysicalType::INT64, ConvertedType::TIMESTAMP_MILLIS),
        }
    }
}

/// One value a record carries for one SCALAR of a column, read as that scalar's own type.
///
/// A record that named no value where a scalar stands carries none at all ([`Value::Null`]),
/// which is what an OPTIONAL Parquet leaf records as a null rather than a cell of its own: "a
/// key that is missing from the record" simply has no value in the converted row.
#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Boolean(bool),
    Int32(i32),
    Int64(i64),
    Float(f32),
    Double(f64),
    Text(String),
    Bytes(Vec<u8>),
}

/// A JSON key or a column name as the deserializer matches it: case-folded where
/// `CaseInsensitive` is in force, and with dots rewritten where
/// `ConvertDotsInJsonKeysToUnderscores` is.
fn matched_key(key: &str, deserializer: &JsonDeserializer) -> String {
    let folded = if deserializer.case_insensitive {
        key.to_lowercase()
    } else {
        key.to_string()
    };
    if deserializer.dots_to_underscores {
        folded.replace('.', "_")
    } else {
        folded
    }
}

/// One JSON value as the cell of a column of type `column_type`.
///
/// `Err` carries why the value is not one that column holds — which is what makes a
/// schema-mismatched record a failed record rather than a row of nulls.
fn cell_of(
    value: &::serde_json::Value,
    column_type: ColumnType,
    deserializer: &JsonDeserializer,
) -> Result<Cell, String> {
    match column_type {
        ColumnType::Boolean => match value {
            ::serde_json::Value::Bool(flag) => Ok(Cell::Boolean(*flag)),
            // A quoted boolean is still a boolean to a JSON deserializer reading into a
            // typed column, which is the one widening both deserializers do.
            ::serde_json::Value::String(text) => match text.to_ascii_lowercase().as_str() {
                "true" => Ok(Cell::Boolean(true)),
                "false" => Ok(Cell::Boolean(false)),
                _ => Err(format!("{text:?} is not a boolean")),
            },
            other => Err(format!("{} is not a boolean", kind_of(other))),
        },
        ColumnType::Integer(width) => {
            let integer = integer_of(value)?;
            let (low, high) = width.range();
            if integer < low || integer > high {
                return Err(format!("{integer} is outside the column's range"));
            }
            Ok(Cell::Int32(
                i32::try_from(integer).map_err(|error| error.to_string())?,
            ))
        }
        ColumnType::BigInt => Ok(Cell::Int64(integer_of(value)?)),
        ColumnType::Float => {
            let number = double_of(value)?;
            // A `float` leaf is 32-bit, so a value whose MAGNITUDE it cannot carry is not a
            // value this column holds — reported like an integer outside its width rather
            // than written as the infinity the narrowing would saturate to. A value it
            // carries with less precision is a value it holds, which is what a 32-bit
            // column means.
            if number.is_finite() && number.abs() > f64::from(f32::MAX) {
                return Err(format!("{number} is outside the column's range"));
            }
            if !number.is_finite() {
                return Err("the value is not a finite number".to_string());
            }
            Ok(Cell::Float(number as f32))
        }
        ColumnType::Double => Ok(Cell::Double(double_of(value)?)),
        ColumnType::Text => match value {
            ::serde_json::Value::String(text) => Ok(Cell::Text(text.clone())),
            // A scalar of another JSON type is carried as the text it is written as; an
            // array or an object is not a value a string column holds.
            ::serde_json::Value::Number(number) => Ok(Cell::Text(number.to_string())),
            ::serde_json::Value::Bool(flag) => Ok(Cell::Text(flag.to_string())),
            other => Err(format!("{} is not a string", kind_of(other))),
        },
        ColumnType::Binary => match value {
            ::serde_json::Value::String(text) => ::aws_smithy_types::base64::decode(text)
                .map(Cell::Bytes)
                .map_err(|error| format!("the value is not base64: {error}")),
            other => Err(format!("{} is not a binary value", kind_of(other))),
        },
        ColumnType::Date => date_of(value),
        ColumnType::Timestamp => timestamp_of(value, deserializer),
    }
}

/// The integer a JSON value holds, for a column whose type is an integer one.
///
/// A number with a fractional part is NOT an integer: the column cannot hold it, so the
/// record carrying it is a failed record rather than a row with the value rounded into it.
fn integer_of(value: &::serde_json::Value) -> Result<i64, String> {
    match value {
        ::serde_json::Value::Number(number) => number
            .as_i64()
            .ok_or_else(|| format!("{number} is not an integer")),
        ::serde_json::Value::String(text) => text
            .trim()
            .parse::<i64>()
            .map_err(|error| format!("{text:?} is not an integer: {error}")),
        other => Err(format!("{} is not an integer", kind_of(other))),
    }
}

/// The double a JSON value holds, for a column whose type is a floating-point one.
fn double_of(value: &::serde_json::Value) -> Result<f64, String> {
    match value {
        ::serde_json::Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| format!("{number} is not a number this column holds")),
        ::serde_json::Value::String(text) => text
            .trim()
            .parse::<f64>()
            .map_err(|error| format!("{text:?} is not a number: {error}")),
        other => Err(format!("{} is not a number", kind_of(other))),
    }
}

/// A JSON value as a `date` cell: whole days since the epoch, which is how a Parquet `DATE`
/// leaf carries one.
fn date_of(value: &::serde_json::Value) -> Result<Cell, String> {
    match value {
        ::serde_json::Value::String(text) => {
            let date = ::chrono::NaiveDate::parse_from_str(text.trim(), DATE_FORMAT)
                .map_err(|error| format!("{text:?} is not a date: {error}"))?;
            // Whole days since the epoch, counted from the instant the date starts at in
            // UTC: a date before the epoch is a negative count, which is why the division
            // floors rather than truncating.
            let days = date
                .and_time(::chrono::NaiveTime::MIN)
                .and_utc()
                .timestamp()
                .div_euclid(SECONDS_PER_DAY);
            Ok(Cell::Int32(
                i32::try_from(days).map_err(|error| error.to_string())?,
            ))
        }
        // A number of whole days since the epoch is the value the leaf itself carries.
        ::serde_json::Value::Number(_) => {
            let days = integer_of(value)?;
            Ok(Cell::Int32(
                i32::try_from(days).map_err(|error| error.to_string())?,
            ))
        }
        other => Err(format!("{} is not a date", kind_of(other))),
    }
}

/// A JSON value as a `timestamp` cell: milliseconds since the epoch, which is how a Parquet
/// `TIMESTAMP_MILLIS` leaf carries one.
///
/// A JSON NUMBER is epoch milliseconds — the reading `HiveJsonSerDe$TimestampFormats` names
/// `millis` for — and a STRING is read as that same count where the deserializer asked for
/// it, else as the default `java.sql.Timestamp::valueOf` form or the ISO-8601 instant a JSON
/// producer writes.
fn timestamp_of(
    value: &::serde_json::Value,
    deserializer: &JsonDeserializer,
) -> Result<Cell, String> {
    match value {
        ::serde_json::Value::Number(_) => Ok(Cell::Int64(integer_of(value)?)),
        ::serde_json::Value::String(text) => {
            let text = text.trim();
            if deserializer.epoch_millis_timestamps {
                if let Ok(millis) = text.parse::<i64>() {
                    return Ok(Cell::Int64(millis));
                }
            }
            for format in TIMESTAMP_VALUE_OF_FORMATS {
                if let Ok(stamp) = ::chrono::NaiveDateTime::parse_from_str(text, format) {
                    return Ok(Cell::Int64(stamp.and_utc().timestamp_millis()));
                }
            }
            ::chrono::DateTime::parse_from_rfc3339(text)
                .map(|stamp| Cell::Int64(stamp.timestamp_millis()))
                .map_err(|error| format!("{text:?} is not a timestamp: {error}"))
        }
        other => Err(format!("{} is not a timestamp", kind_of(other))),
    }
}

/// What KIND of JSON value this is, for the part of a fault that is about the value's TYPE
/// rather than its content: a value of a kind no reading of the column could accept is
/// reported by its kind, because the kind is the whole reason it was refused.
fn kind_of(value: &::serde_json::Value) -> &'static str {
    match value {
        ::serde_json::Value::Null => "a null",
        ::serde_json::Value::Bool(_) => "a boolean",
        ::serde_json::Value::Number(_) => "a number",
        ::serde_json::Value::String(_) => "a string",
        ::serde_json::Value::Array(_) => "an array",
        ::serde_json::Value::Object(_) => "an object",
    }
}

/// The writer of the Parquet file the conversion fills: the message type the bound table's
/// columns are written as ([`nested::message_type`]), with the parameters `ParquetSerDe`
/// carries, writing into a buffer in memory.
fn parquet_writer(
    table: &str,
    columns: &[(String, HiveType)],
    serializer: &ParquetSerDe,
) -> Result<SerializedFileWriter<Vec<u8>>, String> {
    let schema = nested::message_type(table, columns)?;
    SerializedFileWriter::new(
        Vec::new(),
        ::std::sync::Arc::new(schema),
        ::std::sync::Arc::new(writer_properties(serializer)?),
    )
    .map_err(|error| error.to_string())
}

/// The writer parameters `ParquetSerDe` asks for: the compression over the data blocks, the
/// row format version, the page size the column chunks are divided into, and whether
/// dictionary compression is enabled.
fn writer_properties(serializer: &ParquetSerDe) -> Result<WriterProperties, String> {
    let compression = match serializer
        .compression
        .as_deref()
        .unwrap_or(DEFAULT_PARQUET_COMPRESSION)
    {
        "SNAPPY" => Compression::SNAPPY,
        "GZIP" => Compression::GZIP(GzipLevel::default()),
        "UNCOMPRESSED" => Compression::UNCOMPRESSED,
        other => return Err(format!("the serializer asks for the compression {other}")),
    };
    let version = match serializer
        .writer_version
        .as_deref()
        .unwrap_or(DEFAULT_PARQUET_WRITER_VERSION)
    {
        PARQUET_WRITER_VERSION_V2 => WriterVersion::PARQUET_2_0,
        _ => WriterVersion::PARQUET_1_0,
    };
    let mut properties = WriterProperties::builder()
        .set_compression(compression)
        .set_writer_version(version);
    if let Some(page_size) = serializer
        .page_size_bytes
        .and_then(|size| usize::try_from(size).ok())
    {
        // Clamped before it reaches the writer: the page size is what a column chunk
        // buffers before it flushes a page, so this is what keeps a configured one from
        // deciding how much this holds, at either end — "The minimum value is 64 KiB"
        // (`ParquetSerDe$PageSizeBytes`), below which the writer would emit a page header
        // per handful of values.
        properties = properties.set_data_page_size_limit(
            page_size.clamp(MIN_PARQUET_PAGE_SIZE, MAX_PARQUET_PAGE_SIZE),
        );
    }
    if let Some(dictionary) = serializer.enable_dictionary_compression {
        properties = properties.set_dictionary_enabled(dictionary);
    }
    Ok(properties.build())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Hive type names a Glue column declares, each with the leaf it is written as: the
    /// names come from Hive's own data types, which is what `Column$Type` carries.
    #[test]
    fn every_declared_hive_type_reads_as_its_leaf() {
        let cases: &[(&str, Option<ColumnType>)] = &[
            ("boolean", Some(ColumnType::Boolean)),
            ("BOOLEAN", Some(ColumnType::Boolean)),
            ("tinyint", Some(ColumnType::Integer(IntegerWidth::Tiny))),
            ("smallint", Some(ColumnType::Integer(IntegerWidth::Small))),
            ("int", Some(ColumnType::Integer(IntegerWidth::Int))),
            ("integer", Some(ColumnType::Integer(IntegerWidth::Int))),
            ("bigint", Some(ColumnType::BigInt)),
            ("float", Some(ColumnType::Float)),
            ("double", Some(ColumnType::Double)),
            ("string", Some(ColumnType::Text)),
            ("char(3)", Some(ColumnType::Text)),
            ("varchar(255)", Some(ColumnType::Text)),
            ("binary", Some(ColumnType::Binary)),
            ("date", Some(ColumnType::Date)),
            ("timestamp", Some(ColumnType::Timestamp)),
            ("decimal(10,2)", None),
            ("array<string>", None),
            ("struct<a:int>", None),
            ("", None),
        ];
        for (declared, expected) in cases {
            let read = ColumnType::of_declared(declared);
            assert_eq!(
                read, *expected,
                "case {declared:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// A record converts into the row its columns' types admit, and one carrying a value a
    /// column does not hold does not convert at all — in the failure class its own conversion
    /// error code comes from.
    #[test]
    fn a_row_holds_only_values_its_columns_admit() {
        let columns = vec![
            ("flag".to_string(), HiveType::Primitive(ColumnType::Boolean)),
            ("count".to_string(), HiveType::Primitive(ColumnType::BigInt)),
            ("name".to_string(), HiveType::Primitive(ColumnType::Text)),
        ];
        let deserializer = JsonDeserializer::default();
        let budget = nested::CellBudget::of_row_group;
        let row = nested::row_of(
            br#"{"flag": true, "count": 7, "name": "a"}"#,
            &columns,
            &deserializer,
            &mut budget(),
        );
        assert!(
            row.as_ref().is_ok_and(|row| row
                == &[
                    Value::Cell(Cell::Boolean(true)),
                    Value::Cell(Cell::Int64(7)),
                    Value::Cell(Cell::Text("a".to_string()))
                ]),
            "case conforming: expected the three cells, got {row:?}"
        );
        let missing = nested::row_of(
            br#"{"flag": true}"#,
            &columns,
            &deserializer,
            &mut budget(),
        );
        assert!(
            missing.as_ref().is_ok_and(|row| row
                == &[Value::Cell(Cell::Boolean(true)), Value::Null, Value::Null]),
            "case omitted columns: expected nulls, got {missing:?}"
        );
        // Each record that does not convert, with the class its failure belongs to: a record
        // whose bytes could not be READ is not the same failure as one that was read and
        // carries a value the column does not hold.
        let rejected: &[(&[u8], &str)] = &[
            (br#"{"count": "seven"}"#, "malformed"),
            (br#"{"count": 1.5}"#, "malformed"),
            (br#"{"name": {"a": 1}}"#, "malformed"),
            (br#"{"flag": "maybe"}"#, "malformed"),
            (b"not json at all", "unparseable"),
            (br#"[1, 2]"#, "unparseable"),
        ];
        for (record, expected) in rejected {
            let read = nested::row_of(record, &columns, &deserializer, &mut budget());
            let class = match &read {
                Err(nested::RowFault::Rejected(Rejection::Unparseable(_))) => "unparseable",
                Err(nested::RowFault::Rejected(Rejection::Malformed(_))) => "malformed",
                Err(nested::RowFault::TooManyCells) => "too many cells",
                Ok(_) => "converted",
            };
            assert_eq!(
                class,
                *expected,
                "case {:?}: expected {expected}, got {read:?}",
                String::from_utf8_lossy(record)
            );
        }
    }
}
