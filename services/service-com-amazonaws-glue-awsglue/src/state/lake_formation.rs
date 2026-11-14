// SPDX-License-Identifier: CC0-1.0

//! Whether the caller holds the Lake Formation permission a Data Catalog METADATA READ
//! needs.
//!
//! A Data Catalog resource is governed by Lake Formation: a principal may read a
//! database's metadata only while it holds at least one Lake Formation permission on that
//! database — in its own name, through the `IAM_ALLOWED_PRINCIPALS` group every IAM user
//! and role belongs to, or on a resource INSIDE it, since a grant on a table makes the
//! table's parent database describable. A TABLE's metadata needs a permission on that table
//! itself — on the table, on named columns of it, on a filter over its cells, or on every
//! table of its database through a table wildcard — because a database's own permissions are
//! granted over the database resource. A data lake administrator holds every permission
//! of its catalog implicitly. A read by a principal none of those cover is refused.
//!
//! Lake Formation OWNS those grants, so they are read from Lake Formation rather than kept
//! a second time here: this module asks the Lake Formation service in this same emulator
//! over the framework's in-process callback, exactly as the catalog asks KMS about a key
//! ([`super::catalog_encryption`]). The permission state consulted is therefore the very
//! state `GrantPermissions`, `RevokePermissions` and `PutDataLakeSettings` wrote, and this
//! crate holds no copy of it that could drift. Lake Formation also renders the grants a
//! resource was CREATED carrying (a catalog's `CreateDatabaseDefaultPermissions`, a
//! database's `CreateTableDefaultPermissions`, a table creator's own), which is why they
//! are not computed here either.
//!
//! An emulator with no Lake Formation mounted beside this service, or one whose Lake
//! Formation could not answer, governs nothing that can be read: [`catalog_permissions`]
//! answers `None` there, and a reader treats that as "no decision", never as a refusal —
//! a refusal has to be grounded in what Lake Formation said.
//!
//! Nothing here writes: the question is put with reads only, and the answer is used to
//! decide whether a read is served. The call is made with NO guard on this service's own
//! store held, because Lake Formation reads the Data Catalog (the databases and tables a
//! grant names) out of that same store.

use std::collections::BTreeSet;
use std::time::SystemTime;

use ::aws_sdk_lakeformation::types::{DataLakePrincipal, Resource};
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

/// The most entries one `ListPermissions` page is asked for: `MaxResults` is modelled
/// `range 1..1000`, so this is that ceiling and the fewest pages a decision has to walk.
const PERMISSIONS_PAGE_SIZE: i32 = 1000;

/// The most `ListPermissions` pages one decision walks for one principal.
///
/// A loop over a service-supplied continuation token needs an end of its own. Lake
/// Formation bounds the records it holds (its own `MAX_PERMISSION_GRANTS` ceiling, plus
/// one rendered default per database and per table of the catalog), so this many pages of
/// [`PERMISSIONS_PAGE_SIZE`] is far past anything a store that size can answer: the cap is
/// what makes the walk terminate, not a limit a real listing reaches. A listing still
/// going at the cap is an INCOMPLETE answer, and [`catalog_permissions`] treats it like a
/// failed call — no read is refused on it.
const MAX_PERMISSION_PAGES: usize = 64;

/// What the caller holds in ONE Data Catalog, as far as reading metadata out of it goes.
///
/// Built by [`catalog_permissions`] from what Lake Formation answered, and asked by the
/// catalog's read paths. Deliberately NOT a copy of the grants: only the question those
/// paths put to them — which databases are describable — is kept.
#[derive(Debug, Clone, Default)]
pub struct CatalogPermissions {
    /// Whether the caller is an administrator of this catalog's data lake, who holds every
    /// permission on every one of its resources implicitly.
    admin: bool,
    /// The stored (folded) names of the databases the caller holds a Lake Formation
    /// permission on, either on the database itself or on a resource inside it.
    databases: BTreeSet<String>,
    /// The stored (folded) `(database, table)` pairs the caller holds a Lake Formation
    /// permission on — a grant on the table itself, on named columns of it, or on a filter
    /// over its cells.
    tables: BTreeSet<(String, String)>,
    /// The stored (folded) names of the databases the caller holds a permission on EVERY
    /// table of, which is what a grant naming a table wildcard confers.
    every_table_of: BTreeSet<String>,
}

impl CatalogPermissions {
    /// Whether the caller may read `name`'s metadata: it is a data lake administrator, or
    /// it holds a permission on that database or on a resource inside it.
    ///
    /// `name` is matched as the catalog STORES it ([`super::fold_name`]), so the spelling a
    /// request used cannot decide the answer.
    #[must_use]
    pub fn may_describe_database(&self, name: &str) -> bool {
        self.admin || self.databases.contains(&super::fold_name(name))
    }

    /// Whether the caller may read the metadata of the table `table` of the database
    /// `database`: it is a data lake administrator, or it holds a Lake Formation permission
    /// on that table — on the table itself, on named columns of it, on a filter over its
    /// cells, or on every table of its database through a table wildcard.
    ///
    /// A permission on the DATABASE is not one of them: Lake Formation grants a database's
    /// permissions over the database resource, and the metadata of a table inside it is
    /// answered to a principal that holds a permission on that table.
    ///
    /// Both names are matched as the catalog STORES them ([`super::fold_name`]), so the
    /// spelling a request used cannot decide the answer.
    #[must_use]
    pub fn may_describe_table(&self, database: &str, table: &str) -> bool {
        if self.admin {
            return true;
        }
        let database = super::fold_name(database);
        if self.every_table_of.contains(&database) {
            return true;
        }
        self.tables.contains(&(database, super::fold_name(table)))
    }
}

/// What the caller holds in the Data Catalog `catalog_id`, or `None` when Lake Formation
/// said nothing at all (none is mounted beside this service, or it could not answer) — in
/// which case nothing it governs has been read and no read may be refused on its behalf.
///
/// Call this with no guard on this service's store held: Lake Formation resolves the
/// databases and tables a grant names in the Data Catalog, which is that same store.
pub async fn catalog_permissions<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    catalog_id: &str,
) -> Option<CatalogPermissions> {
    let client = lake_formation_client(ctx, region);

    // The catalog's data lake settings, which name its administrators. This is also what
    // says whether Lake Formation is there to answer at all: every catalog HAS settings
    // ("by default, the account ID", and a catalog nothing configured reads back the
    // defaults), so a failure here is about the service and not about the catalog.
    let settings = match client
        .get_data_lake_settings()
        .catalog_id(catalog_id)
        .send()
        .await
    {
        Ok(answer) => answer.data_lake_settings,
        Err(error) => {
            ::tracing::debug!(
                catalog = %catalog_id,
                reason = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "Lake Formation answered nothing about this Data Catalog, so no catalog \
                 read is refused on its behalf"
            );
            return None;
        }
    };

    let principals = caller_data_lake_principals(engine, ctx);
    // "A list of Lake Formation principals" — an administrator of the data lake, and a
    // read-only administrator, hold every permission of the catalog implicitly, so a read
    // by one needs no grant of its own.
    let admin = settings.iter().any(|settings| {
        settings
            .data_lake_admins()
            .iter()
            .chain(settings.read_only_admins())
            .filter_map(DataLakePrincipal::data_lake_principal_identifier)
            .any(|admin| principals.iter().any(|held| held == admin))
    });

    let mut databases = BTreeSet::new();
    let mut tables = BTreeSet::new();
    let mut every_table_of = BTreeSet::new();
    for principal in &principals {
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let page = match client
                .list_permissions()
                .catalog_id(catalog_id)
                .principal(
                    DataLakePrincipal::builder()
                        .data_lake_principal_identifier(principal.clone())
                        .build(),
                )
                .max_results(PERMISSIONS_PAGE_SIZE)
                .set_next_token(next_token.take())
                .send()
                .await
            {
                Ok(page) => page,
                // Lake Formation stopped answering part way through: what it holds is
                // then unknown, and an unknown is not a refusal.
                Err(error) => {
                    ::tracing::debug!(
                        catalog = %catalog_id,
                        reason = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                        "Lake Formation did not answer which permissions this principal \
                         holds, so no catalog read is refused on its behalf"
                    );
                    return None;
                }
            };
            for entry in page.principal_resource_permissions() {
                // An entry carrying no permission at all grants nothing, whichever
                // resource it names.
                let holds = !entry.permissions().is_empty()
                    || !entry.permissions_with_grant_option().is_empty();
                if !holds {
                    continue;
                }
                if let Some(database) = granted_database(entry.resource(), catalog_id) {
                    databases.insert(database);
                }
                match granted_table(entry.resource(), catalog_id) {
                    Some(GrantedTable::Named { database, table }) => {
                        tables.insert((database, table));
                    }
                    Some(GrantedTable::EveryTableOf { database }) => {
                        every_table_of.insert(database);
                    }
                    None => {}
                }
            }
            next_token = page.next_token().map(str::to_string);
            if next_token.is_none() {
                break;
            }
            pages = pages.saturating_add(1);
            if pages >= MAX_PERMISSION_PAGES {
                // The listing is still going at the cap, so this walk has NOT seen
                // everything the principal holds — the same incomplete answer a failed
                // call is, and decided the same way: nothing is refused on it.
                ::tracing::debug!(
                    catalog = %catalog_id,
                    pages,
                    "Lake Formation listed more permission pages than one catalog read \
                     walks, so no catalog read is refused on its behalf"
                );
                return None;
            }
        }
    }

    Some(CatalogPermissions {
        admin,
        databases,
        tables,
        every_table_of,
    })
}

/// The database a grant on `resource` makes describable, as the catalog stores its name:
/// the database itself, or the parent of the table, the column-scoped table or the data
/// cells filter the grant names — a permission on a resource inside a database is what
/// makes that database visible to its holder.
///
/// `None` for a resource that is not in `catalog_id` (a grant in this catalog may name a
/// database of another one) and for every resource that is not a database or a thing
/// inside one: a catalog-level, data-location, LF-tag or tag-policy grant names no one
/// database.
fn granted_database(resource: Option<&Resource>, catalog_id: &str) -> Option<String> {
    let resource = resource?;
    if let Some(database) = resource.database() {
        return in_catalog(database.catalog_id(), catalog_id)
            .then(|| super::fold_name(database.name()));
    }
    if let Some(table) = resource.table() {
        return in_catalog(table.catalog_id(), catalog_id)
            .then(|| super::fold_name(table.database_name()));
    }
    if let Some(table) = resource.table_with_columns() {
        return in_catalog(table.catalog_id(), catalog_id)
            .then(|| super::fold_name(table.database_name()));
    }
    if let Some(filter) = resource.data_cells_filter() {
        let database = filter.database_name()?;
        return in_catalog(filter.table_catalog_id(), catalog_id)
            .then(|| super::fold_name(database));
    }
    None
}

/// Which TABLE a grant on `resource` makes readable: the table it names, or every table of
/// the database it names a table wildcard of.
///
/// `None` for a resource that is not in `catalog_id` and for every resource that is not a
/// table or a thing inside one — a grant on the database, the catalog, a data location, an
/// LF-tag or a tag policy names no one table.
fn granted_table(resource: Option<&Resource>, catalog_id: &str) -> Option<GrantedTable> {
    let resource = resource?;
    if let Some(table) = resource.table() {
        if !in_catalog(table.catalog_id(), catalog_id) {
            return None;
        }
        let database = super::fold_name(table.database_name());
        // "A wildcard object representing every table under a database": the grant covers
        // the tables the database holds now and the ones created in it later, so it is kept
        // as the database it covers rather than as the tables it happened to cover.
        if table.table_wildcard().is_some() {
            return Some(GrantedTable::EveryTableOf { database });
        }
        return table
            .name()
            .filter(|name| !name.is_empty())
            .map(|name| GrantedTable::Named {
                database,
                table: super::fold_name(name),
            });
    }
    if let Some(table) = resource.table_with_columns() {
        return in_catalog(table.catalog_id(), catalog_id).then(|| GrantedTable::Named {
            database: super::fold_name(table.database_name()),
            table: super::fold_name(table.name()),
        });
    }
    if let Some(filter) = resource.data_cells_filter() {
        let database = filter.database_name()?;
        let table = filter.table_name()?;
        return in_catalog(filter.table_catalog_id(), catalog_id).then(|| GrantedTable::Named {
            database: super::fold_name(database),
            table: super::fold_name(table),
        });
    }
    None
}

/// What a grant on one resource says about the TABLES of a catalog.
enum GrantedTable {
    /// One table, by name.
    Named { database: String, table: String },
    /// Every table of one database, which is what a table wildcard names.
    EveryTableOf { database: String },
}

/// Whether a resource member that names `named` sits in `catalog_id`: the catalog it names,
/// else the one the request addressed ("The identifier for the Data Catalog. By default,
/// the account ID.").
fn in_catalog(named: Option<&str>, catalog_id: &str) -> bool {
    named.filter(|id| !id.is_empty()).unwrap_or(catalog_id) == catalog_id
}

/// Every Lake Formation principal whose grants this request's caller holds: the identity
/// its credential speaks for, the IAM role behind an assumed-role session, and the
/// `IAM_ALLOWED_PRINCIPALS` group when one of those is an IAM user or role.
///
/// WHO the caller is comes from STS's own resolution of the access key id the request
/// signed with — the same record `sts:GetCallerIdentity` answers from — and the role behind
/// a session is the ARN the account's IAM holds that role under, which is the form a Lake
/// Formation grant names a role by. A credential that has expired speaks for nobody and so
/// holds nothing.
fn caller_data_lake_principals<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Vec<String> {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let iam = iam_store::service_state(engine);
    // A LONG-TERM access key speaks for the IAM user that holds it; a key IAM holds no
    // record of is this emulator's own published credential.
    let long_term = {
        let held = iam.as_ref().map(|state| state.lock());
        sts_store::long_term_identity(held.as_deref(), &access_key_id)
    };
    // The identity the credential speaks for, and the role of an assumed-role session.
    let resolved = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_request_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
            long_term.clone(),
        )
        .map(|caller| (caller.identity.arn, caller.role_arn)),
        // No STS store on this engine: the only identity a request can present is the
        // long-term one resolved above.
        None => Ok((long_term.arn.clone(), None)),
    };
    let Ok((identity_arn, session_role_arn)) = resolved else {
        // The session credential has expired, so it speaks for nobody and holds nothing.
        return Vec::new();
    };

    let mut principals = vec![identity_arn.clone()];
    // The permissions in force on an assumed-role session are the ROLE's, and a grant
    // names the role by the ARN the account holds it under — which carries its PATH,
    // where the session ARN does not.
    if let Some(role_arn) = session_role_arn {
        push_once(&mut principals, role_arn);
    }
    let session_role =
        ::service_common::iam_role::assumed_role_session_role_name(&identity_arn);
    if let Some(role_name) = session_role {
        if let Some(held) = iam.as_ref().and_then(|state| {
            state
                .lock()
                .roles
                .get(&iam_store::name_key(role_name))
                .map(|role| role.arn.clone())
        }) {
            push_once(&mut principals, held);
        }
    }
    // "IAM_ALLOWED_PRINCIPALS" stands for every principal IAM itself allows, so an IAM
    // user or role holds what that group holds.
    if principals.iter().any(|principal| is_iam_principal(principal)) {
        push_once(&mut principals, super::IAM_ALLOWED_PRINCIPALS.to_string());
    }
    principals
}

/// Append `value` unless `values` already carries it, so one identity asked about twice is
/// one listing.
fn push_once(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

/// Whether `identifier` names an IAM principal, i.e. an IAM user or an IAM role of this
/// account — the two entities Lake Formation documents as supported principals ("Supported
/// principals are IAM users or IAM roles"), and so the two the
/// `IAM_ALLOWED_PRINCIPALS` group covers.
///
/// The ARN reading itself is the tree's shared one ([`::service_common::iam_role`]), so
/// what counts as a role or a user ARN here is what every other service reads.
fn is_iam_principal(identifier: &str) -> bool {
    ::service_common::iam_role::role_name(identifier).is_some()
        || ::service_common::iam_role::user_name(identifier).is_some()
}

/// A Lake Formation client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service: it
/// routes in process, so no network client is configured here and no socket is opened.
/// `region` is the request's own signing region, so the permissions consulted are the ones
/// of the Region whose catalog is being read.
///
/// The transport is in-process, so there is nothing to retry, time out or watch for a
/// stalled stream, and the credentials the callback hands over are used directly.
fn lake_formation_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_lakeformation::Client {
    let config = ::aws_sdk_lakeformation::Config::builder()
        .behavior_version(::aws_sdk_lakeformation::config::BehaviorVersion::latest())
        .region(::aws_sdk_lakeformation::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lakeformation.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_lakeformation::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lakeformation::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lakeformation::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lakeformation::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lakeformation::Client::from_conf(config)
}
