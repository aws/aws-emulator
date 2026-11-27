// SPDX-License-Identifier: CC0-1.0

//! The size a statement's result is REPORTED as (`ResultSize`), and the one
//! definition of the text a result's value carries.
//!
//! `DescribeStatementResponse.ResultSize` is "the size in bytes of the returned
//! results", so it is the size of what the warehouse RETURNED for the statement
//! — the rows it sent back, as it sent them — and not of whatever the engine
//! holding those values happens to spend on them in this process. Those are two
//! different numbers, and the second one is [`super::value_bytes`], which bounds
//! what the retained history may hold.
//!
//! Amazon Redshift returns a result to its client in the PostgreSQL wire
//! protocol's data-row messages, one per row: a row message carries how many
//! fields it holds, and then each field as its own length followed by the value
//! as text. So the bytes a result returned are the bytes of those rows
//! ([`result_size`]) — the same accounting Redshift's own
//! `SYS_QUERY_HISTORY.returned_bytes` ("the total size, in bytes, of the returned
//! data") reports for the query. The column names are described once, ahead of
//! the rows rather than in a row, so they are no part of it, and a result that
//! returned no row returned no bytes.
//!
//! The text of a value is also what the CSV form of a result is written from
//! (`GetStatementResultV2`), so it is stated here once: the size a statement
//! reports and the characters a caller reads back cannot disagree.

use super::*;

/// One value as the TEXT it carries: the characters the warehouse prints for it.
///
/// A SQL NULL carries no text at all, which is the empty cell; binary carries its
/// bytes as hexadecimal, the text form Amazon Redshift prints a `VARBYTE` value
/// in. An exact-numeric value already carries the decimal text its column's scale
/// calls for ([`super::field_from`]), so it is reported with exactly the digits
/// that column declares.
pub fn value_text(field: &Field) -> String {
    match field {
        Field::isNull(_) => String::new(),
        Field::booleanValue(value) => value.to_string(),
        Field::longValue(value) => value.to_string(),
        Field::doubleValue(value) => value.to_string(),
        Field::stringValue(value) => value.clone(),
        Field::blobValue(value) => hex::encode(value),
    }
}

/// What ONE row of a result costs beyond its values: the two-byte count of the
/// fields the row carries.
const ROW_FRAMING_BYTES: usize = 2;

/// What ONE field of a row costs beyond its text: the four-byte length that
/// precedes the value. A NULL field carries that length and no text at all, so it
/// costs exactly this.
const FIELD_LENGTH_BYTES: usize = 4;

/// `ResultSize` for one result set: the bytes the warehouse returned for it — one
/// row per row of the result, each carrying its own field count
/// ([`ROW_FRAMING_BYTES`]) and then every value of that row as a length
/// ([`FIELD_LENGTH_BYTES`]) followed by the value's text ([`value_text`]).
///
/// A value's own bytes are the characters it carries, so a result's size is a
/// function of the result alone: the same rows are the same number of bytes
/// whatever storage class the engine happened to keep each value in, and a NULL
/// carries no text — only the length that says so. A result of no rows returned
/// nothing, so it measures 0.
pub fn result_size(rows: &[Vec<Field>]) -> i64 {
    let mut bytes: usize = 0;
    for row in rows {
        bytes = bytes.saturating_add(ROW_FRAMING_BYTES);
        for field in row {
            bytes = bytes
                .saturating_add(FIELD_LENGTH_BYTES)
                .saturating_add(value_text(field).len());
        }
    }
    i64::try_from(bytes).unwrap_or(i64::MAX)
}
