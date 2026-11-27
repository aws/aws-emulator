// SPDX-License-Identifier: CC0-1.0

//! WHICH permissions a kind of Lake Formation resource can hold.
//!
//! `Permission` is ONE vocabulary (`smithy_query(query="shape",shape="Permission")`) shared
//! by every member of `Resource` (`AnyResource`, `Catalog`, `Database`, `Table`,
//! `TableWithColumns`, `DataLocation`, `DataCellsFilter`, `LFTag`, `LFTagPolicy`,
//! `LFTagExpression`, `RLSPolicy`, `UserDefinedFunction`), and each of those members names
//! a DIFFERENT kind of resource — so only a
//! subset of that vocabulary means anything on any one kind. A permission names a privilege
//! over a thing that kind of resource does not have (creating a table under a table,
//! reading rows of a database, reaching a data location that is not one) is not a
//! modification Lake Formation can record, and the grant is refused whole, with nothing
//! persisted, rather than recorded as a privilege no read can honour.
//!
//! The model states one of these restrictions outright — `TableWithColumnsResource` "is
//! only used when granting a SELECT permission"
//! (`smithy_query(query="doc",shape="TableWithColumnsResource")`), which is
//! [`super::COLUMN_SCOPED_TABLE_PERMISSIONS`], the same set a table grant's column-scoped
//! record is split out by — and documents one more as a relation between the two permission
//! lists: `PermissionsWithGrantOption` "may only be a subset of the permissions granted in
//! the Privileges" (`smithy_query(query="doc",shape="GrantPermissionsRequest",
//! member="PermissionsWithGrantOption")`), so a grant that hands out the grant option for a
//! permission it does not grant is the same class of invalid modification.
//!
//! The remaining sets are the privileges Lake Formation defines for each kind of resource:
//! a database holds the privileges over a database (its tables, its definition), a table
//! the privileges over a table's metadata and data, a data location the access to that
//! location, the Data Catalog itself the privileges to create what lives in it, and an
//! LF-tag or a tag expression the privileges over the tag. An LF-tag POLICY is a grant on
//! every resource its expression matches, so it holds the privileges of the resource kind
//! its required `ResourceType` names (`DATABASE` or `TABLE`).
//!
//! Both grant paths apply this: `GrantPermissions` and `BatchGrantPermissions` read a grant
//! through [`super::read_grant_request`] and plan it through [`super::plan_grant`], so what
//! one refuses the other refuses.

use super::*;

/// The privileges the Data Catalog itself holds: creating what lives in it, and the
/// catalog-wide roles.
///
/// Grounded in what each value names: `CREATE_DATABASE`, `CREATE_CATALOG`, `CREATE_LF_TAG`
/// and `CREATE_LF_TAG_EXPRESSION` each name a thing that is created IN a catalog rather
/// than in anything a catalog contains, and `SUPER_USER` is held over the catalog as a
/// whole.
///
/// `ALTER`, `DROP` and `DESCRIBE` are ADMITTED rather than grounded: a catalog is a
/// resource of its own, nothing this service knows says those three cannot be held on it,
/// and a refusal has to be grounded in what the service knows. Narrow this set here, and
/// only on evidence, if one of them is ever seen refused at catalog scope — the other sets
/// are what the refusal above is grounded in.
pub const CATALOG_PERMISSIONS: &[&str] = &[
    "ALL",
    "ALTER",
    "CREATE_CATALOG",
    "CREATE_DATABASE",
    "CREATE_LF_TAG",
    "CREATE_LF_TAG_EXPRESSION",
    "DESCRIBE",
    "DROP",
    "SUPER_USER",
];

/// The privileges a DATABASE holds: creating a table in it, and altering, dropping and
/// describing the database itself.
///
/// The privileges over a table's own metadata and data (`SELECT`, `INSERT`, `DELETE`) are
/// not among them: a database holds no rows, so they are granted on a table (or on every
/// table of the database, through a `TableWildcard`).
pub const DATABASE_PERMISSIONS: &[&str] = &["ALL", "ALTER", "CREATE_TABLE", "DESCRIBE", "DROP"];

/// The privileges a TABLE holds: reading and writing its data, and altering, dropping and
/// describing the table itself.
///
/// `CREATE_TABLE` and `CREATE_DATABASE` are not among them — nothing is created under a
/// table — and neither is `DATA_LOCATION_ACCESS`, which is held on a registered location.
pub const TABLE_PERMISSIONS: &[&str] = &[
    "ALL", "ALTER", "DELETE", "DESCRIBE", "DROP", "INSERT", "SELECT",
];

/// The one privilege a registered data location holds: access to it.
pub const DATA_LOCATION_PERMISSIONS: &[&str] = &["ALL", "DATA_LOCATION_ACCESS"];

/// The privileges an LF-TAG holds: attaching it to a resource, and altering, dropping and
/// describing the tag itself.
pub const LF_TAG_PERMISSIONS: &[&str] = &["ALL", "ALTER", "ASSOCIATE", "DESCRIBE", "DROP"];

/// The privileges a saved LF-Tag EXPRESSION holds: granting with it, beside the privileges
/// over the expression itself.
pub const LF_TAG_EXPRESSION_PERMISSIONS: &[&str] = &[
    "ALL",
    "ALTER",
    "ASSOCIATE",
    "DESCRIBE",
    "DROP",
    "GRANT_WITH_LF_TAG_EXPRESSION",
];

/// The privileges a data cells filter or a row-level security policy holds: reading the
/// cells or rows it admits, and altering, dropping and describing the filter or policy.
///
/// Both are a narrowing OF a table, so what they hold is the reading privilege plus the
/// privileges over the narrowing itself.
pub const FILTERED_TABLE_PERMISSIONS: &[&str] =
    &["ALL", "ALTER", "DESCRIBE", "DROP", "SELECT"];

/// The privileges a USER-DEFINED FUNCTION holds, which the model states outright: "you can
/// grant the following permissions: EXECUTE – Use the function in a job, and get
/// credentials to download its artifacts; DESCRIBE – View the function's definition; ALTER
/// – Update the function; DROP – Delete the function; ALL – All of the preceding
/// permissions." (`smithy_query(query="doc",shape="Resource",member="UserDefinedFunction")`).
pub const USER_DEFINED_FUNCTION_PERMISSIONS: &[&str] =
    &["ALL", "ALTER", "DESCRIBE", "DROP", "EXECUTE"];

/// The permissions the kind of resource `resource` names can hold, or `None` when it names
/// no kind at all (which [`super::validate_resource`] refuses first).
///
/// The order the members are weighed in is [`super::validate_resource`]'s, which has
/// already refused a `Resource` naming more than one.
pub fn permissions_of_resource_kind(resource: &Resource) -> Option<&'static [&'static str]> {
    if let Some(any) = &resource.any_resource {
        // "All resources of the specified type in a Data Catalog", so it holds what that
        // type holds. `ResourceType` is required and "the only supported value is TABLE"
        // (`AnyResourceSupportedTypes`), which the request boundary holds the member to —
        // so the other arm answers "nothing is known about this kind" rather than a
        // privilege set this service invented for a value the model does not carry.
        match any.resource_type.as_str() {
            "TABLE" => Some(TABLE_PERMISSIONS),
            _ => None,
        }
    } else if resource.catalog.is_some() {
        Some(CATALOG_PERMISSIONS)
    } else if resource.database.is_some() {
        Some(DATABASE_PERMISSIONS)
    } else if resource.table.is_some() {
        Some(TABLE_PERMISSIONS)
    } else if resource.table_with_columns.is_some() {
        // The model's own restriction: a table with columns is "only used when granting a
        // SELECT permission".
        Some(&COLUMN_SCOPED_TABLE_PERMISSIONS)
    } else if resource.data_location.is_some() {
        Some(DATA_LOCATION_PERMISSIONS)
    } else if resource.data_cells_filter.is_some() || resource.rls_policy.is_some() {
        Some(FILTERED_TABLE_PERMISSIONS)
    } else if resource.lf_tag.is_some() {
        Some(LF_TAG_PERMISSIONS)
    } else if let Some(policy) = &resource.lf_tag_policy {
        // A tag policy grants on every resource its expression matches, so it holds what
        // the resource kind it names holds. `ResourceType` is required and its values are
        // DATABASE and TABLE (`smithy_query(query="shape",shape="ResourceType")`), which
        // the request boundary holds the member to (`RESOURCE_TYPE_VALUES`; the grant
        // inputs skip validation of the permission LISTS only) — so the arm below is
        // unreachable, and answers "nothing is known about this kind" rather than a
        // privilege set this service invented for a value the model does not carry.
        match policy.resource_type.as_str() {
            "DATABASE" => Some(DATABASE_PERMISSIONS),
            "TABLE" => Some(TABLE_PERMISSIONS),
            _ => None,
        }
    } else if resource.lf_tag_expression.is_some() {
        Some(LF_TAG_EXPRESSION_PERMISSIONS)
    } else if resource.user_defined_function.is_some() {
        Some(USER_DEFINED_FUNCTION_PERMISSIONS)
    } else {
        None
    }
}

/// Why the permissions a grant names are not a modification Lake Formation can make on the
/// resource it names — `None` when every permission it names is one that kind of resource
/// holds.
///
/// Both permission lists are weighed: the grant option is held at the same scope as the
/// permission it accompanies, so a permission that cannot be held on the resource cannot be
/// handed out with the grant option there either.
///
/// A resource kind this service knows no privilege set for (nothing that
/// [`permissions_of_resource_kind`] answers for) is left alone: a refusal has to be
/// grounded in what the service knows, never assumed.
pub fn inapplicable_permissions(
    resource: &Resource,
    permissions: &[String],
    permissions_with_grant_option: &[String],
) -> Option<String> {
    let applicable = permissions_of_resource_kind(resource)?;
    let holds = |permission: &String| applicable.contains(&permission.as_str());
    (!permissions.iter().all(holds) || !permissions_with_grant_option.iter().all(holds))
        .then(|| PERMISSIONS_MODIFICATION_INVALID.to_string())
}

/// Why the two permission lists of a grant do not stand in the relation the model
/// documents — `None` when `PermissionsWithGrantOption` is a subset of `Permissions`.
///
/// `PermissionsWithGrantOption` is "a list of the granted permissions that the principal
/// may pass to other users. These permissions may only be a subset of the permissions
/// granted in the Privileges."
/// (`smithy_query(query="doc",shape="GrantPermissionsRequest",
/// member="PermissionsWithGrantOption")`), so passing on a permission that is not granted
/// at all is not a modification Lake Formation can make — including the grant that names
/// the grant option ALONE.
///
/// This holds of GRANTING only. A revoke's `PermissionsWithGrantOption` is "a list of
/// permissions for which to revoke the grant option allowing the principal to pass
/// permissions to other principals"
/// (`smithy_query(query="doc",shape="RevokePermissionsRequest",
/// member="PermissionsWithGrantOption")`): taking back only the grant option, and leaving
/// the permission itself granted, is what that list is for, so the two lists stand in no
/// subset relation there.
pub fn grant_option_outside_permissions(
    permissions: &[String],
    permissions_with_grant_option: &[String],
) -> Option<String> {
    let granted: BTreeSet<&str> = permissions.iter().map(String::as_str).collect();
    permissions_with_grant_option
        .iter()
        .any(|permission| !granted.contains(permission.as_str()))
        .then(|| PERMISSIONS_MODIFICATION_INVALID.to_string())
}
