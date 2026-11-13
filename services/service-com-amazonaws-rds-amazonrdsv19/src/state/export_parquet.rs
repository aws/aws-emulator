// SPDX-License-Identifier: CC0-1.0

//! One exported table as the Apache Parquet file Amazon RDS writes it as.
//!
//! "The exported data is stored in Apache Parquet format" (Exporting DB snapshot data to
//! Amazon S3, Amazon RDS User Guide), compressed per column, which is why the objects an
//! export writes are named `*.gz.parquet`. So the bytes this module produces are a real
//! Parquet file carrying the rows the source database holds: written by the Apache
//! implementation of the format ([`parquet`]) through its row-group writer, not assembled
//! by hand here, and read back by any Parquet reader.
//!
//! The column types are the SOURCE's. SQLite — the engine each DB instance's databases run
//! on ([`super::data_plane::sql_engine`]) — fixes a column's type affinity from its declared
//! type (`INT` in the name means INTEGER affinity, `CHAR`/`CLOB`/`TEXT` TEXT, `BLOB` or no
//! declared type BLOB, `REAL`/`FLOA`/`DOUB` REAL, everything else NUMERIC: "Determination
//! of column affinity", SQLite datatypes), and that affinity is what a column holding no
//! value at all is exported as. A column that holds values is exported as the narrowest type
//! that carries every one of them exactly, so no exported value is lost: an integer column
//! stays a 64-bit integer whatever its affinity, and a column mixing kinds is exported as
//! strings rather than nulling the rows one type could not hold.

use ::bigdecimal::num_bigint::BigInt;
use ::bigdecimal::{BigDecimal, RoundingMode};
use ::parquet::basic::{
    Compression, ConvertedType, GzipLevel, LogicalType, Repetition, Type as PhysicalType,
};
use ::parquet::data_type::{
    ByteArray, ByteArrayType, DoubleType, FixedLenByteArray, FixedLenByteArrayType, Int64Type,
};
use ::parquet::file::properties::WriterProperties;
use ::parquet::file::writer::SerializedFileWriter;
use ::parquet::schema::types::Type as SchemaType;

use super::data_plane::sql_engine::{Cell, ColumnInfo, ResultSet};

/// The declared type names of an EXACT numeric column: "DECIMAL(M,D) ... DEC, NUMERIC, and
/// FIXED are synonyms for DECIMAL" (MySQL numeric data types), and `numeric` and `decimal`
/// are the one exact type of PostgreSQL. A column declared one of these holds a value with
/// a precision and a scale, which is what the export has to carry.
const DECIMAL_TYPE_NAMES: [&str; 4] = ["DECIMAL", "NUMERIC", "DEC", "FIXED"];

/// The precision a column declared an exact numeric type WITHOUT one has: "DECIMAL(M,D)
/// ... if M is omitted, the default is 10" (MySQL numeric type syntax).
const DEFAULT_DECIMAL_PRECISION: u32 = 10;

/// The scale such a column has: "if D is omitted, the default is 0", so the column holds
/// whole numbers.
const DEFAULT_DECIMAL_SCALE: i32 = 0;

/// The most digits one exported DECIMAL column carries: a Parquet DECIMAL is a two's
/// complement integer, and 38 digits is the most a 16-byte one holds — the width every
/// Parquet reader supports. A column declaring more precision than that is exported as its
/// DIGITS instead ([`ColumnType::of_declared`]), so no value is lost.
const MAX_DECIMAL_PRECISION: u32 = 38;

/// The bytes such a column's widest value is written in; see [`MAX_DECIMAL_PRECISION`].
const MAX_DECIMAL_BYTES: usize = 16;

/// The Parquet physical type one exported column is written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnType {
    /// A 64-bit integer column: SQLite's INTEGER affinity.
    Integer,
    /// A double column: SQLite's REAL affinity.
    Double,
    /// An EXACT numeric column, with the precision and the scale its source column was
    /// declared with: a Parquet DECIMAL, whose values are the unscaled integers `width`
    /// bytes carry.
    Decimal {
        /// How many digits the column holds, `DECIMAL(precision, scale)`.
        precision: u32,
        /// How many of them are after the point.
        scale: i32,
        /// The bytes one value is written in; see [`decimal_byte_width`].
        width: usize,
    },
    /// A column declared EXACT with more digits than a Parquet decimal carries: exported as
    /// the DIGITS of each of its values, which hold every one of them, rather than as a
    /// decimal or a floating-point type that would drop some.
    ExactDigits,
    /// A UTF-8 string column: SQLite's TEXT affinity, and what a column carrying values of
    /// more than one type is widened to.
    Text,
    /// An opaque byte column: SQLite's BLOB affinity.
    Blob,
}

impl ColumnType {
    /// The affinity SQLite gives a column whose declared type is `declared` — "Determination
    /// of column affinity": a declared type containing `INT` has INTEGER affinity; one
    /// containing `CHAR`, `CLOB` or `TEXT` has TEXT; one containing `BLOB`, or no declared
    /// type at all, has BLOB; one containing `REAL`, `FLOA` or `DOUB` has REAL; anything
    /// else has NUMERIC affinity, whose values this export writes as doubles.
    ///
    /// A column the source declared an EXACT numeric type is the exception: its values have
    /// a precision and a scale, which a binary floating-point type cannot carry, so it is
    /// exported as the Parquet DECIMAL of that precision and scale whatever values it holds.
    ///
    /// The affinity decides the exported type of a column holding NO value at all; a column
    /// that holds values is exported as [`Self::of_values`] reads them.
    fn of_declared(declared: Option<&str>) -> Self {
        let Some(declared) = declared else {
            return Self::Blob;
        };
        let declared = declared.to_ascii_uppercase();
        if let Some((precision, scale)) = declared_decimal(&declared) {
            // More digits than a Parquet DECIMAL carries: the column is exported as its
            // digits, which hold every one of them, rather than as a decimal that would
            // drop some.
            if precision > MAX_DECIMAL_PRECISION {
                return Self::ExactDigits;
            }
            return Self::Decimal {
                precision,
                scale,
                width: decimal_byte_width(precision),
            };
        }
        if declared.contains("INT") {
            return Self::Integer;
        }
        if declared.contains("CHAR") || declared.contains("CLOB") || declared.contains("TEXT") {
            return Self::Text;
        }
        if declared.contains("BLOB") || declared.is_empty() {
            return Self::Blob;
        }
        if declared.contains("REAL") || declared.contains("FLOA") || declared.contains("DOUB") {
            return Self::Double;
        }
        Self::Double
    }

    /// The type a column is exported as once the values it HOLDS are known: the narrowest
    /// one that carries every value exactly, so no exported value is lost.
    ///
    /// Integers are exported as `INT64` whatever the column's affinity — a 64-bit value
    /// beyond a double's exact range would otherwise lose precision — a column mixing
    /// integers and reals as doubles, one holding only byte strings as bytes, and a column
    /// mixing kinds as strings, which carry every value. A column holding nothing but nulls
    /// has no value to read, so it keeps the affinity its declared type gives it
    /// (`declared`).
    ///
    /// A column DECLARED exact keeps its declared precision and scale, which is the point of
    /// declaring it: only a value that precision cannot carry widens it, and then to the
    /// digits themselves rather than to a floating-point type.
    fn of_values(declared: Self, values: &[Cell]) -> Self {
        let present: Vec<&Cell> = values
            .iter()
            .filter(|cell| !matches!(cell, Cell::Null))
            .collect();
        if declared == Self::ExactDigits {
            return declared;
        }
        if let Self::Decimal { scale, width, .. } = declared {
            let carried = present.iter().all(|cell| {
                unscaled_value(cell, scale)
                    .is_some_and(|unscaled| fixed_bytes(&unscaled, width).is_some())
            });
            return if carried { declared } else { Self::ExactDigits };
        }
        if present.is_empty() {
            return declared;
        }
        if present
            .iter()
            .all(|cell| matches!(cell, Cell::Integer(_)))
        {
            return Self::Integer;
        }
        if present
            .iter()
            .all(|cell| matches!(cell, Cell::Integer(_) | Cell::Real(_)))
        {
            return Self::Double;
        }
        if present.iter().all(|cell| matches!(cell, Cell::Blob(_))) {
            return Self::Blob;
        }
        Self::Text
    }

    /// This column's Parquet physical type and the logical (converted) type that says how
    /// its bytes are read.
    fn physical(self) -> (PhysicalType, ConvertedType) {
        match self {
            Self::Integer => (PhysicalType::INT64, ConvertedType::NONE),
            Self::Double => (PhysicalType::DOUBLE, ConvertedType::NONE),
            Self::Decimal { .. } => (PhysicalType::FIXED_LEN_BYTE_ARRAY, ConvertedType::DECIMAL),
            Self::ExactDigits | Self::Text => (PhysicalType::BYTE_ARRAY, ConvertedType::UTF8),
            Self::Blob => (PhysicalType::BYTE_ARRAY, ConvertedType::NONE),
        }
    }
}

/// The `(precision, scale)` of a column whose declared type — ALREADY UPPERCASED — is an
/// exact numeric one, and `None` for every other declared type.
///
/// The type NAME is the last word before the arguments and the arguments are what the
/// parentheses enclose, so an attribute the engine's own spelling carries around them
/// (`UNSIGNED DECIMAL(8,3)`, `DECIMAL(8,3) UNSIGNED`) does not hide the type or its digits.
///
/// The arguments are the declared ones, and the defaults where the declaration omits them
/// ([`DEFAULT_DECIMAL_PRECISION`], [`DEFAULT_DECIMAL_SCALE`]). A declaration whose arguments
/// do not read as a precision and a scale at all names no exact type here: a scale cannot
/// exceed its precision, and a precision of zero carries no digit.
fn declared_decimal(declared: &str) -> Option<(u32, i32)> {
    let (name, arguments) = match declared.split_once('(') {
        Some((name, rest)) => (
            name,
            rest.split_once(')').map_or(rest, |(arguments, _)| arguments),
        ),
        None => (declared, ""),
    };
    if !name
        .split_whitespace()
        .next_back()
        .is_some_and(|name| DECIMAL_TYPE_NAMES.contains(&name))
    {
        return None;
    }
    let mut declared_arguments = arguments.split(',').map(|argument| argument.trim());
    let precision = match declared_arguments.next().filter(|first| !first.is_empty()) {
        Some(first) => first.parse::<u32>().ok()?,
        None => DEFAULT_DECIMAL_PRECISION,
    };
    let scale = match declared_arguments.next().filter(|second| !second.is_empty()) {
        Some(second) => second.parse::<i32>().ok()?,
        None => DEFAULT_DECIMAL_SCALE,
    };
    (precision >= 1 && scale >= 0 && u32::try_from(scale).ok()? <= precision)
        .then_some((precision, scale))
}

/// How many bytes a DECIMAL of `precision` digits is written in: the fewest whose two's
/// complement range covers every value of that precision, which is what a Parquet
/// `FIXED_LEN_BYTE_ARRAY` decimal column is declared with.
fn decimal_byte_width(precision: u32) -> usize {
    let bound = BigInt::from(10).pow(precision);
    (1..MAX_DECIMAL_BYTES)
        .find(|width| {
            BigInt::from(2).pow(
                u32::try_from(width * 8).unwrap_or(u32::MAX).saturating_sub(1),
            ) >= bound
        })
        .unwrap_or(MAX_DECIMAL_BYTES)
}

/// One cell as the UNSCALED integer a DECIMAL of `scale` carries it as — the value times
/// `10^scale`, rounded half up as an engine rounds a value into a declared scale.
///
/// `None` for a null and for a value that denotes no number at all, which the column
/// carries as a null rather than as a number nothing said.
fn unscaled_value(cell: &Cell, scale: i32) -> Option<BigInt> {
    let value: BigDecimal = match cell {
        Cell::Null => return None,
        Cell::Integer(number) => BigDecimal::from(*number),
        // The engine's OWN rendering of a stored double, which is the shortest decimal that
        // reads back as that value — not the exact binary expansion, whose fifty-odd digits
        // are an artefact of the storage rather than the value the source holds.
        Cell::Real(number) => number.to_string().parse().ok()?,
        Cell::Text(text) => text.trim().parse().ok()?,
        Cell::Blob(bytes) => String::from_utf8_lossy(bytes).trim().parse().ok()?,
    };
    // Scaling a value into the column's scale MATERIALISES its digits, so a value whose own
    // digit count or scale is further out than a Parquet decimal can hold is not scaled at
    // all: it is no value this column can carry, and the column carries its digits instead
    // ([`ColumnType::of_values`]). That is also what keeps a numeral a caller can write in
    // one short literal (`1e2000000000`) from being multiplied out here.
    let bound = i64::from(MAX_DECIMAL_PRECISION);
    if value.digits() > u64::from(MAX_DECIMAL_PRECISION)
        || (i64::from(scale) - value.fractional_digit_count()).abs() > bound
    {
        return None;
    }
    let (unscaled, _) = value
        .with_scale_round(i64::from(scale), RoundingMode::HalfUp)
        .into_bigint_and_exponent();
    Some(unscaled)
}

/// `unscaled` as the `width` big-endian two's complement bytes a Parquet DECIMAL leaf holds,
/// and `None` for a value those bytes cannot carry.
fn fixed_bytes(unscaled: &BigInt, width: usize) -> Option<Vec<u8>> {
    let minimal = unscaled.to_signed_bytes_be();
    if minimal.len() > width {
        return None;
    }
    // Sign-extended to the leaf's own width: a negative value is padded with ones, a
    // non-negative one with zeros, which is what two's complement of a fixed width is.
    let padding = u8::from(unscaled.sign() == ::bigdecimal::num_bigint::Sign::Minus) * 0xFF;
    let mut bytes = vec![padding; width - minimal.len()];
    bytes.extend_from_slice(&minimal);
    Some(bytes)
}

/// The bytes of the Parquet file holding `rows`, one row group, every column OPTIONAL
/// because every value of the source engine may be null.
///
/// `Err` carries what the Parquet writer refused, which the export reports as the cause it
/// failed with rather than writing a truncated object.
pub fn table_parquet_bytes(table: &str, rows: &ResultSet) -> Result<Vec<u8>, String> {
    let types: Vec<ColumnType> = rows
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let values: Vec<Cell> = rows
                .rows
                .iter()
                .filter_map(|row| row.get(index).cloned())
                .collect();
            ColumnType::of_values(
                ColumnType::of_declared(column.declared_type.as_deref()),
                &values,
            )
        })
        .collect();
    let schema = message_type(table, &rows.columns, &types)?;
    // GZIP, which is the compression the `.gz.parquet` objects of an export carry.
    let properties = WriterProperties::builder()
        .set_compression(Compression::GZIP(GzipLevel::default()))
        .build();
    let mut writer = SerializedFileWriter::new(
        Vec::new(),
        ::std::sync::Arc::new(schema),
        ::std::sync::Arc::new(properties),
    )
    .map_err(|error| error.to_string())?;
    {
        let mut row_group = writer.next_row_group().map_err(|error| error.to_string())?;
        for (index, column_type) in types.iter().enumerate() {
            let mut column = row_group
                .next_column()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("the Parquet schema of {table} lost a column"))?;
            let values: Vec<Cell> = rows
                .rows
                .iter()
                .map(|row| row.get(index).cloned().unwrap_or(Cell::Null))
                .collect();
            write_column(&mut column, *column_type, &values).map_err(|error| error.to_string())?;
            column.close().map_err(|error| error.to_string())?;
        }
        row_group.close().map_err(|error| error.to_string())?;
    }
    writer
        .into_inner()
        .map_err(|error| error.to_string())
        .map(|buffer| buffer.to_vec())
}

/// The Parquet message type of one exported table: one OPTIONAL leaf per source column.
fn message_type(
    table: &str,
    columns: &[ColumnInfo],
    types: &[ColumnType],
) -> Result<SchemaType, String> {
    let mut fields = Vec::with_capacity(columns.len());
    for (column, column_type) in columns.iter().zip(types) {
        let (physical, converted) = column_type.physical();
        let mut field = SchemaType::primitive_type_builder(&column.name, physical)
            // Every column is optional: a source value may be null, and a null is carried
            // by the definition levels the write below passes.
            .with_repetition(Repetition::OPTIONAL)
            .with_converted_type(converted);
        // An exact numeric column carries the precision and the scale of its source column,
        // in both the logical type a current reader reads and the converted type an older
        // one does, over a leaf as wide as that precision needs.
        if let ColumnType::Decimal {
            precision,
            scale,
            width,
        } = *column_type
        {
            let precision = i32::try_from(precision).unwrap_or(i32::MAX);
            field = field
                .with_length(i32::try_from(width).unwrap_or(i32::MAX))
                .with_precision(precision)
                .with_scale(scale)
                .with_logical_type(Some(LogicalType::Decimal(
                    ::parquet::basic::DecimalType { scale, precision },
                )));
        }
        let field = field.build().map_err(|error| error.to_string())?;
        fields.push(::std::sync::Arc::new(field));
    }
    SchemaType::group_type_builder(table)
        .with_fields(fields)
        .build()
        .map_err(|error| error.to_string())
}

/// Write one column's values, with the definition level of each row: 1 for a value that is
/// present and 0 for a null, which is how an OPTIONAL Parquet leaf carries one.
fn write_column(
    column: &mut ::parquet::file::writer::SerializedColumnWriter<'_>,
    column_type: ColumnType,
    values: &[Cell],
) -> Result<(), ::parquet::errors::ParquetError> {
    let definitions: Vec<i16> = values
        .iter()
        .map(|cell| i16::from(!matches!(cell, Cell::Null)))
        .collect();
    match column_type {
        ColumnType::Integer => {
            let present: Vec<i64> = values
                .iter()
                .filter_map(|cell| match cell {
                    Cell::Integer(value) => Some(*value),
                    _ => None,
                })
                .collect();
            column
                .typed::<Int64Type>()
                .write_batch(&present, Some(&definitions), None)?;
        }
        ColumnType::Double => {
            let present: Vec<f64> = values
                .iter()
                .filter_map(|cell| match cell {
                    Cell::Integer(value) => Some(*value as f64),
                    Cell::Real(value) => Some(*value),
                    _ => None,
                })
                .collect();
            column
                .typed::<DoubleType>()
                .write_batch(&present, Some(&definitions), None)?;
        }
        ColumnType::Decimal { scale, width, .. } => {
            // Each value as the bytes the leaf carries it in, and a null for a cell that
            // denotes no number: the definition levels of THIS column come from that
            // reading, so a value the column cannot carry is written as the null it is
            // rather than silently shifting the rows after it.
            let carried: Vec<Option<FixedLenByteArray>> = values
                .iter()
                .map(|cell| {
                    unscaled_value(cell, scale)
                        .and_then(|unscaled| fixed_bytes(&unscaled, width))
                        .map(|bytes| FixedLenByteArray::from(ByteArray::from(bytes)))
                })
                .collect();
            let definitions: Vec<i16> = carried
                .iter()
                .map(|value| i16::from(value.is_some()))
                .collect();
            let present: Vec<FixedLenByteArray> = carried.into_iter().flatten().collect();
            column
                .typed::<FixedLenByteArrayType>()
                .write_batch(&present, Some(&definitions), None)?;
        }
        ColumnType::ExactDigits | ColumnType::Text => {
            let present: Vec<ByteArray> = values
                .iter()
                .filter_map(text_of)
                .map(|text| ByteArray::from(text.into_bytes()))
                .collect();
            column
                .typed::<ByteArrayType>()
                .write_batch(&present, Some(&definitions), None)?;
        }
        ColumnType::Blob => {
            let present: Vec<ByteArray> = values
                .iter()
                .filter_map(|cell| match cell {
                    Cell::Blob(bytes) => Some(ByteArray::from(bytes.clone())),
                    Cell::Null => None,
                    other => text_of(other).map(|text| ByteArray::from(text.into_bytes())),
                })
                .collect();
            column
                .typed::<ByteArrayType>()
                .write_batch(&present, Some(&definitions), None)?;
        }
    }
    Ok(())
}

/// One value as the text a string column carries it as: the engine's own rendering of a
/// number, the text itself, and the lossless UTF-8 reading of a byte string (a blob that is
/// not UTF-8 is carried as the bytes it holds, which is what a `BYTE_ARRAY` leaf stores).
fn text_of(cell: &Cell) -> Option<String> {
    match cell {
        Cell::Null => None,
        Cell::Integer(value) => Some(value.to_string()),
        Cell::Real(value) => Some(value.to_string()),
        Cell::Text(text) => Some(text.clone()),
        Cell::Blob(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
    }
}
