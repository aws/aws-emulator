// SPDX-License-Identifier: CC0-1.0

//! Reading a Data Catalog table's HIVE TYPE SYNTAX: the one place this crate decides what a
//! declared column type is called and what instant a date or timestamp value names.
//!
//! `Column$Type` is "the data type of the column", which a Data Catalog table states in Hive
//! type syntax — the names its own table definitions carry (`int`, `bigint`, `string`,
//! `varchar(64)`, `decimal(10,2)`, `timestamp`). Every reader of a table's declared types
//! asks the same two questions of that syntax, so they are answered here once: which type a
//! declaration NAMES ([`base_hive_type`]), and which instant a date or timestamp value names
//! ([`hive_instant_seconds`]). Keeping them in one file is what stops two readers of the same
//! table drifting apart on either.

/// The TYPE a declaration names, lowercased and without its parameter list.
///
/// The parameter list of a parameterised name (`char`, `varchar`, `decimal`) and the member
/// list of a complex one (`array`, `map`, `struct`) do not change WHICH type is named, so
/// they are cut off; the match is lowercased because a table's declared type is
/// client-written text.
pub fn base_hive_type(column_type: &str) -> String {
    column_type
        .trim()
        .to_ascii_lowercase()
        .split(['(', '<'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The instant a date or timestamp value names, in seconds from the epoch, or `None` for a
/// value that names none.
///
/// Both Hive spellings are read: a date is `YYYY-MM-DD` and a timestamp is that followed by a
/// time of day, separated by a space or by `T` and optionally carrying a fraction of a
/// second. The reading itself is the smithy date-time parser's, over the value rewritten as
/// the instant it names in UTC — a Hive timestamp carries no zone.
pub fn hive_instant_seconds(value: &str) -> Option<i64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let normalised = value.replace(' ', "T");
    let candidate = if normalised.contains('T') {
        format!("{}Z", normalised.trim_end_matches('Z'))
    } else {
        format!("{normalised}T00:00:00Z")
    };
    ::aws_smithy_types::DateTime::from_str(
        &candidate,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()
    .map(|instant| instant.secs())
}
