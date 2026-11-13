// SPDX-License-Identifier: CC0-1.0

//! The COLUMN ORDER of a pool's user-import CSV template, which `GetCSVHeader`
//! hands out: "a comma-separated value (CSV) list populated with available user
//! attributes in the user pool ... the header for the CSV file that determines
//! the users in a user import job" (`GetCSVHeader` in this service's model).
//!
//! The template's order is the SERVICE's own, not the order the pool happens to
//! store its schema in: the same pool's attributes come back in one order
//! however the schema was configured — named by a create's `Schema`, in any
//! order, or appended by `AddCustomAttributes` afterwards. The columns
//! themselves are the pool's STORED attributes, so the template and the schema
//! `DescribeUserPool` reports always describe the same attribute set; only the
//! sequence is the import format's.

use super::*;

/// Amazon Cognito's import template, as it lists a pool's STANDARD attributes
/// once that pool carries attributes of its own: the pool's custom columns sit
/// between the first [`IMPORT_TEMPLATE_COLUMNS_BEFORE_CUSTOM`] of these and the
/// rest, rather than behind them all.
///
/// Every name here is one of [`STANDARD_SCHEMA`]'s, and the minted
/// [`SUB_ATTRIBUTE`] is not among them — no import supplies it.
const IMPORT_TEMPLATE_STANDARD_COLUMNS: [&str; 19] = [
    "profile",
    "address",
    "birthdate",
    "gender",
    "preferred_username",
    "updated_at",
    "website",
    "picture",
    "email_verified",
    "phone_number_verified",
    "phone_number",
    "zoneinfo",
    // The pool's own (custom) columns go here.
    "locale",
    "email",
    "given_name",
    "family_name",
    "middle_name",
    "name",
    "nickname",
];

/// How many of [`IMPORT_TEMPLATE_STANDARD_COLUMNS`] the template lists AHEAD of
/// the pool's custom columns, which is therefore also the rank the custom
/// columns themselves take.
const IMPORT_TEMPLATE_COLUMNS_BEFORE_CUSTOM: usize = 12;

/// The CSV header of a pool whose stored schema is `schema`: one column per
/// attribute of the pool a user import can carry, in the import template's
/// order, then the columns the import format itself adds
/// ([`CSV_HEADER_TRAILING_COLUMNS`]).
///
/// The attributes the pool mints or maintains for itself carry no column,
/// because no import supplies them: the minted [`SUB_ATTRIBUTE`] and the
/// service-maintained [`IDENTITIES_ATTRIBUTE`]. Everything else the pool's
/// schema declares has a column, so a schema that grew a custom attribute
/// reports one for it.
///
/// The sequence comes from a TABLE of the service's own rather than from the
/// stored schema, so two pools holding the same attributes are handed the same
/// template whichever way each was configured: a pool carrying custom
/// attributes is served [`IMPORT_TEMPLATE_STANDARD_COLUMNS`] with its custom
/// columns in the template's slot for them, and a pool carrying none is served
/// its standard attributes in the claim order the default schema declares them
/// in ([`STANDARD_SCHEMA`]). A column neither table ranks keeps its stored
/// position behind the ranked ones instead of being dropped.
pub fn csv_header_columns(schema: &[SchemaAttributeType]) -> Vec<String> {
    let mut columns: Vec<String> = schema
        .iter()
        .filter_map(|attribute| attribute.name.clone())
        .filter(|name| name != SUB_ATTRIBUTE && name != IDENTITIES_ATTRIBUTE)
        .collect();
    let carries_custom_columns = columns
        .iter()
        .any(|name| !is_standard_attribute(name));
    // A stable sort by rank, so several custom columns — and any column no
    // table ranks — keep the order the pool stores them in.
    columns.sort_by_key(|name| csv_column_rank(name, carries_custom_columns));
    columns.extend(
        CSV_HEADER_TRAILING_COLUMNS
            .iter()
            .map(|column| (*column).to_string()),
    );
    columns
}

/// Where the template puts the column `name` of a pool that does
/// (`carries_custom_columns`) or does not carry custom attributes: a rank, with
/// every unranked column behind the ranked ones.
fn csv_column_rank(name: &str, carries_custom_columns: bool) -> usize {
    if !carries_custom_columns {
        return STANDARD_SCHEMA
            .iter()
            .position(|standard| standard.name == name)
            .unwrap_or(STANDARD_SCHEMA.len());
    }
    match IMPORT_TEMPLATE_STANDARD_COLUMNS
        .iter()
        .position(|column| *column == name)
    {
        // The standard columns the template lists behind the custom ones leave
        // the custom slot its own rank.
        Some(at) if at < IMPORT_TEMPLATE_COLUMNS_BEFORE_CUSTOM => at,
        Some(at) => at.saturating_add(1),
        // A column of the pool's own, in the template's slot for them.
        None => IMPORT_TEMPLATE_COLUMNS_BEFORE_CUSTOM,
    }
}
