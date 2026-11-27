// SPDX-License-Identifier: CC0-1.0

//! The grants a Data Catalog TABLE is created carrying, which no `GrantPermissions` call
//! made.
//!
//! Two of them, both made by the act of creating the table:
//!
//! * the principal that created it holds Super on it, with the grant option, so a listing
//!   of a table nobody has granted anything on still reports its creator;
//! * the default permissions its parent database hands the tables created in it
//!   (`Database$CreateTableDefaultPermissions` — "creates a set of default permissions on
//!   the table for principals. Used by Lake Formation") are held on the table by the
//!   principals that setting names.
//!
//! The database-level counterpart lives beside this module in `state.rs`
//! ([`super::default_database_grants`]), where the grant comes from the account's own data
//! lake settings.
//!
//! Like the database default, these grants are rendered per request from the Data Catalog
//! rather than written when the table is created, because this store never sees a table
//! being created: the Data Catalog is another service's state in the same shared engine
//! (see [`super::missing_grant_entity`] for that seam). Taking one away is the sibling
//! module `implicit_table_grants` ([`super::revoke_implicit_grants`]).

use super::*;

use crate::ops::types::TableResource;

/// What a table's creator holds on it: Lake Formation's `ALL` — "Super", the permission on
/// the resource as a whole — together with every individual permission a table takes.
///
/// The members come from the modelled `Permission` enum
/// (`smithy_query(query="shape",shape="Permission")`), narrowed to the values the published
/// Lake Formation permissions reference lists for a TABLE resource — `SELECT`, `ALTER`,
/// `DROP`, `DELETE`, `INSERT`, `DESCRIBE` — in that enum's own declaration order with `ALL`
/// first. Every other value of the enum names a different scope: a catalog, a database, an
/// LF-tag or a data location (`CREATE_CATALOG`, `CREATE_DATABASE`, `CREATE_TABLE`,
/// `CREATE_LF_TAG`, `CREATE_LF_TAG_EXPRESSION`, `DATA_LOCATION_ACCESS`, `ASSOCIATE`,
/// `GRANT_WITH_LF_TAG_EXPRESSION`, `SUPER_USER`).
///
/// The set is split across the two scopes Lake Formation holds table permissions at by
/// [`super::scoped_grant_records`], so the column-scoped part
/// ([`super::COLUMN_SCOPED_TABLE_PERMISSIONS`]) is reported as the table's column-wildcard
/// record and the rest — `ALL` among it — as the table's own.
pub const CREATOR_TABLE_PERMISSIONS: [&str; 7] = [
    "ALL", "SELECT", "ALTER", "DROP", "DELETE", "INSERT", "DESCRIBE",
];

/// The most grants one listing renders for the tables it addresses.
///
/// What a rendering costs is the number of addressed tables TIMES what each one carries:
/// its creator's grant plus one per entry of its database's
/// `CreateTableDefaultPermissions`. Both factors come from stored state — the Data
/// Catalog's own per-catalog database and per-database table ceilings bound the first, and
/// the second is a list a `CreateDatabase` supplied, which the model gives no length
/// constraint — so the PRODUCT needs a ceiling of its own rather than one per factor. This
/// is it: the walk stops here, so a single unfiltered listing can neither allocate nor work
/// without end.
///
/// Emulator-chosen, and the size of the explicit grant store's own ceiling
/// ([`MAX_PERMISSION_GRANTS`]): the two together bound everything one listing can report,
/// and both are far above the [`MAX_PERMISSIONS_PAGE`] entries a single page carries.
pub const MAX_IMPLICIT_TABLE_GRANTS: usize = 10_000;

/// The grants the tables a listing in `catalog_id` addresses were created carrying, keyed
/// by [`grant_key`] so each one merges with the explicit grant on the same resource.
///
/// Two grants per table, and both are the act of creating it:
///
/// * its CREATOR holds [`CREATOR_TABLE_PERMISSIONS`] with the grant option, at the two
///   scopes a table's permissions are held at — so a listing of the table reports both the
///   table entry and the column-wildcard entry ("call this operation on the table, and the
///   operation returns the table and the table w columns" — `ListPermissions$Resource`).
///   Who created it is the Data Catalog's own record (`Table$CreatedBy`, "the person or
///   entity who created the table"), never derived from the request: a table the catalog
///   records no creator for carries no creator grant.
/// * each principal the parent database's `CreateTableDefaultPermissions` names holds what
///   that setting grants it, without the grant option — a database that hands its tables
///   nothing leaves the table to its creator alone.
///
/// Both are rendered at the scope each permission applies at ([`scoped_grant_records`]),
/// the same split the explicit grant path writes, and two grants that land on one key are
/// reported as one entry carrying both.
///
/// Which tables: the one a `Table` or `TableWithColumns` resource filter names, or every
/// table of the catalog when the listing filters on no resource at all. A filter naming any
/// other resource — including a `TableWildcard`, which is its own resource rather than the
/// tables the catalog holds — selects no table-level grant. What this renders is bounded by
/// [`MAX_IMPLICIT_TABLE_GRANTS`], which the walk stops at.
///
/// The tables belong to GLUE, so they are read from Glue's own store in the shared engine.
/// Call this BEFORE locking this service's store: it takes Glue's lock itself, and never
/// holds the two at the same time.
///
/// `resource_filter`, when present, must already be [`normalize_resource`]d, so the names
/// are the ones the catalog stores and the catalog id is filled in — that is what makes the
/// keys here the ones a revoke records a revoked grant under.
pub fn created_table_grants<Eng: Send + Sync + 'static>(
    engine: &Eng,
    catalog_id: &str,
    resource_filter: Option<&Resource>,
) -> Vec<(String, PermissionGrant)> {
    // The catalog, database and table the filter narrows to, with an absent level meaning
    // "every one of them".
    let (table_catalog, named_database, named_table) = match resource_filter {
        None => (catalog_id.to_string(), None, None),
        Some(resource) => {
            if let Some(table) = &resource.table {
                // A `TableWildcard` filter is its OWN resource ("every table under a
                // database"), not the tables the catalog holds — a grant on one of those
                // is a different resource, which no such filter selects — so nothing is
                // rendered for it.
                let Some(name) = table.name.clone().filter(|name| !name.is_empty()) else {
                    return Vec::new();
                };
                (
                    resource_catalog(table.catalog_id.as_deref(), catalog_id),
                    Some(table.database_name.clone()),
                    Some(name),
                )
            } else if let Some(table) = &resource.table_with_columns {
                (
                    resource_catalog(table.catalog_id.as_deref(), catalog_id),
                    Some(table.database_name.clone()),
                    Some(table.name.clone()),
                )
            } else {
                return Vec::new();
            }
        }
    };

    // Every addressed database with its addressed tables: the default permissions the
    // database hands the tables created in it, read ONCE per database, and each table's
    // own name and the creator the catalog recorded for it. Collected under Glue's lock and
    // used after it is dropped, and stopped at [`MAX_IMPLICIT_TABLE_GRANTS`] so neither the
    // collection nor the rendering below grows with the product of the two counts.
    let Some(catalog) = glue_state::service_state(engine) else {
        return Vec::new();
    };
    let created: Vec<CreatedDatabase> = {
        let catalog = catalog.lock();
        let Some(held) = catalog.catalogs.get(&table_catalog) else {
            return Vec::new();
        };
        let databases: Vec<&glue_state::DatabaseEntry> = match &named_database {
            Some(name) => match held.databases.get(&glue_state::fold_name(name)) {
                Some(database) => vec![database],
                None => return Vec::new(),
            },
            None => held.databases.values().collect(),
        };
        let mut created: Vec<CreatedDatabase> = Vec::new();
        // What the collected tables and defaults can render, kept under the ceiling as the
        // walk goes: one grant per table for its creator, plus one per (table, default
        // entry) pair.
        let mut renders = 0_usize;
        for database in databases {
            if renders >= MAX_IMPLICIT_TABLE_GRANTS {
                break;
            }
            let tables: Vec<&glue_state::TableEntry> = match &named_table {
                Some(name) => match database.tables.get(&glue_state::fold_name(name)) {
                    Some(table) => vec![table],
                    None => continue,
                },
                None => database.tables.values().collect(),
            };
            if tables.is_empty() {
                continue;
            }
            // Read apart here rather than carried as the Data Catalog's own
            // `PrincipalPermissions` shape: that is Glue's version of the structure, a
            // different type from this service's. Once per database, because that is what
            // the setting belongs to — the tables created in it share one value.
            let defaults: Vec<(String, Vec<String>)> = database
                .create_table_default_permissions
                .iter()
                .filter_map(|entry| {
                    let principal = entry
                        .principal
                        .as_ref()?
                        .data_lake_principal_identifier
                        .as_deref()
                        .filter(|principal| !principal.is_empty())?;
                    let permissions = dedup_values(entry.permissions.iter().flatten());
                    (!permissions.is_empty()).then(|| (principal.to_string(), permissions))
                })
                .collect();

            // Only as many tables as the ceiling still leaves room to render.
            let per_table = 1 + defaults.len();
            let room = (MAX_IMPLICIT_TABLE_GRANTS - renders) / per_table;
            if room == 0 {
                break;
            }
            let tables: Vec<CreatedTable> = tables
                .into_iter()
                .take(room)
                .map(|table| CreatedTable {
                    name: table.name.clone(),
                    creator: table
                        .created_by
                        .clone()
                        .filter(|creator| !creator.is_empty()),
                    created_at: table.create_time,
                })
                .collect();
            renders += tables.len() * per_table;
            created.push(CreatedDatabase {
                database_name: database.name.clone(),
                defaults,
                tables,
            });
        }
        created
    };

    let creator_permissions: Vec<String> = CREATOR_TABLE_PERMISSIONS
        .iter()
        .map(|permission| (*permission).to_string())
        .collect();
    // Keyed, so a principal that is both the creator and named by the database's defaults
    // is ONE entry carrying both — a listing reports one entry per (principal, resource,
    // condition) triple.
    let mut grants: BTreeMap<String, PermissionGrant> = BTreeMap::new();
    for database in created {
        for table in database.tables {
            // The table as the catalog holds it: the stored (folded) names and a definite
            // catalog id, which is what [`normalize_resource`] would leave it as.
            let resource = Resource {
                table: Some(TableResource {
                    catalog_id: Some(table_catalog.clone()),
                    database_name: database.database_name.clone(),
                    name: Some(table.name),
                    table_wildcard: None,
                }),
                ..Resource::default()
            };

            // The creator holds Super, so this split yields both the table's own record
            // and its column-wildcard one. The write that made the grant is the CREATE of
            // the table, so that is the provenance a listing reports for it: the instant
            // the catalog records for the table and the principal it records as having
            // created it, else the identity this emulator's credential speaks for, which is
            // the one every call into it is made under.
            let provenance = GrantProvenance {
                at: table.created_at,
                by: table.creator.clone().unwrap_or_else(
                    ::service_common::caller_principal::ambient_user_arn,
                ),
            };
            if let Some(creator) = &table.creator {
                record_implicit_grant(
                    &mut grants,
                    catalog_id,
                    creator,
                    &resource,
                    &creator_permissions,
                    &creator_permissions,
                    &provenance,
                );
            }

            // What the database hands the tables created in it, held by the principals it
            // names and passable to nobody: a default carries no grant option.
            for (principal, permissions) in &database.defaults {
                record_implicit_grant(
                    &mut grants,
                    catalog_id,
                    principal,
                    &resource,
                    permissions,
                    &[],
                    &provenance,
                );
            }
        }
    }
    grants.into_iter().collect()
}

/// One addressed database, as the Data Catalog holds it: what the grants above are rendered
/// from.
#[derive(Debug)]
struct CreatedDatabase {
    /// The database's stored (folded) name.
    database_name: String,
    /// Its `CreateTableDefaultPermissions` as the catalog holds them, read once for the
    /// whole database: one (principal, permissions) pair per entry that grants something.
    defaults: Vec<(String, Vec<String>)>,
    /// Its addressed tables.
    tables: Vec<CreatedTable>,
}

/// One addressed table, as the Data Catalog holds it.
#[derive(Debug, Clone)]
struct CreatedTable {
    /// Its stored (folded) name.
    name: String,
    /// Its `Table$CreatedBy`, when the catalog recorded one.
    creator: Option<String>,
    /// Its `Table$CreateTime` — the instant of the write that made these grants.
    created_at: SystemTime,
}

/// Adds the grant of `permissions` to `principal` on `resource` to `grants`, split across
/// the scopes each permission applies at and merged with whatever is already keyed there,
/// each record carrying the provenance of the write that made it.
fn record_implicit_grant(
    grants: &mut BTreeMap<String, PermissionGrant>,
    catalog_id: &str,
    principal: &str,
    resource: &Resource,
    permissions: &[String],
    permissions_with_grant_option: &[String],
    provenance: &GrantProvenance,
) {
    for (scoped_resource, scoped_permissions, scoped_with_grant_option) in
        scoped_grant_records(resource, permissions, permissions_with_grant_option)
    {
        let mut grant = PermissionGrant {
            catalog_id: catalog_id.to_string(),
            principal: principal.to_string(),
            resource: scoped_resource,
            permissions: scoped_permissions,
            permissions_with_grant_option: scoped_with_grant_option,
            // A grant a resource was created carrying never has a condition.
            condition_expression: None,
            last_updated: None,
            last_updated_by: None,
        };
        grant.written_by(provenance);
        let key = grant_key(catalog_id, &grant.principal, &grant.resource, None);
        match grants.get_mut(&key) {
            Some(held) => {
                merge_permissions(&mut held.permissions, &grant.permissions);
                merge_permissions(
                    &mut held.permissions_with_grant_option,
                    &grant.permissions_with_grant_option,
                );
                held.absorb_provenance(&grant);
            }
            None => {
                grants.insert(key, grant);
            }
        }
    }
}

