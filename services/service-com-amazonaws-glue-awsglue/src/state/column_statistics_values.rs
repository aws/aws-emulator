// SPDX-License-Identifier: CC0-1.0

//! What a column statistics task run COMPUTES: the statistics of the values one column
//! holds, aggregated cell by cell as the table's data is read.
//!
//! "Represents the generated column-level statistics for a table or partition"
//! (`ColumnStatistics`), and the statistics of a column are typed by the column's own
//! declared type: the modelled `ColumnStatisticsType`
//! (`BOOLEAN | DATE | DECIMAL | DOUBLE | LONG | STRING | BINARY`) picks WHICH of the
//! `ColumnStatisticsData` members carries them, and each of those members is a different
//! set of figures. This module holds that mapping ([`column_statistics_type_of`]) and one
//! accumulator per mapped type ([`ColumnStatisticsAccumulator`]), so every reader of a
//! table's data — whatever layout the objects behind it are in — aggregates a column the
//! same way.
//!
//! Nothing here reads anything: it is handed the cells
//! ([`super::column_statistics_source`] is what reads them) and answers the figures they
//! add up to. A cell the column's type cannot hold is a NULL, which is how a Hive-style
//! reader resolves a value it cannot parse, so the null count of a column is the count of
//! its cells that hold no value OF THAT TYPE.

use super::*;

/// `ColumnStatisticsData$Type` of a column of integers: of the modelled
/// `ColumnStatisticsType`, the one whose data member is "column statistics supported for
/// integer data columns" (`LongColumnStatisticsData`).
pub const COLUMN_STATISTICS_TYPE_LONG: &str = "LONG";
/// See [`COLUMN_STATISTICS_TYPE_LONG`]: "column statistics supported for floating-point
/// number data columns".
pub const COLUMN_STATISTICS_TYPE_DOUBLE: &str = "DOUBLE";
/// See [`COLUMN_STATISTICS_TYPE_LONG`]: "column statistics supported for fixed-point
/// number data columns".
pub const COLUMN_STATISTICS_TYPE_DECIMAL: &str = "DECIMAL";
/// See [`COLUMN_STATISTICS_TYPE_LONG`]: "column statistics supported for character
/// sequence data values".
pub const COLUMN_STATISTICS_TYPE_STRING: &str = "STRING";
/// See [`COLUMN_STATISTICS_TYPE_LONG`]: "column statistics supported for Boolean data
/// columns".
pub const COLUMN_STATISTICS_TYPE_BOOLEAN: &str = "BOOLEAN";
/// See [`COLUMN_STATISTICS_TYPE_LONG`]: "column statistics supported for timestamp data
/// columns" (`DateColumnStatisticsData`), which is why a `date` column and a `timestamp`
/// column are both reported under it.
pub const COLUMN_STATISTICS_TYPE_DATE: &str = "DATE";
/// See [`COLUMN_STATISTICS_TYPE_LONG`]: "column statistics supported for bit sequence
/// data values".
pub const COLUMN_STATISTICS_TYPE_BINARY: &str = "BINARY";

/// Which `ColumnStatisticsType` the statistics of a column of this declared type are
/// reported under, or `None` for a type no member of `ColumnStatisticsData` describes.
///
/// `column_type` is `Column$Type`, "the data type of the column", which a Data Catalog table
/// states in Hive type syntax — the names the catalog's own table definitions carry (`int`,
/// `bigint`, `string`, `varchar(64)`, `decimal(10,2)`, `timestamp`). WHICH type a declaration
/// names is read once, by [`super::base_hive_type`], so every reader of a table's declared
/// types agrees on it.
///
/// A COMPLEX type — `array`, `map`, `struct`, `uniontype` — maps to nothing: the modelled
/// enum has no member for one, so such a column has no column-level statistics and a run
/// over it generates none.
pub fn column_statistics_type_of(column_type: &str) -> Option<&'static str> {
    match super::base_hive_type(column_type).as_str() {
        "boolean" => Some(COLUMN_STATISTICS_TYPE_BOOLEAN),
        "tinyint" | "smallint" | "int" | "integer" | "bigint" | "long" => {
            Some(COLUMN_STATISTICS_TYPE_LONG)
        }
        "float" | "double" | "real" => Some(COLUMN_STATISTICS_TYPE_DOUBLE),
        "decimal" | "numeric" => Some(COLUMN_STATISTICS_TYPE_DECIMAL),
        "string" | "char" | "varchar" => Some(COLUMN_STATISTICS_TYPE_STRING),
        "date" | "timestamp" => Some(COLUMN_STATISTICS_TYPE_DATE),
        "binary" => Some(COLUMN_STATISTICS_TYPE_BINARY),
        _ => None,
    }
}

/// The `ColumnStatisticsData` member the figures of `data` belong in and which `data` does
/// NOT carry, as the model names that member — or `None` when the figures its own `Type`
/// names are there.
///
/// "Contains the individual types of column statistics data. Only one data object should be
/// set and indicated by the `Type` attribute" (`ColumnStatisticsData`), so the
/// discriminator selects WHICH member has to carry the figures, and a structure whose
/// selected member is absent states no statistics at all. `Type` is the modelled
/// `ColumnStatisticsType`, which the request layer validates a submitted value against, so
/// a value naming no member of the shape does not reach here.
///
/// ONE place that pairs a `ColumnStatisticsType` with its member, so a write that refuses
/// an entry names the same member a reader would have read the figures out of.
pub fn missing_statistics_data_member(data: &ColumnStatisticsData) -> Option<&'static str> {
    let (member, stated) = match data.r#type.as_str() {
        COLUMN_STATISTICS_TYPE_BOOLEAN => (
            "BooleanColumnStatisticsData",
            data.boolean_column_statistics_data.is_some(),
        ),
        COLUMN_STATISTICS_TYPE_DATE => (
            "DateColumnStatisticsData",
            data.date_column_statistics_data.is_some(),
        ),
        COLUMN_STATISTICS_TYPE_DECIMAL => (
            "DecimalColumnStatisticsData",
            data.decimal_column_statistics_data.is_some(),
        ),
        COLUMN_STATISTICS_TYPE_DOUBLE => (
            "DoubleColumnStatisticsData",
            data.double_column_statistics_data.is_some(),
        ),
        COLUMN_STATISTICS_TYPE_LONG => (
            "LongColumnStatisticsData",
            data.long_column_statistics_data.is_some(),
        ),
        COLUMN_STATISTICS_TYPE_STRING => (
            "StringColumnStatisticsData",
            data.string_column_statistics_data.is_some(),
        ),
        COLUMN_STATISTICS_TYPE_BINARY => (
            "BinaryColumnStatisticsData",
            data.binary_column_statistics_data.is_some(),
        ),
        _ => return None,
    };
    (!stated).then_some(member)
}

/// The statistics of ONE column, as a run computed them over the table's data.
///
/// Held apart from the modelled `ColumnStatistics` because that shape carries
/// `AnalyzedTime`, "the timestamp of when column statistics were generated", which is the
/// instant the RUN finished rather than anything the data says — so it is stamped on when
/// the run settles ([`Self::to_column_statistics`]) and two reads of a settled run's
/// statistics report the same instant.
#[derive(Debug, Clone)]
pub struct ComputedColumnStatistics {
    /// "Name of column which statistics belong to", exactly as the stored table declares
    /// it.
    pub column_name: String,
    /// "The data type of the column", exactly as the stored table declares it.
    pub column_type: String,
    /// The figures the column's values add up to, under the
    /// `ColumnStatisticsData$Type` its declared type maps to.
    pub statistics_data: ColumnStatisticsData,
}

impl ComputedColumnStatistics {
    /// These statistics in the modelled `ColumnStatistics` shape, generated at
    /// `analyzed_at`.
    pub fn to_column_statistics(&self, analyzed_at: SystemTime) -> ColumnStatistics {
        ColumnStatistics {
            column_name: self.column_name.clone(),
            column_type: self.column_type.clone(),
            analyzed_time: to_date_time(analyzed_at),
            statistics_data: self.statistics_data.clone(),
        }
    }
}

/// The running statistics of one column, fed one cell at a time.
#[derive(Debug)]
pub struct ColumnStatisticsAccumulator {
    /// The column's declared name, carried into the statistics it produces.
    name: String,
    /// The column's declared type, carried into the statistics it produces.
    declared_type: String,
    /// The figures so far, in the shape the column's type is reported under.
    values: ColumnValues,
}

impl ColumnStatisticsAccumulator {
    /// An accumulator for a column of this declared type, or `None` when the type has no
    /// column-level statistics at all ([`column_statistics_type_of`]).
    pub fn for_column(name: &str, declared_type: &str) -> Option<Self> {
        let values = match column_statistics_type_of(declared_type)? {
            COLUMN_STATISTICS_TYPE_BOOLEAN => ColumnValues::Boolean {
                nulls: 0,
                trues: 0,
                falses: 0,
            },
            COLUMN_STATISTICS_TYPE_LONG => ColumnValues::Long {
                nulls: 0,
                distinct: HashSet::new(),
                minimum: None,
                maximum: None,
            },
            COLUMN_STATISTICS_TYPE_DOUBLE => ColumnValues::Double {
                nulls: 0,
                distinct: HashSet::new(),
                minimum: None,
                maximum: None,
            },
            COLUMN_STATISTICS_TYPE_DECIMAL => ColumnValues::Decimal {
                nulls: 0,
                distinct: HashSet::new(),
                minimum: None,
                maximum: None,
            },
            COLUMN_STATISTICS_TYPE_DATE => ColumnValues::Date {
                nulls: 0,
                distinct: HashSet::new(),
                minimum: None,
                maximum: None,
            },
            COLUMN_STATISTICS_TYPE_BINARY => ColumnValues::Binary {
                nulls: 0,
                values: 0,
                maximum_length: 0,
                total_length: 0,
            },
            // Every other mapped type is the character-sequence one.
            _ => ColumnValues::Text {
                nulls: 0,
                distinct: HashSet::new(),
                values: 0,
                maximum_length: 0,
                total_length: 0,
            },
        };
        Some(Self {
            name: name.to_string(),
            declared_type: declared_type.to_string(),
            values,
        })
    }

    /// Take one cell of this column: `None` is a cell holding NO value — the row did not
    /// carry the column, or the cell held the layout's null marker.
    pub fn observe(&mut self, cell: Option<&str>) {
        self.values.observe(cell);
    }

    /// The statistics the cells taken so far add up to.
    pub fn finish(self) -> ComputedColumnStatistics {
        ComputedColumnStatistics {
            column_name: self.name,
            column_type: self.declared_type,
            statistics_data: self.values.to_statistics_data(),
        }
    }
}

/// The figures one column's cells add up to, one variant per `ColumnStatisticsType` that
/// a declared column type maps to.
///
/// `NumberOfDistinctValues` counts the DISTINCT VALUES of the column, so a null is not one
/// of them: it is counted by `NumberOfNulls` instead, and the two figures are the counts
/// the model names ("the number of distinct values in a column", "the number of null
/// values in the column"). The distinct values are held as a set of the values seen, whose
/// size is bounded by the number of cells read, which the reader bounds
/// ([`super::column_statistics_source::MAX_COLUMN_STATISTICS_ROWS`]).
#[derive(Debug)]
enum ColumnValues {
    /// A Boolean column, which reports how many of each truth value it holds and no
    /// distinct count: the modelled `BooleanColumnStatisticsData` has none.
    Boolean {
        nulls: i64,
        trues: i64,
        falses: i64,
    },
    /// An integer column.
    Long {
        nulls: i64,
        distinct: HashSet<i64>,
        minimum: Option<i64>,
        maximum: Option<i64>,
    },
    /// A floating-point column. The distinct values are keyed by the bit pattern of the
    /// parsed number, so two spellings of one value (`1`, `1.0`) count once.
    Double {
        nulls: i64,
        distinct: HashSet<u64>,
        minimum: Option<f64>,
        maximum: Option<f64>,
    },
    /// A fixed-point column. Whether two cells hold the same value is decided by their
    /// normalised spelling, so `1.10` and `1.1` are one value; the extreme values keep the
    /// spelling their own cell was written with, which is the unscaled value and the scale
    /// the statistics report.
    Decimal {
        nulls: i64,
        distinct: HashSet<String>,
        minimum: Option<(f64, String)>,
        maximum: Option<(f64, String)>,
    },
    /// A date or timestamp column, keyed by the instant the cell names, in seconds from
    /// the epoch.
    Date {
        nulls: i64,
        distinct: HashSet<i64>,
        minimum: Option<i64>,
        maximum: Option<i64>,
    },
    /// A character-sequence column.
    Text {
        nulls: i64,
        distinct: HashSet<String>,
        values: i64,
        maximum_length: i64,
        total_length: u128,
    },
    /// A bit-sequence column, which reports lengths and nulls and no distinct count: the
    /// modelled `BinaryColumnStatisticsData` has none.
    Binary {
        nulls: i64,
        values: i64,
        maximum_length: i64,
        total_length: u128,
    },
}

impl ColumnValues {
    /// Take one cell, counting it as a value of this column's type or as a null.
    fn observe(&mut self, cell: Option<&str>) {
        match self {
            Self::Boolean {
                nulls,
                trues,
                falses,
            } => match cell.and_then(parse_boolean) {
                Some(true) => *trues += 1,
                Some(false) => *falses += 1,
                None => *nulls += 1,
            },
            Self::Long {
                nulls,
                distinct,
                minimum,
                maximum,
            } => match cell.and_then(|cell| cell.trim().parse::<i64>().ok()) {
                Some(value) => {
                    distinct.insert(value);
                    *minimum = Some(minimum.map_or(value, |held: i64| held.min(value)));
                    *maximum = Some(maximum.map_or(value, |held: i64| held.max(value)));
                }
                None => *nulls += 1,
            },
            Self::Double {
                nulls,
                distinct,
                minimum,
                maximum,
            } => match cell.and_then(parse_double) {
                Some(value) => {
                    // Keyed by the bit pattern of the value, with the two spellings of
                    // zero normalised to one — they are the same number, so they are one
                    // distinct value.
                    let key = if value == 0.0 { 0.0 } else { value };
                    distinct.insert(key.to_bits());
                    *minimum = Some(minimum.map_or(value, |held: f64| held.min(value)));
                    *maximum = Some(maximum.map_or(value, |held: f64| held.max(value)));
                }
                None => *nulls += 1,
            },
            Self::Decimal {
                nulls,
                distinct,
                minimum,
                maximum,
            } => match cell.and_then(parse_decimal) {
                Some((order, normalised, written)) => {
                    distinct.insert(normalised);
                    if minimum.as_ref().is_none_or(|(held, _)| order < *held) {
                        *minimum = Some((order, written.clone()));
                    }
                    if maximum.as_ref().is_none_or(|(held, _)| order > *held) {
                        *maximum = Some((order, written));
                    }
                }
                None => *nulls += 1,
            },
            Self::Date {
                nulls,
                distinct,
                minimum,
                maximum,
            } => match cell.and_then(parse_instant_seconds) {
                Some(value) => {
                    distinct.insert(value);
                    *minimum = Some(minimum.map_or(value, |held: i64| held.min(value)));
                    *maximum = Some(maximum.map_or(value, |held: i64| held.max(value)));
                }
                None => *nulls += 1,
            },
            Self::Text {
                nulls,
                distinct,
                values,
                maximum_length,
                total_length,
            } => match cell {
                // A character sequence holds whatever the cell held, the empty sequence
                // included: only the layout's null marker (which the reader resolves to
                // `None`) is a null here.
                Some(text) => {
                    let length = i64::try_from(text.chars().count()).unwrap_or(i64::MAX);
                    distinct.insert(text.to_string());
                    *values += 1;
                    *maximum_length = (*maximum_length).max(length);
                    *total_length += u128::try_from(length).unwrap_or_default();
                }
                None => *nulls += 1,
            },
            Self::Binary {
                nulls,
                values,
                maximum_length,
                total_length,
            } => match cell.and_then(decoded_binary_length) {
                Some(length) => {
                    *values += 1;
                    *maximum_length = (*maximum_length).max(length);
                    *total_length += u128::try_from(length).unwrap_or_default();
                }
                None => *nulls += 1,
            },
        }
    }

    /// These figures in the modelled `ColumnStatisticsData`: the `Type` the column is
    /// reported under, and the ONE data member that type names ("only one data object
    /// should be set and indicated by the `Type` attribute").
    fn to_statistics_data(&self) -> ColumnStatisticsData {
        match self {
            Self::Boolean {
                nulls,
                trues,
                falses,
            } => ColumnStatisticsData {
                r#type: COLUMN_STATISTICS_TYPE_BOOLEAN.to_string(),
                boolean_column_statistics_data: Some(BooleanColumnStatisticsData {
                    number_of_trues: *trues,
                    number_of_falses: *falses,
                    number_of_nulls: *nulls,
                }),
                ..ColumnStatisticsData::default()
            },
            Self::Long {
                nulls,
                distinct,
                minimum,
                maximum,
            } => ColumnStatisticsData {
                r#type: COLUMN_STATISTICS_TYPE_LONG.to_string(),
                long_column_statistics_data: Some(LongColumnStatisticsData {
                    // "The lowest value in the column" and "the highest value in the
                    // column" are not required members and have the model's own default
                    // of 0, which is what a column holding no value at all reports.
                    minimum_value: minimum.unwrap_or_default(),
                    maximum_value: maximum.unwrap_or_default(),
                    number_of_nulls: *nulls,
                    number_of_distinct_values: distinct_count(distinct.len()),
                }),
                ..ColumnStatisticsData::default()
            },
            Self::Double {
                nulls,
                distinct,
                minimum,
                maximum,
            } => ColumnStatisticsData {
                r#type: COLUMN_STATISTICS_TYPE_DOUBLE.to_string(),
                double_column_statistics_data: Some(DoubleColumnStatisticsData {
                    minimum_value: minimum.unwrap_or_default(),
                    maximum_value: maximum.unwrap_or_default(),
                    number_of_nulls: *nulls,
                    number_of_distinct_values: distinct_count(distinct.len()),
                }),
                ..ColumnStatisticsData::default()
            },
            Self::Decimal {
                nulls,
                distinct,
                minimum,
                maximum,
            } => ColumnStatisticsData {
                r#type: COLUMN_STATISTICS_TYPE_DECIMAL.to_string(),
                decimal_column_statistics_data: Some(DecimalColumnStatisticsData {
                    // A fixed-point extreme is an optional `DecimalNumber`, so a column
                    // holding no value reports none rather than a zero that stood for
                    // nothing.
                    minimum_value: minimum
                        .as_ref()
                        .map(|(_, written)| to_decimal_number(written)),
                    maximum_value: maximum
                        .as_ref()
                        .map(|(_, written)| to_decimal_number(written)),
                    number_of_nulls: *nulls,
                    number_of_distinct_values: distinct_count(distinct.len()),
                }),
                ..ColumnStatisticsData::default()
            },
            Self::Date {
                nulls,
                distinct,
                minimum,
                maximum,
            } => ColumnStatisticsData {
                r#type: COLUMN_STATISTICS_TYPE_DATE.to_string(),
                date_column_statistics_data: Some(DateColumnStatisticsData {
                    minimum_value: minimum.map(::aws_smithy_types::DateTime::from_secs),
                    maximum_value: maximum.map(::aws_smithy_types::DateTime::from_secs),
                    number_of_nulls: *nulls,
                    number_of_distinct_values: distinct_count(distinct.len()),
                }),
                ..ColumnStatisticsData::default()
            },
            Self::Text {
                nulls,
                distinct,
                values,
                maximum_length,
                total_length,
            } => ColumnStatisticsData {
                r#type: COLUMN_STATISTICS_TYPE_STRING.to_string(),
                string_column_statistics_data: Some(StringColumnStatisticsData {
                    maximum_length: *maximum_length,
                    average_length: average_length(*total_length, *values),
                    number_of_nulls: *nulls,
                    number_of_distinct_values: distinct_count(distinct.len()),
                }),
                ..ColumnStatisticsData::default()
            },
            Self::Binary {
                nulls,
                values,
                maximum_length,
                total_length,
            } => ColumnStatisticsData {
                r#type: COLUMN_STATISTICS_TYPE_BINARY.to_string(),
                binary_column_statistics_data: Some(BinaryColumnStatisticsData {
                    maximum_length: *maximum_length,
                    average_length: average_length(*total_length, *values),
                    number_of_nulls: *nulls,
                }),
                ..ColumnStatisticsData::default()
            },
        }
    }
}

/// How many distinct values a set of them holds, as the modelled `NonNegativeLong` the
/// count is reported in.
fn distinct_count(seen: usize) -> i64 {
    i64::try_from(seen).unwrap_or(i64::MAX)
}

/// "The average string length in the column" (and the average bit-sequence length beside
/// it): the mean over the cells that HOLD a value, and 0 for a column holding none —
/// `AverageLength` is a `NonNegativeDouble` with the model's own default of 0.
fn average_length(total: u128, values: i64) -> f64 {
    if values <= 0 {
        return 0.0;
    }
    total as f64 / values as f64
}

/// How many BYTES the bit sequence a binary cell holds is long, or `None` for a cell that
/// holds no bit sequence.
///
/// "The size of the longest bit sequence in the column" and the average beside it are
/// lengths of the VALUE, and a text-format table writes a binary value base64-encoded —
/// so the cell is decoded and its decoded length counted, rather than the length of the
/// text that encodes it. A cell that is not base64 at all is not a value of this column's
/// type, which is a null, the way a Hive-style reader resolves a cell it cannot read as
/// the column's type. The decoding is the `base64` crate's.
fn decoded_binary_length(cell: &str) -> Option<i64> {
    let decoded = ::base64::Engine::decode(
        &::base64::engine::general_purpose::STANDARD_NO_PAD,
        cell.trim().trim_end_matches('='),
    )
    .ok()?;
    i64::try_from(decoded.len()).ok()
}

/// The truth value a cell holds, or `None` for a cell that holds none.
///
/// A Hive-style reader resolves a cell it cannot parse as the column's type to NULL, so
/// only the two literals of the type are values; the comparison ignores case, because the
/// cell is data written by whoever wrote the object.
fn parse_boolean(cell: &str) -> Option<bool> {
    let cell = cell.trim();
    if cell.eq_ignore_ascii_case("true") {
        return Some(true);
    }
    if cell.eq_ignore_ascii_case("false") {
        return Some(false);
    }
    None
}

/// The floating-point value a cell holds, or `None` for a cell that holds none — which
/// includes a cell spelling a value that is not a number at all (`NaN`), because such a
/// value has no place in a minimum or a maximum.
fn parse_double(cell: &str) -> Option<f64> {
    let value = cell.trim().parse::<f64>().ok()?;
    value.is_finite().then_some(value)
}

/// The fixed-point value a cell holds, as `(the value for ordering, its normalised
/// spelling, the spelling the cell carried)`, or `None` for a cell that holds none.
///
/// The normalised spelling is what decides whether two cells hold the SAME value: the sign
/// is kept, leading zeros of the integer part and trailing zeros of the fraction are
/// dropped, so `1.10`, `01.1` and `1.1` are one value. The ordering key is the value as a
/// floating-point number, which is what a minimum and a maximum are selected by — and the
/// cell's OWN spelling travels with them, because the scale a fixed-point extreme reports
/// is the scale the value was written with.
fn parse_decimal(cell: &str) -> Option<(f64, String, String)> {
    let cell = cell.trim();
    let order = cell.parse::<f64>().ok()?;
    if !order.is_finite() {
        return None;
    }
    let (negative, digits) = match cell.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, cell.strip_prefix('+').unwrap_or(cell)),
    };
    let (whole, fraction) = match digits.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (digits, ""),
    };
    if whole
        .chars()
        .chain(fraction.chars())
        .any(|character| !character.is_ascii_digit())
        || (whole.is_empty() && fraction.is_empty())
    {
        return None;
    }
    let written = format!(
        "{}{whole}{}",
        if negative { "-" } else { "" },
        if fraction.is_empty() {
            String::new()
        } else {
            format!(".{fraction}")
        }
    );
    // A fixed-point value is reported as its UNSCALED value, so a cell carrying more
    // digits than that value can hold is not a value of this column's type at all — which
    // is a null, the way a Hive-style reader resolves a cell it cannot read as the
    // column's type. The widest decimal a Data Catalog column declares is 38 digits, which
    // is inside what [`to_decimal_number`] carries.
    format!("{whole}{fraction}").parse::<i128>().ok()?;
    let trimmed_whole = whole.trim_start_matches('0');
    let trimmed_fraction = fraction.trim_end_matches('0');
    let normalised = format!(
        "{}{}{}",
        if negative && (!trimmed_whole.is_empty() || !trimmed_fraction.is_empty()) {
            "-"
        } else {
            ""
        },
        if trimmed_whole.is_empty() {
            "0"
        } else {
            trimmed_whole
        },
        if trimmed_fraction.is_empty() {
            String::new()
        } else {
            format!(".{trimmed_fraction}")
        }
    );
    Some((order, normalised, written))
}

/// A fixed-point cell's own spelling ([`parse_decimal`]) as the modelled `DecimalNumber`:
/// "UnscaledValues within are Base64-encoded binary objects storing big-endian, two's
/// complement representations of the decimal's unscaled value", with `Scale` the number of
/// digits the value carries after the point.
///
/// Only a spelling [`parse_decimal`] accepted reaches here, so its digits are known to fit
/// the unscaled value.
fn to_decimal_number(written: &str) -> DecimalNumber {
    let (negative, digits) = match written.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, written),
    };
    let (whole, fraction) = match digits.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (digits, ""),
    };
    let unscaled = format!("{whole}{fraction}")
        .parse::<i128>()
        .unwrap_or_default();
    let unscaled = if negative { -unscaled } else { unscaled };
    DecimalNumber {
        unscaled_value: two_s_complement_be(unscaled),
        scale: i32::try_from(fraction.len()).unwrap_or_default(),
    }
}

/// `value` as the SHORTEST big-endian two's complement byte sequence that still carries
/// its sign — the form a `DecimalNumber`'s unscaled value is stored in.
fn two_s_complement_be(value: i128) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let sign: u8 = if value < 0 { 0xFF } else { 0x00 };
    let mut start = 0;
    while start + 1 < bytes.len() {
        // A leading sign byte can go only while the byte after it still spells the same
        // sign in its own high bit.
        let keeps_sign = (bytes[start + 1] & 0x80 != 0) == (value < 0);
        if bytes[start] == sign && keeps_sign {
            start += 1;
        } else {
            break;
        }
    }
    bytes[start..].to_vec()
}

/// The instant a date or timestamp cell names, in seconds from the epoch, or `None` for a
/// cell that names none.
///
/// Read by [`super::hive_instant_seconds`], the one reading of a Hive date or timestamp in
/// this crate, so a cell of a table's data and a partition value of the same spelling name
/// the same instant.
fn parse_instant_seconds(cell: &str) -> Option<i64> {
    super::hive_instant_seconds(cell)
}
