// SPDX-License-Identifier: CC0-1.0

//! What a LISTING of permissions reports: the ORDER its entries come in.
//!
//! `ListPermissions` answers a PAGE of entries ("`MaxResults` — the maximum number of
//! results to return", `range 1..1000`, with a `NextToken` continuation), so which entries
//! a caller sees first is decided by the order the entries are put in. That order is a
//! function of what each entry CARRIES — the principal it is held by, then the resource it
//! is held on, then the condition it is scoped by ([`reported_grant_order_key`]) — and not
//! of how the record happens to be stored: [`super::grant_key`] renders a record's IDENTITY,
//! length-prefixed so no two resources can share a record, and those prefixes and
//! separators order records by the LENGTH of a principal identifier and put a table's
//! column-level entry ahead of the table's own, neither of which a caller can see or
//! predict.

use super::*;

/// The field separator the reporting order key renders.
///
/// A C0 control character, which is below every character the identifiers and names of
/// this API carry: the name and catalog-id shapes are patterned
/// `^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$`, whose lowest admitted
/// character is the tab (`\t`, `\u{9}`), and a principal is an ARN or one of the few
/// non-ARN forms [`is_non_arn_principal`] admits. So a field boundary always sorts before
/// any value a request supplies, which is what makes the key order field by field rather
/// than by the run-on text of the fields before it.
const ORDER_SEPARATOR: char = '\u{1}';

/// The resource a grant is held on, as the reporting order sorts it: the modelled
/// `Resource` member it is named by, then the names that address it, outermost first.
///
/// The member name first, so entries are grouped by the kind of resource they are on and a
/// table's own entry comes before that table's column-level one ("call this operation on
/// the table, and the operation returns the table and the table w columns" —
/// `ListPermissions$Resource`): `Table` is a prefix of `TableWithColumns`, and the
/// separator that follows it sorts below any character a member name can continue with.
///
/// The column SELECTION of a `TableWithColumns` resource, and an LF-tag's values, are not
/// part of this: they are sets rather than names, with no order of their own to report by.
/// Two entries that order alike are still distinct records — [`reported_grant_order_key`]
/// appends the record's identity.
pub fn resource_order_key(resource: &Resource) -> String {
    let (member, fields): (&str, Vec<&str>) = if let Some(catalog) = &resource.catalog {
        ("Catalog", vec![catalog.id.as_deref().unwrap_or_default()])
    } else if let Some(database) = &resource.database {
        (
            "Database",
            vec![
                database.catalog_id.as_deref().unwrap_or_default(),
                &database.name,
            ],
        )
    } else if let Some(table) = &resource.table {
        (
            "Table",
            vec![
                table.catalog_id.as_deref().unwrap_or_default(),
                &table.database_name,
                table.name.as_deref().unwrap_or_default(),
            ],
        )
    } else if let Some(table) = &resource.table_with_columns {
        (
            "TableWithColumns",
            vec![
                table.catalog_id.as_deref().unwrap_or_default(),
                &table.database_name,
                &table.name,
            ],
        )
    } else if let Some(location) = &resource.data_location {
        (
            "DataLocation",
            vec![
                location.catalog_id.as_deref().unwrap_or_default(),
                &location.resource_arn,
            ],
        )
    } else if let Some(filter) = &resource.data_cells_filter {
        (
            "DataCellsFilter",
            vec![
                filter.table_catalog_id.as_deref().unwrap_or_default(),
                filter.database_name.as_deref().unwrap_or_default(),
                filter.table_name.as_deref().unwrap_or_default(),
                filter.name.as_deref().unwrap_or_default(),
            ],
        )
    } else if let Some(tag) = &resource.lf_tag {
        (
            "LFTag",
            vec![tag.catalog_id.as_deref().unwrap_or_default(), &tag.tag_key],
        )
    } else if let Some(policy) = &resource.lf_tag_policy {
        (
            "LFTagPolicy",
            vec![
                policy.catalog_id.as_deref().unwrap_or_default(),
                &policy.resource_type,
                policy.expression_name.as_deref().unwrap_or_default(),
            ],
        )
    } else if let Some(expression) = &resource.lf_tag_expression {
        (
            "LFTagExpression",
            vec![
                expression.catalog_id.as_deref().unwrap_or_default(),
                &expression.name,
            ],
        )
    } else if let Some(policy) = &resource.rls_policy {
        (
            "RLSPolicy",
            vec![
                policy.catalog_id.as_deref().unwrap_or_default(),
                &policy.name,
            ],
        )
    } else if let Some(function) = &resource.user_defined_function {
        (
            "UserDefinedFunction",
            vec![
                function.catalog_id.as_deref().unwrap_or_default(),
                &function.database_name,
                function.name.as_deref().unwrap_or_default(),
            ],
        )
    } else if let Some(any) = &resource.any_resource {
        (
            "AnyResource",
            vec![
                any.catalog_id.as_deref().unwrap_or_default(),
                &any.resource_type,
            ],
        )
    } else {
        // Unreachable through a handler: `validate_resource` refuses a `Resource` naming
        // no member before a grant or a filter is keyed.
        ("", Vec::new())
    };
    let mut key = String::from(member);
    for field in fields {
        key.push(ORDER_SEPARATOR);
        key.push_str(field);
    }
    key
}

/// Where one entry falls in the order a listing reports its entries in — and so which page
/// of a `MaxResults`-bounded listing carries it.
///
/// The values the entry itself reports, in the order a caller reads them: the principal the
/// permissions are held by, then the resource they are held on ([`resource_order_key`]),
/// then the condition they are scoped by. Each is compared as the member carries it, so the
/// order is one a caller can see and predict from the answer.
///
/// The record's own identity ([`grant_key`]) is appended, so the key stays INJECTIVE — two
/// entries that order alike never collapse into one, and a continuation token, which names
/// the key the previous page stopped on, names exactly one entry.
pub fn reported_grant_order_key(
    catalog_id: &str,
    principal: &str,
    resource: &Resource,
    condition: Option<&str>,
) -> String {
    let mut key = String::new();
    key.push_str(principal);
    key.push(ORDER_SEPARATOR);
    key.push_str(&resource_order_key(resource));
    key.push(ORDER_SEPARATOR);
    key.push_str(condition.unwrap_or_default());
    key.push(ORDER_SEPARATOR);
    key.push_str(&grant_key(catalog_id, principal, resource, condition));
    key
}

/// Adds `grant` to the listing being built in `reported`, under the principal it is
/// REPORTED to be held by: `reported_as` when the listing resolves a group grant onto the
/// principal it asked about, else the grantee the record names.
///
/// Keyed by [`reported_grant_order_key`], so the listing walks its entries in the order it
/// reports them and a page is cut from them in that order.
///
/// Two grants reported under one identity — a principal's own record and a group grant
/// resolved onto it — are ONE entry carrying the union of both, because a listing reports
/// one entry per (principal, resource, condition) triple and re-attributing a group grant
/// must not open a second entry for a triple that already has one. That union is reported in
/// the order the `Permission` enum declares its values ([`reported_permissions`]): the
/// records were granted independently of each other, so the order either of them was granted
/// in says nothing about the set they together hold.
pub fn merge_reported_grant(
    reported: &mut BTreeMap<String, PermissionGrant>,
    grant: &PermissionGrant,
    reported_as: Option<&str>,
) {
    let mut grant = grant.clone();
    if let Some(principal) = reported_as {
        grant.principal = principal.to_string();
    }
    let key = reported_grant_order_key(
        &grant.catalog_id,
        &grant.principal,
        &grant.resource,
        grant.condition_expression.as_deref(),
    );
    match reported.get_mut(&key) {
        Some(held) => {
            merge_permissions(&mut held.permissions, &grant.permissions);
            merge_permissions(
                &mut held.permissions_with_grant_option,
                &grant.permissions_with_grant_option,
            );
            held.permissions = reported_permissions(&held.permissions);
            held.permissions_with_grant_option =
                reported_permissions(&held.permissions_with_grant_option);
            // The entry reports the provenance of the most recent write any of the records
            // it reports carries, that being when what the entry reports last changed.
            held.absorb_provenance(&grant);
        }
        None => {
            reported.insert(key, grant);
        }
    }
}
