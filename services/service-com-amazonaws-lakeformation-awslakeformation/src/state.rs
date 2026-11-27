// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lakeformation-awslakeformation service.
//!
//! Two stores live here: the LF-tag definitions (`lf_tags`) and the Lake Formation
//! permission records (`permissions`) that `GrantPermissions` writes and
//! `ListPermissions` reads back.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use crate::ops::types::{
    AllRowsWildcard, ColumnWildcard, Condition, DataCellsFilter, DataLakePrincipal,
    DataLakeSettings, DatabaseResource, LFTag, PrincipalPermissions, PrincipalResourcePermissions,
    Resource, RowFilter, TableResource, TableWithColumnsResource,
};

/// The Glue Data Catalog's own store, in the same shared engine.
///
/// Lake Formation grants permissions ON Data Catalog resources, which Glue owns, so the
/// databases and tables a grant may name are read from Glue's store rather than kept a
/// second time here — the same seam AmplifyBackend's `ListS3Buckets` uses to read S3's
/// buckets. See [`missing_grant_entity`].
use ::service_com_amazonaws_glue_awsglue::state as glue_state;

/// What a Data Catalog TABLE is created carrying — the grants its creation makes, beside
/// the setting-driven [`default_database_grants`] — and how one of them is taken away.
/// Re-exported, so a handler reaches both under `crate::state::` like every other part of
/// this store.
mod creator_table_grants;
/// One grant, from the request that asks for it to the records it writes: the path
/// `GrantPermissions` and `BatchGrantPermissions` share.
mod grant_requests;
mod implicit_table_grants;
/// WHICH permissions each kind of Lake Formation resource can hold, and the refusal a
/// grant naming one it cannot draws.
mod permission_scopes;
/// What identifies a Lake Formation principal, and the refusal a value that identifies
/// none draws.
mod principals;
/// How this service words its refusals: the message of each error class, and its own
/// rendering of a refusal the framework raised before a handler ran.
mod refusals;
/// What a LISTING reports: the order it reports its entries in, and the audit each
/// record carries.
mod reported_grants;
/// One revoke, from the request that asks for it to the records it trims: the path
/// `RevokePermissions` and `BatchRevokePermissions` share.
mod revoke_requests;
pub use creator_table_grants::*;
pub use grant_requests::*;
pub use implicit_table_grants::*;
pub use permission_scopes::*;
pub use principals::*;
pub use refusals::*;
pub use reported_grants::*;
pub use revoke_requests::*;

/// One registered data-lake location: the resource ARN Lake Formation manages, the IAM
/// role it uses to reach it, and the registration flags the caller set.
///
/// `role_arn` is stored as a plain `String` because a registration always HAS a data
/// access role — `RegisterResource` refuses a request that names neither `RoleArn` nor
/// `UseServiceLinkedRole` ("If UseServiceLinkedRole is not set to true, you must provide
/// or set the RoleArn" — `RegisterResource`'s own documentation), and
/// `UseServiceLinkedRole` resolves to [`service_linked_role_arn`].
///
/// A registration RESOLVES two of its members rather than merely echoing them, because
/// Lake Formation reports both for every registration: `hybrid_access_enabled` takes the
/// registration default when the request names no value, and
/// `expected_resource_owner_account` falls back to the caller's own account ("The Amazon
/// Web Services account that owns the Glue tables associated with specific Amazon S3
/// locations" — `RegisterResource$ExpectedResourceOwnerAccount`). `with_federation` keeps
/// the `Option` the model gives it (`NullableBoolean`), so a flag no registration set stays
/// absent rather than invented.
#[derive(Debug, Clone)]
pub struct RegisteredResource {
    /// The ARN this registration is keyed by.
    pub resource_arn: String,
    /// The IAM role that registered the resource (`ResourceInfo$RoleArn`).
    pub role_arn: String,
    /// `ResourceInfo$WithFederation`, as set by the last register/update.
    pub with_federation: Option<bool>,
    /// `ResourceInfo$HybridAccessEnabled`: whether "the data access of tables pointing to
    /// the location can be managed by both Lake Formation permissions as well as Amazon S3
    /// bucket policies". A registration always HAS a value — hybrid access is off unless
    /// the registration turned it on — so this is a plain `bool` rather than an `Option`.
    pub hybrid_access_enabled: bool,
    /// `ResourceInfo$WithPrivilegedAccess`. `RegisterResource`'s input member is
    /// modelled `Boolean` with `default false`, so a registration always has a value.
    pub with_privileged_access: Option<bool>,
    /// `ResourceInfo$ExpectedResourceOwnerAccount`: the account expected to own the
    /// resource — the one the request named, else the caller's own account.
    pub expected_resource_owner_account: String,
    /// When this registration was last MODIFIED, or `None` for one nothing has modified
    /// since it was registered.
    ///
    /// `ResourceInfo$LastModified` is "the date and time the resource was last modified",
    /// so a registration no call has changed has no such instant and the member is
    /// reported absent — which is what a describe of a freshly registered location
    /// answers. `UpdateResource` is what fills it.
    pub last_modified: Option<SystemTime>,
}

/// The `VerificationStatus` a registration this service holds reports: "Registered role
/// has sufficient permissions to access the resource" (`ResourceInfo$VerificationStatus`).
///
/// A terminal status, and the only one a held registration is in: the other two values the
/// enum carries (`VERIFICATION_FAILED`, `NOT_VERIFIED`) describe a registration whose role
/// cannot reach the location, and `RegisterResource` refuses a request that names no data
/// access role at all, so nothing this store holds is in either.
pub const REGISTRATION_VERIFIED: &str = "VERIFIED";

impl RegisteredResource {
    /// This registration as `DescribeResource` reports it: the modelled `ResourceInfo`
    /// carrying the registration's own configuration and the verification status derived
    /// from it.
    ///
    /// `ResourceArn` is left absent: it is the ARN the request itself addressed, so the
    /// describe answers the registration rather than echoing its key. A LISTING is the
    /// reader that has to identify each entry it reports, so `ListResources` (a stub
    /// today) carries the ARN in its own projection.
    pub fn to_resource_info(&self) -> crate::ops::types::ResourceInfo {
        crate::ops::types::ResourceInfo {
            resource_arn: None,
            role_arn: Some(self.role_arn.clone()),
            last_modified: self.last_modified.map(::aws_smithy_types::DateTime::from),
            with_federation: self.with_federation,
            hybrid_access_enabled: Some(self.hybrid_access_enabled),
            with_privileged_access: self.with_privileged_access,
            verification_status: Some(REGISTRATION_VERIFIED.to_string()),
            expected_resource_owner_account: Some(self.expected_resource_owner_account.clone()),
        }
    }
}

/// One stored LF-tag definition: the Data Catalog it lives in, its key, and the ordered
/// set of values the key may take.
///
/// `tag_values` is kept in the order its values were supplied, de-duplicated, and is never
/// empty: an LF-tag's values are a set, which the service reports in the order the tag was
/// created and updated with them, and `TagValueList` is modelled `len 1..50`
/// (`smithy_query(query="shape",shape="TagValueList")`), so a tag whose values were all
/// removed could not be rendered back on the wire.
#[derive(Debug, Clone, Default)]
pub struct LfTag {
    /// The catalog this tag belongs to; echoed by `GetLFTag` / `ListLFTags`.
    pub catalog_id: String,
    /// The stored key-name (`LFTagKey`, len 1..128).
    pub tag_key: String,
    /// The values the key may take: ordered, de-duplicated, 1..50 entries.
    pub tag_values: Vec<String>,
}

/// One explicitly granted Lake Formation permission: the principal it was granted to,
/// the resource it was granted on, and the two permission sets `GrantPermissions`
/// carries.
///
/// A record's identity is the (catalog, principal, resource, condition) tuple that
/// [`grant_key`] renders, so granting on the same resource to the same principal twice
/// UPDATES one record instead of adding a second — which is what makes
/// `GrantPermissions` idempotent and `ListPermissions` answer exactly one entry for a
/// repeated grant. Both permission lists are kept as sets in first-granted order, and a
/// record whose two sets are both empty is dropped rather than kept as an entry with no
/// permissions at all. An entry that reports SEVERAL of these records carries their union in
/// the order the `Permission` enum declares its values ([`reported_permissions`]), there
/// being no grant order across records.
#[derive(Debug, Clone)]
pub struct PermissionGrant {
    /// The Data Catalog the grant was made in ([`resolve_catalog_id`]).
    pub catalog_id: String,
    /// `DataLakePrincipal$DataLakePrincipalIdentifier` — the grantee.
    pub principal: String,
    /// The resource, as the modelled shape, with the absent catalog ids the model
    /// documents a default for filled in by [`normalize_resource`], so the same resource
    /// named twice is one record.
    pub resource: Resource,
    /// The permissions held, as a set in first-granted order.
    pub permissions: Vec<String>,
    /// The permissions held WITH the grant option, as a set in first-granted order.
    pub permissions_with_grant_option: Vec<String>,
    /// `Condition$Expression`, when the grant carried one: part of the record's identity,
    /// since the same permissions under two conditions are two grants.
    pub condition_expression: Option<String>,
    /// When the write that last set this record ran — `PrincipalResourcePermissions$LastUpdated`,
    /// "the date and time when the resource was last updated".
    ///
    /// Written by the write itself, never by a read, so two reads of one unchanged grant
    /// report one instant. For a grant a Data Catalog resource was CREATED carrying it is
    /// the instant the catalog records for that resource, which is the write that
    /// established the grant ([`created_table_grants`], [`default_database_grants`]).
    pub last_updated: Option<SystemTime>,
    /// Who that write was — `PrincipalResourcePermissions$LastUpdatedBy`, "the user who
    /// updated the record" — or `None` where no principal is recorded for it.
    pub last_updated_by: Option<String>,
}

impl PermissionGrant {
    /// This record as the wire shape `ListPermissions` projects it.
    ///
    /// One member of the wire shape is left ABSENT, because this service holds nothing to
    /// put in it: `AdditionalDetails` carries the RAM resource-share ARNs of a cross-account
    /// share, and this emulator models no cross-account sharing.
    ///
    /// The record-keeping pair — `LastUpdated` ("The date and time when the resource was
    /// last updated") and `LastUpdatedBy` ("The user who updated the record") — is the
    /// PROVENANCE of the write that set the record, which every reported entry carries: the
    /// instant that write ran and the principal that made it, both recorded when the write
    /// happened rather than read off the clock when a listing runs, so repeated reads of one
    /// unchanged grant agree.
    pub fn to_wire(&self) -> PrincipalResourcePermissions {
        PrincipalResourcePermissions {
            principal: Some(DataLakePrincipal {
                data_lake_principal_identifier: Some(self.principal.clone()),
            }),
            resource: Some(self.resource.clone()),
            condition: self
                .condition_expression
                .clone()
                .map(|expression| Condition {
                    expression: Some(expression),
                }),
            permissions: Some(self.permissions.clone()),
            permissions_with_grant_option: Some(self.permissions_with_grant_option.clone()),
            additional_details: None,
            last_updated: self.last_updated.map(reported_instant),
            last_updated_by: self.last_updated_by.clone(),
        }
    }

    /// Record that `provenance` wrote this grant: the write that last set it is the one a
    /// read reports.
    pub fn written_by(&mut self, provenance: &GrantProvenance) {
        self.last_updated = Some(provenance.at);
        self.last_updated_by = Some(provenance.by.clone());
    }

    /// Carry over the LATER of two provenances, for an entry a listing reports several
    /// records under: what the entry reports is the most recent write any of them carries,
    /// since that is when what the entry reports last changed.
    pub fn absorb_provenance(&mut self, other: &Self) {
        if other.last_updated > self.last_updated {
            self.last_updated = other.last_updated;
            self.last_updated_by = other.last_updated_by.clone();
        }
    }
}

/// An instant as this API REPORTS it: the epoch-seconds timestamp the protocol carries a
/// `@timestamp` member in, at MILLISECOND resolution.
///
/// `LastModifiedTimestamp` declares no `@timestampFormat`
/// (`smithy_query(query="shape",shape="LastModifiedTimestamp")`), so restJson1 carries it as
/// an epoch-seconds number — and the resolution of a timestamp AWS's JSON APIs put on the
/// wire is the millisecond, so the sub-millisecond part of the clock this emulator read is
/// not part of the value a client gets back.
fn reported_instant(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_millis(i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// The PROVENANCE of a write of permissions: when it ran and which principal made it, which
/// is what a read reports as a record's `LastUpdated` / `LastUpdatedBy`.
///
/// Built once per request from the request's own instant and the caller's identity
/// ([`caller_provenance`]), so every record one call writes carries the same provenance.
#[derive(Debug, Clone)]
pub struct GrantProvenance {
    /// The instant the write ran.
    pub at: SystemTime,
    /// The IAM principal the calling credential speaks for.
    pub by: String,
}

/// What a listing reports for one grant: the record as the wire shape carries it
/// ([`PermissionGrant::to_wire`]), with the record-keeping pair dropped where the listing
/// answers EFFECTIVE permissions.
///
/// "If both Principal and Resource parameters are provided, the response returns effective
/// permissions rather than the explicitly granted permissions" (`ListPermissions`). An
/// effective answer is COMPUTED — what the principal holds in its own name together with
/// what it holds through every group it belongs to — rather than a record read back, so
/// there is no single write for it to report the provenance of: `LastUpdated` and
/// `LastUpdatedBy` are the provenance of a RECORD, and the entry a listing of explicitly
/// granted permissions reports IS that record, so only such an entry carries them.
pub fn reported_grant_entry(
    grant: &PermissionGrant,
    effective: bool,
) -> PrincipalResourcePermissions {
    let mut entry = grant.to_wire();
    if effective {
        entry.last_updated = None;
        entry.last_updated_by = None;
    }
    entry
}

/// The provenance a write of this request's own carries: the instant it runs and the IAM
/// principal the calling credential speaks for
/// ([`::service_common::caller_principal::ambient_user_arn`], the tree's ONE definition of
/// that identity, which STS reports as the caller and IAM holds among the account's users).
pub fn caller_provenance(at: SystemTime) -> GrantProvenance {
    GrantProvenance {
        at,
        by: ::service_common::caller_principal::ambient_user_arn(),
    }
}

/// The values of `Permission` in the order that enum DECLARES them
/// (`smithy_query(query="shape", shape="Permission")`, whose `values` are in declaration
/// order), which is the order a MERGED entry's permissions are reported in
/// ([`reported_permissions`]).
///
/// Spelled out here because the generated [`crate::ops::types::PERMISSION_VALUES`] carries
/// the same values SORTED, and what this order is about is the enum's own succession; one
/// test asserts the two name exactly the same set.
pub const PERMISSION_DECLARATION_ORDER: &[&str] = &[
    "ALL",
    "SELECT",
    "ALTER",
    "DROP",
    "DELETE",
    "INSERT",
    "DESCRIBE",
    "CREATE_DATABASE",
    "CREATE_TABLE",
    "DATA_LOCATION_ACCESS",
    "CREATE_LF_TAG",
    "ASSOCIATE",
    "GRANT_WITH_LF_TAG_EXPRESSION",
    "CREATE_LF_TAG_EXPRESSION",
    "CREATE_CATALOG",
    "SUPER_USER",
];

/// The UNION of the permissions several records hold, as the entry reporting them all
/// carries it: the values held, in the order the `Permission` enum declares them
/// ([`PERMISSION_DECLARATION_ORDER`]).
///
/// One record's own two lists are held in the order that record was granted them in, which is
/// the order they are reported in. An entry that reports SEVERAL records has no such order —
/// a principal's own grant and a group's grant resolved onto it were made independently of
/// each other — so what it reports is the union as a SET, in the enum's own succession: a
/// principal holding `DESCRIBE` in its own name and `ALL` through a group is reported holding
/// `ALL` then `DESCRIBE`, whichever record the listing read first.
///
/// A value the enum does not carry cannot be in a record — neither write path records one
/// ([`unmodelled_permissions`]) — and one that somehow were is reported after the modelled
/// values in the order it is held, so nothing a record holds is dropped from the answer.
pub fn reported_permissions(held: &[String]) -> Vec<String> {
    let mut reported: Vec<String> = PERMISSION_DECLARATION_ORDER
        .iter()
        .filter(|declared| held.iter().any(|permission| permission == *declared))
        .map(|declared| (*declared).to_string())
        .collect();
    reported.extend(
        held.iter()
            .filter(|permission| !PERMISSION_DECLARATION_ORDER.contains(&permission.as_str()))
            .cloned(),
    );
    reported
}

/// Per-service state for com-amazonaws-lakeformation-awslakeformation. A handler reaches it through [`service_state`].
///
/// # What stops this growing without end
///
/// An LF-tag is a durable control-plane record: AWS gives it no expiry, so this store
/// has no TTL and no eviction, and a tag is released only by `DeleteLFTag` (which also
/// drops a catalog left empty). What bounds it instead are three ceilings checked by
/// `CreateLFTag`, the single site that inserts: [`MAX_LF_TAGS_PER_CATALOG`] per catalog,
/// [`MAX_LF_TAG_CATALOGS`] catalogs, and [`MAX_LF_TAGS_TOTAL`] tags across every catalog
/// — the last two are what bound the store when the caller varies `CatalogId`, since the
/// catalog id is request-supplied. Passing any of them answers the modelled
/// `ResourceNumberLimitExceededException`, never a silent insert.
///
/// A permission grant is durable in the same way — only `RevokePermissions` releases one
/// — and `GrantPermissions`, its single inserting site, refuses a record past
/// [`MAX_PERMISSION_GRANTS`]. `revoked_default_grants` is a record of the same kind and
/// counts against the same ceiling, checked where `RevokePermissions` inserts into it.
///
/// `registered_resources` is bounded the same way, by [`MAX_REGISTERED_RESOURCES`] checked
/// in `RegisterResource`, the single site that inserts into it.
///
/// `data_cells_filters` is bounded by [`MAX_DATA_CELLS_FILTERS`], checked in
/// `CreateDataCellsFilter`, the single site that inserts into it.
///
/// `data_lake_settings` holds one record per Data Catalog, bounded by
/// [`MAX_DATA_LAKE_SETTINGS_CATALOGS`] checked in `PutDataLakeSettings`, the single site
/// that inserts into it — needed because the catalog id is request-supplied.
#[derive(Debug, Default)]
pub struct ComAmazonawsLakeformationAwslakeformationState {
    /// Catalog id -> (tag key -> tag). Both levels are `BTreeMap`s so enumeration order
    /// is stable and sorted, which is what makes `ListLFTags`' exclusive-start
    /// continuation token resume exactly where the previous page stopped.
    pub lf_tags: BTreeMap<String, BTreeMap<String, LfTag>>,
    /// [`grant_key`] -> the explicit grant it identifies. A `BTreeMap` for the same
    /// reason: `ListPermissions` pages through it in key order and its continuation
    /// token names the key the previous page stopped on.
    pub permission_grants: BTreeMap<String, PermissionGrant>,
    /// The [`grant_key`]s whose creation-time default grant has been revoked.
    ///
    /// A default grant ([`default_database_grant`]) is not a record: nothing granted it,
    /// so there is nothing in `permission_grants` for `RevokePermissions` to trim. What
    /// the revoke leaves behind is this memory that the grant is gone, which is what
    /// makes the default revocable exactly once.
    pub revoked_default_grants: BTreeSet<String>,
    /// Resource ARN -> its registration. A `BTreeMap` for the same reason: `ListResources`
    /// pages over a stable, sorted order, and `DescribeResource` is an exact-ARN lookup.
    pub registered_resources: BTreeMap<String, RegisteredResource>,
    /// [`data_cells_filter_key`] -> the stored data cells filter.
    ///
    /// The modelled `DataCellsFilter` is kept whole, because that is exactly what
    /// `GetDataCellsFilter` answers with and what `ListDataCellsFilter` pages over; a
    /// `BTreeMap` so those readers enumerate in a stable, sorted order.
    pub data_cells_filters: BTreeMap<String, DataCellsFilter>,
    /// Catalog id -> the data lake settings last written for that Data Catalog.
    ///
    /// Keyed per catalog because both operations of the pair address one
    /// (`CatalogId`, "by default, the account ID"), and the whole modelled structure is
    /// kept because `PutDataLakeSettings` REPLACES it ("This API replaces the current list
    /// of data lake admins with the new list being passed") and `GetDataLakeSettings`
    /// answers with exactly it. A catalog with no entry was never configured and reads
    /// back [`default_data_lake_settings`]. A `BTreeMap` for the same reason as the stores
    /// above: a stable, sorted enumeration order.
    pub data_lake_settings: BTreeMap<String, DataLakeSettings>,
}

impl ComAmazonawsLakeformationAwslakeformationState {
    /// How many LF-tags the store holds across every catalog — what
    /// [`MAX_LF_TAGS_TOTAL`] bounds.
    pub fn total_lf_tags(&self) -> usize {
        self.lf_tags.values().map(BTreeMap::len).sum()
    }

    /// Whether `catalog_id` holds an LF-tag under `tag_key`.
    pub fn holds_lf_tag(&self, catalog_id: &str, tag_key: &str) -> bool {
        self.lf_tags
            .get(catalog_id)
            .is_some_and(|catalog| catalog.contains_key(tag_key))
    }

    /// The data lake settings `catalog_id` reads back: the ones last written for it, else
    /// the settings a Data Catalog nobody has configured starts with
    /// ([`default_data_lake_settings`]) — in both cases as a read REPORTS them
    /// ([`reported_data_lake_settings`]).
    pub fn data_lake_settings_for(&self, catalog_id: &str) -> DataLakeSettings {
        let stored = self
            .data_lake_settings
            .get(catalog_id)
            .cloned()
            .unwrap_or_else(default_data_lake_settings);
        reported_data_lake_settings(&stored)
    }
}

/// The most data cells filters this store will hold.
///
/// That the number of data cells filters IS bounded comes from the model:
/// `CreateDataCellsFilter` declares `ResourceNumberLimitExceededException` — "A resource
/// numerical limit was exceeded" (HTTP 400). The model does not carry the number, and AWS
/// publishes it as an adjustable service quota, so this ceiling is the emulator's own
/// choice; what the model fixes is that passing it answers that error instead of growing
/// the store without limit. Needed because every part of a filter's identity (catalog,
/// database, table and filter name) is request-supplied.
pub const MAX_DATA_CELLS_FILTERS: usize = 10_000;

/// The deepest nested column attribute a data cells filter may name.
///
/// From the model: `DataCellsFilter$ColumnNames` documents "When specifying nested
/// attributes, use a qualified dot (.) delimited format such as \"address\".\"zip\".
/// Nested attributes within this list may not exceed a depth of 5."
/// (`smithy_query(query="doc",shape="DataCellsFilter",member="ColumnNames")`).
pub const MAX_COLUMN_NAME_DEPTH: usize = 5;

/// The identity of one data cells filter: the (table catalog, database, table, filter
/// name) tuple every one of its operations addresses it by.
///
/// All four members are modelled required on `DataCellsFilter` and on
/// `GetDataCellsFilter`'s input, so a filter always has a complete key. Length-prefixed
/// like [`grant_key`], so no name a request supplies can be read as the next field.
pub fn data_cells_filter_key(
    table_catalog_id: &str,
    database_name: &str,
    table_name: &str,
    name: &str,
) -> String {
    let mut key = String::new();
    push_text(&mut key, table_catalog_id);
    push_text(&mut key, database_name);
    push_text(&mut key, table_name);
    push_text(&mut key, name);
    key
}

/// The version a data cells filter is created at: "The ID of the data cells filter
/// version" (`DataCellsFilter$VersionId`) of a filter nothing has updated yet.
///
/// The version is the SERVICE's own counter rather than a value a request supplies: a
/// create assigns this one and each accepted update advances it
/// ([`next_filter_version`]), so a `VersionId` a request carries is not stored.
pub const FIRST_FILTER_VERSION: u64 = 1;

/// The version that follows the one a stored filter carries: one past the number `stored`
/// spells, and the first version when it spells no number at all.
pub fn next_filter_version(stored: Option<&str>) -> String {
    stored
        .and_then(|version| version.parse::<u64>().ok())
        .unwrap_or(FIRST_FILTER_VERSION)
        .saturating_add(1)
        .to_string()
}

/// The PartiQL predicate that selects every row.
///
/// The all-rows wildcard says the same thing — "A structure that you pass to indicate you
/// want all rows in a filter" (`AllRowsWildcard`'s own documentation) — so the wildcard
/// and this predicate are one row filter, and a filter that selects every row is reported
/// carrying both forms.
pub const ALL_ROWS_PREDICATE: &str = "TRUE";

/// Whether `expression` is the all-rows predicate, whatever case it is spelled in: what a
/// predicate SELECTS is the requirement, not how it is cased.
pub fn selects_all_rows(expression: Option<&str>) -> bool {
    expression.is_some_and(|expression| {
        expression.trim().eq_ignore_ascii_case(ALL_ROWS_PREDICATE)
    })
}

/// `row_filter` as the service reports one: a filter that selects every row carries both
/// forms of that predicate ([`ALL_ROWS_PREDICATE`] and the wildcard), and any other
/// predicate is reported exactly as it was named.
pub fn reported_row_filter(row_filter: Option<RowFilter>) -> Option<RowFilter> {
    let row_filter = row_filter?;
    if row_filter.all_rows_wildcard.is_some()
        || selects_all_rows(row_filter.filter_expression.as_deref())
    {
        return Some(RowFilter {
            filter_expression: Some(ALL_ROWS_PREDICATE.to_string()),
            all_rows_wildcard: Some(AllRowsWildcard {}),
        });
    }
    Some(row_filter)
}

/// `filter` as this store holds it at `version`: the cells the request named, the row
/// filter the service reports for them ([`reported_row_filter`]) and the version the
/// service assigned — which is what `GetDataCellsFilter` then serves.
///
/// The one place a stored filter is built, so `CreateDataCellsFilter` and
/// `UpdateDataCellsFilter` cannot store two different shapes of the same filter.
pub fn stored_data_cells_filter(filter: &DataCellsFilter, version: String) -> DataCellsFilter {
    DataCellsFilter {
        row_filter: reported_row_filter(filter.row_filter.clone()),
        version_id: Some(version),
        ..filter.clone()
    }
}

/// How many levels the qualified, dot-delimited column path `column` names, or `None`
/// when the path leaves a quoted segment unterminated and so names no levels at all.
///
/// A segment may be double-quoted (`"address"."zip"` — the form
/// `DataCellsFilter$ColumnNames` documents), and a quoted segment may itself contain a
/// dot, so only a dot OUTSIDE quotes separates levels. An odd number of quotes leaves a
/// segment open, which is not a path the documented form admits: answering `None` for it
/// is what stops an unterminated quote hiding the dots after it and scoring a path
/// shallower than it is. An empty path is depth 0.
pub fn column_name_depth(column: &str) -> Option<usize> {
    if column.is_empty() {
        return Some(0);
    }
    let mut depth = 1;
    let mut quoted = false;
    for character in column.chars() {
        match character {
            '"' => quoted = !quoted,
            '.' if !quoted => depth += 1,
            _ => {}
        }
    }
    if quoted {
        return None;
    }
    Some(depth)
}

/// `Ok(())` when exactly one of the two ways a request has of naming a table's columns is
/// used, else why it is not.
///
/// One rule, stated by the model for each of the two shapes that carry the pair: "At least
/// one of `ColumnNames` or `ColumnWildcard` is required."
/// (`smithy_query(query="doc",shape="TableWithColumnsResource",member="ColumnNames")`) and
/// "You must specify either a `ColumnNames` list or the `ColumnWildCard`."
/// (`smithy_query(query="doc",shape="DataCellsFilter",member="ColumnWildcard")`) — so one
/// of the two and never both, and a `ColumnNames` that selects no column at all is not a
/// naming of columns. `context` names the member under validation, so the refusal says
/// which one it is about.
pub fn validate_column_choice(
    context: &str,
    column_names: Option<&[String]>,
    has_column_wildcard: bool,
) -> Result<(), String> {
    match (column_names, has_column_wildcard) {
        (Some(_), true) => Err(format!(
            "{context} must name either ColumnNames or ColumnWildcard, not both"
        )),
        (None, false) => Err(format!(
            "{context} must name either ColumnNames or ColumnWildcard"
        )),
        (Some([]), false) => Err(format!(
            "{context}.ColumnNames must name at least one column"
        )),
        _ => Ok(()),
    }
}

/// `Ok(())` when `filter` is a data cells filter the service can store, else why it is
/// not — the message of the modelled `InvalidInputException` its operations answer with.
///
/// Two rules the model states but cannot express as constraints:
///
/// * exactly one way of naming columns, which [`validate_column_choice`] is;
/// * a nested column attribute no deeper than [`MAX_COLUMN_NAME_DEPTH`], applied to both
///   lists of column paths the shape carries (`ColumnNames` and the wildcard's
///   `ExcludedColumnNames`, which target the same `ColumnNames` shape). This one is the
///   data cells filter's own: `TableWithColumnsResource$ColumnNames` documents no nesting
///   at all, so it is not held to a depth the model states only for `DataCellsFilter`.
///
/// A `RowFilter` is REQUIRED of the filter, and held to the same "exactly one" rule over
/// its own two alternatives: a PartiQL predicate is either an expression or the all-rows
/// wildcard, and a predicate that is neither selects no rows. The member is optional in the
/// shape and the service requires it of the filter ([`ROW_FILTER_REQUIRED`]), a filter
/// describing "certain columns on certain rows" (`smithy_query(query="shape",
/// shape="DataCellsFilter")`) no more without its rows than without its columns.
pub fn validate_data_cells_filter(filter: &DataCellsFilter) -> Result<(), String> {
    validate_column_choice(
        "TableData",
        filter.column_names.as_deref(),
        filter.column_wildcard.is_some(),
    )?;

    let excluded = filter
        .column_wildcard
        .as_ref()
        .and_then(|wildcard| wildcard.excluded_column_names.as_deref())
        .unwrap_or_default();
    for column in filter
        .column_names
        .as_deref()
        .unwrap_or_default()
        .iter()
        .chain(excluded)
    {
        let Some(depth) = column_name_depth(column) else {
            return Err(format!(
                "Column {column:?} leaves a quoted segment unterminated"
            ));
        };
        if depth > MAX_COLUMN_NAME_DEPTH {
            return Err(format!(
                "Column {column:?} nests {depth} attributes, the maximum depth is \
                 {MAX_COLUMN_NAME_DEPTH}"
            ));
        }
    }

    if let Some(row_filter) = &filter.row_filter {
        let expression = row_filter.filter_expression.is_some();
        let all_rows = row_filter.all_rows_wildcard.is_some();
        // A predicate is an expression OR the wildcard, never both — even where the
        // expression is the all-rows predicate the wildcard stands for
        // ([`ALL_ROWS_PREDICATE`]), which is what a filter selecting every row is
        // REPORTED carrying rather than something a request may name beside the wildcard.
        if expression && all_rows {
            return Err(ROW_FILTER_BOTH_SET.to_string());
        }
        if !expression && !all_rows {
            return Err(
                "TableData.RowFilter must name either FilterExpression or AllRowsWildcard"
                    .to_string(),
            );
        }
    } else {
        // No predicate at all names no rows, which is the same gap as a predicate naming
        // neither alternative: the service answers its own sentence for it.
        return Err(ROW_FILTER_REQUIRED.to_string());
    }
    Ok(())
}

/// The largest page `ListLFTags` can answer, and the page size it uses when the caller
/// names none.
///
/// Grounded in the model, not chosen: the output member `LFTags` targets `LFTagsList`,
/// which is `len 1..50`, so no single response can carry more than 50 LF-tag pairs even
/// though `MaxResults` is modelled `range 1..1000`.
pub const MAX_LF_TAGS_PAGE: i32 = 50;

/// The most values one LF-tag can hold.
///
/// Grounded in the model: `TagValues` targets `TagValueList`, which is `len 1..50`
/// (`smithy_query(query="shape",shape="TagValueList")`), so a tag can neither lose every
/// value nor grow past 50.
pub const MAX_TAG_VALUES: usize = 50;

/// The most LF-tags one Data Catalog can hold here.
///
/// That the number of LF-tags a catalog may hold IS bounded comes from the model:
/// `CreateLFTag` declares `ResourceNumberLimitExceededException` — "A resource numerical
/// limit was exceeded" (HTTP 400). The model does not carry the number, and AWS publishes
/// it as an adjustable service quota, so this ceiling is the emulator's own choice; what
/// the model fixes is that passing it answers that error instead of growing the store
/// without limit.
pub const MAX_LF_TAGS_PER_CATALOG: usize = 1000;

/// The most Data Catalogs this store will hold LF-tags for.
///
/// `CatalogId` is request-supplied, so the per-catalog ceiling alone would let a caller
/// that varies it grow the store without end. This bounds the partition count itself;
/// a create that would open one catalog too many answers the modelled
/// `ResourceNumberLimitExceededException`. Emulator-chosen, like the ceiling above.
pub const MAX_LF_TAG_CATALOGS: usize = 100;

/// The most LF-tags this store will hold across every catalog.
///
/// The store-wide ceiling, so no partitioning of the same number of create calls can
/// grow resident state past it. A create that would pass it answers the modelled
/// `ResourceNumberLimitExceededException`. Emulator-chosen, like the ceilings above.
pub const MAX_LF_TAGS_TOTAL: usize = 10_000;

/// The principal the data lake settings' default grant names: AWS's own group standing
/// for "every IAM principal that IAM itself allows", spelled exactly like this on the
/// wire.
pub const IAM_ALLOWED_PRINCIPALS: &str = "IAM_ALLOWED_PRINCIPALS";

/// The one permission the "use only IAM access control" default grant carries.
///
/// From the model, not chosen: `DataLakeSettings$CreateDatabaseDefaultPermissions`
/// documents that "A value that assigns ALL to IAM_ALLOWED_PRINCIPALS indicates access
/// control by IAM permissions. This is referred to as the setting \"Use only IAM access
/// control\", and is for backward compatibility with the Glue permission model", and that
/// "The only permitted values are an empty array or an array that contains a single JSON
/// object that grants ALL to IAM_ALLOWED_PRINCIPALS"
/// (`smithy_query(query="doc",shape="DataLakeSettings",member="CreateDatabaseDefaultPermissions")`).
pub const DEFAULT_IAM_ACCESS_CONTROL_PERMISSION: &str = "ALL";

/// The most Data Catalogs this store will hold data lake settings for.
///
/// `CatalogId` is request-supplied, so without a ceiling a caller that varies it would
/// grow the store without end. `PutDataLakeSettings` declares no numerical-limit error, so
/// a write that would open one catalog too many is refused with the modelled
/// `InvalidInputException` it does declare. Emulator-chosen, like the ceilings above.
pub const MAX_DATA_LAKE_SETTINGS_CATALOGS: usize = 1_000;

/// The `CreateDatabaseDefaultPermissions` / `CreateTableDefaultPermissions` value that
/// selects "Use only IAM access control": [`DEFAULT_IAM_ACCESS_CONTROL_PERMISSION`] to
/// [`IAM_ALLOWED_PRINCIPALS`], the one entry those two members admit beside the empty list.
pub fn iam_access_control_default_permissions() -> Vec<PrincipalPermissions> {
    vec![PrincipalPermissions {
        principal: Some(DataLakePrincipal {
            data_lake_principal_identifier: Some(IAM_ALLOWED_PRINCIPALS.to_string()),
        }),
        permissions: Some(vec![DEFAULT_IAM_ACCESS_CONTROL_PERMISSION.to_string()]),
    }]
}

/// The settings a Data Catalog nothing has written settings for reads back.
///
/// The two create-default members carry the "Use only IAM access control" value, which is
/// from the model rather than chosen: it is the setting that "is for backward compatibility
/// with the Glue permission model implemented by IAM permissions"
/// (see [`DEFAULT_IAM_ACCESS_CONTROL_PERMISSION`]), so it is what a catalog that has not
/// been handed to Lake Formation is under — and it is what [`default_database_grants`]
/// reports for the databases such a catalog holds.
///
/// Nothing has been designated yet, so the two administrator lists and the two list members
/// that name accounts read back EMPTY, while every member whose absence the model gives a
/// meaning of its own — the two nullable flags, and `Parameters` — is left absent rather
/// than filled with a value nothing set. The three members the service reports for every
/// catalog are left to [`reported_data_lake_settings`], the one place their defaults live.
pub fn default_data_lake_settings() -> DataLakeSettings {
    DataLakeSettings {
        disallow_grant_on_iam_allowed_principals: None,
        set_source_identity: None,
        whitelisted_for_external_data_filtering: None,
        data_lake_admins: Some(Vec::new()),
        read_only_admins: Some(Vec::new()),
        create_database_default_permissions: Some(iam_access_control_default_permissions()),
        create_table_default_permissions: Some(iam_access_control_default_permissions()),
        parameters: None,
        trusted_resource_owners: Some(Vec::new()),
        allow_external_data_filtering: None,
        allow_full_table_external_data_access: None,
        external_data_filtering_allow_list: Some(Vec::new()),
        authorized_session_tag_value_list: Some(Vec::new()),
    }
}

/// The `Parameters` key that carries the source-identity setting, and the value of it that
/// turns the setting on: "SET_SOURCE_IDENTITY - Accepted values are TRUE and FALSE. When
/// set to TRUE, Lake Formation includes the IAM role identifier that was used to query in
/// the S3 data event CloudTrail logs"
/// (`smithy_query(query="doc",shape="DataLakeSettings",member="Parameters")`).
///
/// The settings document reports that same setting as the `SetSourceIdentity` member, which
/// is why the key is named here rather than spelled at each site that reads it.
pub const SET_SOURCE_IDENTITY_PARAMETER: &str = "SET_SOURCE_IDENTITY";

/// The `SET_SOURCE_IDENTITY` value that turns the setting on.
pub const PARAMETER_TRUE: &str = "TRUE";

/// The `Parameters` keys the model documents a value set for, and the values each admits.
///
/// From the model: "CROSS_ACCOUNT_VERSION - Accepted values are 1, 2, 3, 4, and 5" and
/// "SET_SOURCE_IDENTITY - Accepted values are TRUE and FALSE"
/// (`smithy_query(query="doc",shape="DataLakeSettings",member="Parameters")`). A key the
/// model documents no value set for is stored as written.
pub const DOCUMENTED_SETTINGS_PARAMETERS: [(&str, &[&str]); 2] = [
    ("CROSS_ACCOUNT_VERSION", &["1", "2", "3", "4", "5"]),
    (SET_SOURCE_IDENTITY_PARAMETER, &[PARAMETER_TRUE, "FALSE"]),
];

/// `settings` as a read of a Data Catalog's data lake settings REPORTS them: the stored
/// document, plus the three members the service reports for every catalog whether or not
/// the write that configured it named them.
///
/// Those three are what a client needs to round-trip the document — the only supported way
/// to change one field is to fetch the settings, change it and write the whole structure
/// back — so a read that left them absent would lose them on the next write. The two
/// nullable flags of the modelled structure are NOT among them: absence carries a meaning
/// of its own there, so a flag nothing set stays absent.
///
/// `SetSourceIdentity` is the settings document's reading of the `SET_SOURCE_IDENTITY`
/// parameter ([`SET_SOURCE_IDENTITY_PARAMETER`]) — one setting under two names, so a
/// document that carries the parameter reports the member that agrees with it.
///
/// Reporting only: it derives a response and writes nothing.
pub fn reported_data_lake_settings(settings: &DataLakeSettings) -> DataLakeSettings {
    let from_parameters = settings
        .parameters
        .as_ref()
        .and_then(|parameters| parameters.get(SET_SOURCE_IDENTITY_PARAMETER))
        .map(|value| value.eq_ignore_ascii_case(PARAMETER_TRUE));
    DataLakeSettings {
        disallow_grant_on_iam_allowed_principals: Some(
            settings
                .disallow_grant_on_iam_allowed_principals
                .unwrap_or(false),
        ),
        set_source_identity: Some(
            settings
                .set_source_identity
                .or(from_parameters)
                .unwrap_or(false),
        ),
        whitelisted_for_external_data_filtering: Some(
            settings
                .whitelisted_for_external_data_filtering
                .clone()
                .unwrap_or_default(),
        ),
        ..settings.clone()
    }
}

/// Whether `entry` is the one create-default entry that selects "Use only IAM access
/// control": [`DEFAULT_IAM_ACCESS_CONTROL_PERMISSION`] and nothing else, to
/// [`IAM_ALLOWED_PRINCIPALS`].
fn is_iam_access_control_default(entry: &PrincipalPermissions) -> bool {
    let principal = entry
        .principal
        .as_ref()
        .and_then(|principal| principal.data_lake_principal_identifier.as_deref());
    let only_permission = match entry.permissions.as_deref().unwrap_or_default() {
        [permission] => Some(permission.as_str()),
        _ => None,
    };
    principal == Some(IAM_ALLOWED_PRINCIPALS)
        && only_permission == Some(DEFAULT_IAM_ACCESS_CONTROL_PERMISSION)
}

/// `Ok(())` when `settings` are data lake settings the service can store, else why they are
/// not — the message of the modelled `InvalidInputException` `PutDataLakeSettings` answers
/// with.
///
/// Four rules the model states but cannot express as constraints:
///
/// * every designated administrator is an IAM user or an IAM role — "A list of Lake
///   Formation principals. Supported principals are IAM users or IAM roles."
///   (`DataLakeSettings$DataLakeAdmins`, and the same sentence on `ReadOnlyAdmins`), which
///   [`is_iam_principal`] is. It is read in two steps, because the service refuses the two
///   in two different voices: an identifier that is not an ARN at all ([`ARN_PREFIX`])
///   never reaches the question of which principal it names and draws the ARN reader's own
///   refusal ([`invalid_arn`]), naming the rejected value; an ARN that names something
///   other than an IAM user or role draws the refusal of the rule above;
/// * no principal is designated in BOTH administrator lists: the two members are separate
///   roles — a data lake administrator may "make changes" where a read-only one has "only
///   view access to the resources, without the ability to make changes"
///   (`DataLakeSettings$ReadOnlyAdmins`) — so one identity cannot hold both, and the
///   refusal is [`BOTH_ADMIN_ROLES`];
/// * each create-default member is the empty list or the single entry that selects "Use
///   only IAM access control" — "The only permitted values are an empty array or an array
///   that contains a single JSON object that grants ALL to IAM_ALLOWED_PRINCIPALS";
/// * a `Parameters` key the model documents a value set for carries one of those values
///   ([`DOCUMENTED_SETTINGS_PARAMETERS`]). The comparison folds case, so the documented
///   value spelled in another case is stored rather than refused.
pub fn validate_data_lake_settings(settings: &DataLakeSettings) -> Result<(), String> {
    for (member, admins) in [
        ("DataLakeAdmins", &settings.data_lake_admins),
        ("ReadOnlyAdmins", &settings.read_only_admins),
    ] {
        for admin in admins.iter().flatten() {
            let identifier = admin
                .data_lake_principal_identifier
                .as_deref()
                .unwrap_or_default();
            // A principal is addressed by ARN, so an identifier that does not even begin
            // one is refused by the service's ARN reader before the question of WHICH
            // principal it names is reached, and the refusal is that reader's sentence
            // ([`invalid_arn`]) carrying the value the request wrote.
            if !identifier.starts_with(ARN_PREFIX) {
                return Err(invalid_arn(identifier));
            }
            if !is_iam_principal(identifier) {
                return Err(format!(
                    "{member} principal {identifier:?} is not supported: a data lake \
                     administrator is an IAM user or an IAM role"
                ));
            }
        }
    }

    let administrators: BTreeSet<&str> = settings
        .data_lake_admins
        .iter()
        .flatten()
        .filter_map(|admin| admin.data_lake_principal_identifier.as_deref())
        .collect();
    if settings
        .read_only_admins
        .iter()
        .flatten()
        .filter_map(|admin| admin.data_lake_principal_identifier.as_deref())
        .any(|identifier| administrators.contains(identifier))
    {
        return Err(BOTH_ADMIN_ROLES.to_string());
    }

    for (member, entries) in [
        (
            "CreateDatabaseDefaultPermissions",
            &settings.create_database_default_permissions,
        ),
        (
            "CreateTableDefaultPermissions",
            &settings.create_table_default_permissions,
        ),
    ] {
        let permitted = match entries.as_deref().unwrap_or_default() {
            [] => true,
            [only] => is_iam_access_control_default(only),
            _ => false,
        };
        if !permitted {
            return Err(format!(
                "The only permitted values for {member} are an empty array or an array that \
                 contains a single JSON object that grants \
                 {DEFAULT_IAM_ACCESS_CONTROL_PERMISSION} to {IAM_ALLOWED_PRINCIPALS}"
            ));
        }
    }

    for (key, value) in settings.parameters.iter().flatten() {
        for (documented, accepted) in DOCUMENTED_SETTINGS_PARAMETERS {
            if key == documented && !accepted.contains(&value.to_uppercase().as_str()) {
                return Err(format!(
                    "Parameter {key} accepts only {}, got {value:?}",
                    accepted.join(", ")
                ));
            }
        }
    }

    Ok(())
}

/// The refusal for a write that designates one principal as both a read-only administrator
/// and a data lake administrator.
///
/// Wholly fixed text, as the service words it: the refusal names the contradiction and not
/// the principal that caused it, so a request naming several of them is answered the same
/// way.
pub const BOTH_ADMIN_ROLES: &str = "Principal cannot be both readOnly and LF admin.";

/// The permissions `settings`' `CreateDatabaseDefaultPermissions` grants `principal` on a
/// database created under them, in first-named order with duplicates dropped.
///
/// A setting that names no entry for the principal grants it nothing, and so does an absent
/// member: "A null value indicates access control by Lake Formation permissions"
/// (`DataLakeSettings$CreateDatabaseDefaultPermissions`), which is to say nothing is held
/// until something grants it.
pub fn create_database_default_permissions(
    settings: &DataLakeSettings,
    principal: &str,
) -> Vec<String> {
    let granted: Vec<&String> = settings
        .create_database_default_permissions
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|entry| {
            entry
                .principal
                .as_ref()
                .and_then(|named| named.data_lake_principal_identifier.as_deref())
                == Some(principal)
        })
        .flat_map(|entry| entry.permissions.iter().flatten())
        .collect();
    dedup_values(granted)
}

/// The principals `settings`' `CreateDatabaseDefaultPermissions` names, in first-named
/// order with duplicates dropped — the principals a listing reports a created database's
/// default grant under.
pub fn create_database_default_principals(settings: &DataLakeSettings) -> Vec<String> {
    let named: Vec<&String> = settings
        .create_database_default_permissions
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            entry
                .principal
                .as_ref()?
                .data_lake_principal_identifier
                .as_ref()
        })
        .collect();
    dedup_values(named)
}

/// The grant a Data Catalog DATABASE is created carrying, until a revoke takes it away:
/// what `settings`' `CreateDatabaseDefaultPermissions` grants `principal`, which on a
/// catalog nothing has configured is `ALL` to `IAM_ALLOWED_PRINCIPALS`, the "Use only IAM
/// access control" default (see [`default_data_lake_settings`]). `None` — nothing is held
/// by default — for anything that setting does not cover.
///
/// `settings` is the ADDRESSED catalog's, read from the store
/// ([`ComAmazonawsLakeformationAwslakeformationState::data_lake_settings_for`]), so what a
/// database is created holding follows what `PutDataLakeSettings` last wrote rather than a
/// value fixed here.
///
/// Why it stops at databases: the default a resource is created with comes from the
/// account's data lake settings for a DATABASE
/// (`DataLakeSettings$CreateDatabaseDefaultPermissions`), but for a TABLE it comes from
/// the parent database's own `CreateTableDefaultPermissions` in the Data Catalog. The
/// database default is this service's own setting, so it is the one this store can answer
/// for. What a table was CREATED carrying — its creator's Super and the table-level
/// defaults its database hands it — is rendered by [`created_table_grants`].
///
/// It is rendered per request rather than written when the database is created, because
/// this store never sees a database being created. The two calls that can change what it
/// renders are `RevokePermissions`, which takes the grant away for one database — recorded
/// in `revoked_default_grants`, which answers every later call — and `PutDataLakeSettings`,
/// which changes the setting itself for every database of the catalog at once rather than
/// only for the ones created after it.
///
/// One consequence of this rendering, deliberate: every database NAME carries the default
/// on the REVOKE path, including one the Data Catalog does not hold, so a revoke against a
/// database that does not exist answers 200 where the real service refuses the request.
/// [`missing_grant_entity`] is what can tell the two apart — `GrantPermissions` refuses on
/// it — but `RevokePermissions` does NOT apply it: a revoke is refused by what the grantee
/// HOLDS, and the recorded trace shows that refusal naming the permissions it does not
/// have. Refusing a revoke on the entity instead would need a recorded unit to pin which
/// of the two the real service answers first.
///
/// What a LISTING reports is [`default_database_grants`], which renders the same grant for
/// the databases the Data Catalog actually holds.
pub fn default_database_grant(
    settings: &DataLakeSettings,
    resource: &Resource,
    principal: &str,
    condition: Option<&str>,
) -> Option<Vec<String>> {
    // The default carries no condition, so a condition-scoped request addresses a
    // different grant from the default one.
    if resource.database.is_none() || condition.is_some() {
        return None;
    }
    let permissions = create_database_default_permissions(settings, principal);
    (!permissions.is_empty()).then_some(permissions)
}

/// The default grants a listing in `catalog_id` must report, keyed by [`grant_key`] so
/// each one merges with the explicit grant on the same resource.
///
/// A Data Catalog database is created carrying the addressed catalog's
/// `DataLakeSettings$CreateDatabaseDefaultPermissions` as a DATABASE-level grant — on a
/// catalog nothing has configured, `ALL` to [`IAM_ALLOWED_PRINCIPALS`] — and holds it until
/// a revoke takes it away, so a reader of that database's permissions sees it. One entry is
/// rendered per principal that setting names ([`create_database_default_principals`]), so a
/// listing reports the grant under whoever holds it. The permissions a database
/// hands to the TABLES created in it are a different setting
/// (`Database$CreateTableDefaultPermissions`, held in the Data Catalog), so they change
/// nothing here.
///
/// Which databases: the one a `Database` resource filter names, or every database the
/// catalog holds when the listing filters on no resource at all. A filter naming any other
/// resource selects no database-level grant.
///
/// Only a database the Data Catalog HOLDS carries one, so a name it does not hold, and a
/// catalog this emulator holds nothing for, report nothing: what a listing says has to be
/// grounded in what a store says, and a catalog that cannot be enumerated cannot be
/// answered for one name either. (`RevokePermissions` is looser — see
/// [`default_database_grant`] — because a revoke is refused by what the grantee holds.)
///
/// The databases belong to GLUE, so they are read from Glue's own store in the shared
/// engine, the same seam [`missing_grant_entity`] uses. Call this BEFORE locking this
/// service's store: it takes Glue's lock itself, and never holds the two at the same time.
///
/// `resource_filter`, when present, must already be [`normalize_resource`]d, so the name
/// is the one the catalog stores and the catalog id is filled in — that is what makes the
/// key here the one `RevokePermissions` records a revoked default under.
pub fn default_database_grants<Eng: Send + Sync + 'static>(
    engine: &Eng,
    catalog_id: &str,
    resource_filter: Option<&Resource>,
    settings: &DataLakeSettings,
) -> Vec<(String, PermissionGrant)> {
    let (database_catalog, named) = match resource_filter {
        Some(resource) => {
            let Some(database) = &resource.database else {
                return Vec::new();
            };
            (
                resource_catalog(database.catalog_id.as_deref(), catalog_id),
                Some(database.name.clone()),
            )
        }
        None => (catalog_id.to_string(), None),
    };

    // The databases the Data Catalog holds under the addressed catalog id: the one named,
    // if it is there, else every one of them, each with the instant it was created — the
    // write that made the grant this renders.
    let Some(catalog) = glue_state::service_state(engine) else {
        return Vec::new();
    };
    let names: Vec<(String, SystemTime)> = {
        let catalog = catalog.lock();
        let Some(held) = catalog.catalogs.get(&database_catalog) else {
            return Vec::new();
        };
        match named {
            Some(name) => {
                let stored = glue_state::fold_name(&name);
                match held.databases.get(&stored) {
                    Some(database) => vec![(stored, database.create_time)],
                    None => return Vec::new(),
                }
            }
            None => held
                .databases
                .values()
                .map(|database| (database.name.clone(), database.create_time))
                .collect(),
        }
    };

    // One grant per (database, principal the setting grants to), with the permissions each
    // principal is granted computed once: they depend on the setting alone, not on which
    // database carries them. A principal the setting names with no permission at all is
    // dropped here — an entry carrying nothing is not a grant.
    let granted: Vec<(String, Vec<String>)> = create_database_default_principals(settings)
        .into_iter()
        .map(|principal| {
            let permissions = create_database_default_permissions(settings, &principal);
            (principal, permissions)
        })
        .filter(|(_, permissions)| !permissions.is_empty())
        .collect();
    let mut grants: Vec<(String, PermissionGrant)> = Vec::new();
    for (name, created_at) in names {
        for (principal, permissions) in &granted {
            let resource = Resource {
                database: Some(DatabaseResource {
                    catalog_id: Some(database_catalog.clone()),
                    name: name.clone(),
                }),
                ..Resource::default()
            };
            let grant = PermissionGrant {
                catalog_id: catalog_id.to_string(),
                principal: principal.clone(),
                resource,
                permissions: permissions.clone(),
                // The default carries no grant option and no condition.
                permissions_with_grant_option: Vec::new(),
                condition_expression: None,
                // The write that made this grant is the CREATE of the database, so that is
                // the instant a listing reports for it, and the principal that made it is
                // the identity this emulator's credential speaks for — the one every call
                // into it is made under ([`caller_provenance`]).
                last_updated: Some(created_at),
                last_updated_by: Some(
                    ::service_common::caller_principal::ambient_user_arn(),
                ),
            };
            let key = grant_key(catalog_id, &grant.principal, &grant.resource, None);
            grants.push((key, grant));
        }
    }
    grants
}

/// The Data Catalog a resource member belongs to: the one it names, else the one the
/// request addresses ("The identifier for the Data Catalog. By default, the account ID.",
/// and "By default, it is the account ID of the caller" on the catalog id INSIDE a
/// resource — `TableResource$CatalogId`).
///
/// The one definition of that rule, so the sites that apply it — a listing's rendering of
/// what a resource was created carrying, and [`missing_grant_entity`]'s resolution of the
/// entity a grant names — cannot drift on the empty-string case.
pub fn resource_catalog(named: Option<&str>, default_catalog: &str) -> String {
    match named.filter(|id| !id.is_empty()) {
        Some(id) => id.to_string(),
        None => default_catalog.to_string(),
    }
}

/// The first field of every Amazon Resource Name, with its separator: what a value has to
/// begin with to be read as an ARN at all.
pub const ARN_PREFIX: &str = "arn:";

/// The `arn:<partition>:iam::<account>:<type>/…` resource types that name an IAM
/// principal: an IAM user or an IAM role. Nothing else under the `iam` service is a
/// principal IAM can allow a request from (an identity provider, a policy, an instance
/// profile), and a decorated principal of another identity system carries its own type
/// here.
pub const IAM_PRINCIPAL_RESOURCE_TYPES: [&str; 2] = ["role", "user"];

/// Whether `identifier` names an IAM principal, i.e. an IAM user or role ARN.
///
/// The six-field `arn:partition:service:region:account-id:resource` layout, with the
/// service `iam` and one of [`IAM_PRINCIPAL_RESOURCE_TYPES`] as the resource's type.
pub fn is_iam_principal(identifier: &str) -> bool {
    let fields: Vec<&str> = identifier.splitn(6, ':').collect();
    if fields.len() < 6 || fields[0] != "arn" || fields[2] != "iam" {
        return false;
    }
    fields[5]
        .split_once('/')
        .is_some_and(|(resource_type, rest)| {
            IAM_PRINCIPAL_RESOURCE_TYPES.contains(&resource_type) && !rest.is_empty()
        })
}

/// Whether a grant held by `grant_principal` is part of what `queried` EFFECTIVELY holds.
///
/// Its own grants, plus the grants of every group it belongs to. The one group this
/// service models is [`IAM_ALLOWED_PRINCIPALS`], which stands for every principal IAM
/// itself allows — so an IAM user or role ([`is_iam_principal`]) holds what that group
/// holds, while a principal of another identity system, and the group itself, match only
/// by name.
pub fn effective_principal_holds(queried: &str, grant_principal: &str) -> bool {
    grant_principal == queried
        || (grant_principal == IAM_ALLOWED_PRINCIPALS && is_iam_principal(queried))
}

/// The most revoked-default records this store will hold, beside the explicit grants
/// [`MAX_PERMISSION_GRANTS`] bounds.
///
/// A revoked default is keyed by a request-supplied database name, so it needs a ceiling
/// of its own for the same reason the grant store does; it gets a separate one so neither
/// store's capacity depends on how full the other is. `RevokePermissions`, the single site
/// that inserts one, refuses past this ceiling with its modelled `InvalidInputException`.
/// Emulator-chosen, like the ceilings above.
pub const MAX_REVOKED_DEFAULT_GRANTS: usize = 10_000;

/// The most data-lake locations this store will hold registrations for.
///
/// That the number of registered locations IS bounded comes from the model:
/// `RegisterResource` declares `ResourceNumberLimitExceededException` — "A resource
/// numerical limit was exceeded" (HTTP 400). The model does not carry the number, and AWS
/// publishes it as an adjustable service quota, so this ceiling is the emulator's own
/// choice; what the model fixes is that passing it answers that error instead of growing
/// the store without limit.
pub const MAX_REGISTERED_RESOURCES: usize = 1_000;

/// The Lake Formation service-linked role a registration uses when the request sets
/// `UseServiceLinkedRole` and names no `RoleArn`.
///
/// The role name is the one `RegisterResource`'s documentation names
/// ("AWSServiceRoleForLakeFormationDataAccess"); the ARN is built for this emulator's
/// account, and matches `IAMRoleArn`'s `^arn:aws:iam::[0-9]*:role/` pattern.
pub fn service_linked_role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/aws-service-role/lakeformation.amazonaws.com/AWSServiceRoleForLakeFormationDataAccess",
        ::engine::emulator_account_id()
    )
}

/// `Ok(())` when `arn` has the shape of an Amazon Resource Name, else the refusal the
/// service answers a value that is not one with ([`UNSUPPORTED_RESOURCE_ARN`]).
///
/// `ResourceArnString` carries no pattern in the model, so the boundary validator admits
/// any string and the handler is what must refuse a value that is not an ARN at all. The
/// six-field `arn:partition:service:region:account-id:resource` layout is the general ARN
/// format; `region` and `account-id` are empty for an Amazon S3 location
/// (`arn:aws:s3:::my-bucket/`), so only `partition`, `service` and `resource` are required
/// to be non-empty.
pub fn validate_resource_arn(arn: &str) -> Result<(), String> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let malformed = fields.len() < 6
        || fields[0] != "arn"
        || fields[1].is_empty()
        || fields[2].is_empty()
        || fields[5].is_empty();
    if malformed {
        return Err(UNSUPPORTED_RESOURCE_ARN.to_string());
    }
    Ok(())
}

/// The Data Catalog a request addresses: its `CatalogId` when it carries one, else the
/// caller's account id ("The identifier for the Data Catalog. By default, the account
/// ID." — `CatalogIdString`'s own documentation).
pub fn resolve_catalog_id(requested: Option<&str>) -> String {
    match requested {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => ::engine::emulator_account_id().to_string(),
    }
}

/// The values of `values`, in first-seen order, with duplicates dropped.
///
/// An LF-tag's values are a SET of the values the key may take, and so are a grant's
/// permissions, so a repeated value in one request, or a value already stored, must not
/// appear twice in the stored record.
///
/// The membership test is a `BTreeSet`, so the cost grows with `n log n` whatever the
/// number of distinct values — which matters wherever the list is request-supplied with
/// no modelled length constraint, as `PermissionList` is.
pub fn dedup_values<'a, I>(values: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a String>,
{
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    for value in values {
        if seen.insert(value.as_str()) {
            out.push(value.clone());
        }
    }
    out
}

/// The largest page `ListPermissions` answers, and the page size it uses when the caller
/// names none.
///
/// Grounded in the model: `MaxResults` is `PageSize`, `range 1..1000`, and the output
/// member `PrincipalResourcePermissions` targets `PrincipalResourcePermissionsList`,
/// which carries no length limit of its own — so the modelled ceiling on `MaxResults` is
/// what bounds a page.
pub const MAX_PERMISSIONS_PAGE: i32 = 1000;

/// The most explicit permission grants this store will hold.
///
/// A grant is a durable record released only by `RevokePermissions`, and both the
/// principal and the resource are request-supplied, so without a ceiling a caller that
/// varies either would grow the store without end. `GrantPermissions` is the single site
/// that inserts, and it refuses a record past this with the modelled
/// `InvalidInputException` (the operation models no quota error of its own:
/// `smithy_query(query="operation",operation="GrantPermissions")` lists only
/// `ConcurrentModificationException`, `EntityNotFoundException` and
/// `InvalidInputException`). Emulator-chosen, like the LF-tag ceilings above.
pub const MAX_PERMISSION_GRANTS: usize = 10_000;

/// The `DataLakeResourceType` values that select `resource`.
///
/// From the model: `DataLakeResourceType` is `CATALOG | DATABASE | DATA_LOCATION |
/// LF_NAMED_TAG_EXPRESSION | LF_TAG | LF_TAG_POLICY | LF_TAG_POLICY_DATABASE |
/// LF_TAG_POLICY_TABLE | TABLE` (`crate::ops::types::DATA_LAKE_RESOURCE_TYPE_VALUES`),
/// while `Resource` carries ten members — so the mapping is not one-to-one:
///
/// * a column-level grant (`TableWithColumns`) is a grant on a TABLE;
/// * an LF-tag policy is selected both by the general `LF_TAG_POLICY` and by the
///   specialisation its own `ResourceType` (`DATABASE | TABLE`) names;
/// * `DataCellsFilter` and `RLSPolicy` have NO `DataLakeResourceType` member, so no
///   `ResourceType` filter selects them (a listing that names no `ResourceType` still
///   does).
pub fn resource_types(resource: &Resource) -> Vec<&'static str> {
    if resource.catalog.is_some() {
        vec!["CATALOG"]
    } else if resource.database.is_some() {
        vec!["DATABASE"]
    } else if resource.table.is_some() || resource.table_with_columns.is_some() {
        vec!["TABLE"]
    } else if resource.data_location.is_some() {
        vec!["DATA_LOCATION"]
    } else if resource.lf_tag.is_some() {
        vec!["LF_TAG"]
    } else if let Some(policy) = &resource.lf_tag_policy {
        match policy.resource_type.as_str() {
            "DATABASE" => vec!["LF_TAG_POLICY", "LF_TAG_POLICY_DATABASE"],
            "TABLE" => vec!["LF_TAG_POLICY", "LF_TAG_POLICY_TABLE"],
            _ => vec!["LF_TAG_POLICY"],
        }
    } else if resource.lf_tag_expression.is_some() {
        vec!["LF_NAMED_TAG_EXPRESSION"]
    } else {
        Vec::new()
    }
}

/// What a call whose `Resource` names no resource at all — no member, or no `Resource`
/// where a batch entry may leave it out — is refused with.
pub const RESOURCE_NAMES_NO_TYPE: &str =
    "Resource must name exactly one resource type, but none was provided";

/// Why `resource` cannot name a resource, or `Ok(())` when it names exactly one.
///
/// `Resource` is modelled as a STRUCTURE whose ten members are all optional, but it
/// stands for a choice: every member is a different resource, so naming none leaves the
/// grant with no target and naming two leaves it ambiguous. The two members that are
/// themselves a choice — a table named by `Name` or by `TableWildcard`, and the columns
/// of a `TableWithColumns` named by `ColumnNames` or by `ColumnWildcard` — are checked
/// the same way. The caller is refused with the modelled `InvalidInputException`; the
/// message names the member wherever the service's own wording does not fix the sentence
/// ([`TABLE_NAME_AND_WILDCARD`] does), which is the rule the boundary validator follows
/// for the constraints the model CAN express.
pub fn validate_resource(resource: &Resource) -> Result<(), String> {
    let named: Vec<&str> = [
        ("Catalog", resource.catalog.is_some()),
        ("Database", resource.database.is_some()),
        ("Table", resource.table.is_some()),
        ("TableWithColumns", resource.table_with_columns.is_some()),
        ("DataLocation", resource.data_location.is_some()),
        ("DataCellsFilter", resource.data_cells_filter.is_some()),
        ("LFTag", resource.lf_tag.is_some()),
        ("LFTagPolicy", resource.lf_tag_policy.is_some()),
        ("LFTagExpression", resource.lf_tag_expression.is_some()),
        ("RLSPolicy", resource.rls_policy.is_some()),
    ]
    .into_iter()
    .filter_map(|(name, present)| present.then_some(name))
    .collect();
    match named.len() {
        1 => {}
        0 => return Err(RESOURCE_NAMES_NO_TYPE.to_string()),
        count => {
            return Err(format!(
                "Resource must name exactly one resource type, but {count} were provided: {}",
                named.join(", ")
            ))
        }
    }

    if let Some(table) = &resource.table {
        match (table.name.is_some(), table.table_wildcard.is_some()) {
            // Both members named: the refusal is the service's fixed sentence for the
            // mutually exclusive pair ([`TABLE_NAME_AND_WILDCARD`]), which names neither
            // the table nor the member path.
            (true, true) => return Err(TABLE_NAME_AND_WILDCARD.to_string()),
            (false, false) => {
                return Err("Resource.Table must name either Name or TableWildcard".to_string())
            }
            _ => {}
        }
    }

    if let Some(table) = &resource.table_with_columns {
        // The same rule a data cells filter's columns are held to, so the two cannot
        // drift apart.
        validate_column_choice(
            "Resource.TableWithColumns",
            table.column_names.as_deref(),
            table.column_wildcard.is_some(),
        )?;
    }

    Ok(())
}

/// `resource` with every absent catalog id that the model DOCUMENTS a default for filled
/// in with `catalog_id`.
///
/// "The identifier for the Data Catalog. By default, the account ID." holds for the
/// catalog id INSIDE a resource too (`smithy_query(query="doc",shape="TableResource",
/// member="CatalogId")`: "By default, it is the account ID of the caller"), so a grant
/// on a resource named without one and a grant on the same resource named with the
/// request's own catalog id are the same grant — which only holds if the stored form is
/// normalised before [`resource_key`] renders it.
///
/// Two members are left exactly as the request named them, because the model documents
/// no default for either: `CatalogResource$Id` ("An identifier for the catalog
/// resource" — a catalog resource is named, not defaulted) and nothing else. A
/// `DataCellsFilterResource$TableCatalogId` ("The ID of the catalog to which the table
/// belongs") IS defaulted with the rest: the table it belongs to takes the caller's
/// account as its catalog, so the filter on that table cannot sit in another one, and
/// leaving it absent would make the same filter named with and without the account id
/// two records.
pub fn normalize_resource(resource: &Resource, catalog_id: &str) -> Resource {
    /// Folds a database or table name the way the Data Catalog stores it.
    ///
    /// "For Hive compatibility, this is folded to lowercase when it is stored"
    /// (`DatabaseInput$Name` and `TableInput$Name`), so two spellings of one name are one
    /// entity — and so must be one grant, keyed and reported on the stored name.
    fn fold(name: &mut String) {
        *name = glue_state::fold_name(name);
    }

    let mut normalized = resolve_resource_catalogs(resource, catalog_id);
    if let Some(database) = normalized.database.as_mut() {
        fold(&mut database.name);
    }
    if let Some(table) = normalized.table.as_mut() {
        fold(&mut table.database_name);
        if let Some(name) = table.name.as_mut() {
            fold(name);
        }
    }
    if let Some(table) = normalized.table_with_columns.as_mut() {
        fold(&mut table.database_name);
        fold(&mut table.name);
    }
    if let Some(cells) = normalized.data_cells_filter.as_mut() {
        if let Some(database_name) = cells.database_name.as_mut() {
            fold(database_name);
        }
        if let Some(table_name) = cells.table_name.as_mut() {
            fold(table_name);
        }
    }
    normalized
}

/// `resource` with every absent catalog id the model documents a default for filled in
/// with `catalog_id`, and nothing else changed.
///
/// The first half of [`normalize_resource`], on its own because a resource is also ECHOED
/// back to the caller — a batch reports the entry it could not apply — and an echo RESOLVES
/// what the request left out without folding what the request wrote.
pub fn resolve_resource_catalogs(resource: &Resource, catalog_id: &str) -> Resource {
    /// Fills `slot` with `catalog_id` when it names no catalog.
    fn default_catalog(slot: &mut Option<String>, catalog_id: &str) {
        if slot.as_deref().is_none_or(str::is_empty) {
            *slot = Some(catalog_id.to_string());
        }
    }

    let mut resolved = resource.clone();
    if let Some(database) = resolved.database.as_mut() {
        default_catalog(&mut database.catalog_id, catalog_id);
    }
    if let Some(table) = resolved.table.as_mut() {
        default_catalog(&mut table.catalog_id, catalog_id);
    }
    if let Some(table) = resolved.table_with_columns.as_mut() {
        default_catalog(&mut table.catalog_id, catalog_id);
    }
    if let Some(location) = resolved.data_location.as_mut() {
        default_catalog(&mut location.catalog_id, catalog_id);
    }
    if let Some(cells) = resolved.data_cells_filter.as_mut() {
        default_catalog(&mut cells.table_catalog_id, catalog_id);
    }
    if let Some(tag) = resolved.lf_tag.as_mut() {
        default_catalog(&mut tag.catalog_id, catalog_id);
    }
    if let Some(policy) = resolved.lf_tag_policy.as_mut() {
        default_catalog(&mut policy.catalog_id, catalog_id);
    }
    if let Some(expression) = resolved.lf_tag_expression.as_mut() {
        default_catalog(&mut expression.catalog_id, catalog_id);
    }
    if let Some(policy) = resolved.rls_policy.as_mut() {
        default_catalog(&mut policy.catalog_id, catalog_id);
    }
    resolved
}

/// Appends `value` to `key` length-prefixed (`5:alice`).
///
/// Length-prefixing is what makes a composite key INJECTIVE: no value a request supplies
/// — a table name with a colon, an S3 ARN with a slash — can be read as the next field,
/// so two different resources can never render the same key and share one grant record.
fn push_text(key: &mut String, value: &str) {
    key.push_str(&value.len().to_string());
    key.push(':');
    key.push_str(value);
}

/// Appends an optional value: `-` for absent, `+` and the value for present.
fn push_optional(key: &mut String, value: Option<&str>) {
    match value {
        Some(value) => {
            key.push('+');
            push_text(key, value);
        }
        None => key.push('-'),
    }
}

/// Appends a SET of values: sorted and de-duplicated, so the order a request listed them
/// in does not change which resource the key names.
fn push_set(key: &mut String, values: &[String]) {
    let mut sorted: Vec<&str> = values.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    key.push('[');
    for value in sorted {
        push_text(key, value);
    }
    key.push(']');
}

/// The identity of the resource `resource` names: equal keys are the same resource.
///
/// Built from the members of the one `Resource` member that is set, with the lists the
/// model carries as SETS (a table's columns, an LF-tag's values, an LF-tag policy's
/// expression) rendered order-insensitively — granting on columns `[b, a]` and on
/// `[a, b]` is a grant on the same columns. A table named by `TableWildcard` is its own
/// resource ("every table in the database"), not the table whose name is absent, so it
/// gets its own key.
pub fn resource_key(resource: &Resource) -> String {
    let mut key = String::new();
    if let Some(catalog) = &resource.catalog {
        key.push_str("CATALOG|");
        push_optional(&mut key, catalog.id.as_deref());
    } else if let Some(database) = &resource.database {
        key.push_str("DATABASE|");
        push_optional(&mut key, database.catalog_id.as_deref());
        push_text(&mut key, &database.name);
    } else if let Some(table) = &resource.table {
        key.push_str("TABLE|");
        push_optional(&mut key, table.catalog_id.as_deref());
        push_text(&mut key, &table.database_name);
        match &table.name {
            Some(name) => push_text(&mut key, name),
            None => key.push('*'),
        }
    } else if let Some(table) = &resource.table_with_columns {
        key.push_str("TABLE_WITH_COLUMNS|");
        push_optional(&mut key, table.catalog_id.as_deref());
        push_text(&mut key, &table.database_name);
        push_text(&mut key, &table.name);
        match &table.column_wildcard {
            Some(wildcard) => {
                key.push_str("EXCLUDING");
                push_set(
                    &mut key,
                    wildcard.excluded_column_names.as_deref().unwrap_or_default(),
                );
            }
            None => {
                key.push_str("INCLUDING");
                push_set(&mut key, table.column_names.as_deref().unwrap_or_default());
            }
        }
    } else if let Some(location) = &resource.data_location {
        key.push_str("DATA_LOCATION|");
        push_optional(&mut key, location.catalog_id.as_deref());
        push_text(&mut key, &location.resource_arn);
    } else if let Some(filter) = &resource.data_cells_filter {
        key.push_str("DATA_CELLS_FILTER|");
        push_optional(&mut key, filter.table_catalog_id.as_deref());
        push_optional(&mut key, filter.database_name.as_deref());
        push_optional(&mut key, filter.table_name.as_deref());
        push_optional(&mut key, filter.name.as_deref());
    } else if let Some(tag) = &resource.lf_tag {
        key.push_str("LF_TAG|");
        push_optional(&mut key, tag.catalog_id.as_deref());
        push_text(&mut key, &tag.tag_key);
        push_set(&mut key, &tag.tag_values);
    } else if let Some(policy) = &resource.lf_tag_policy {
        key.push_str("LF_TAG_POLICY|");
        push_optional(&mut key, policy.catalog_id.as_deref());
        push_text(&mut key, &policy.resource_type);
        push_optional(&mut key, policy.expression_name.as_deref());
        let mut expression: Vec<&LFTag> = policy
            .expression
            .as_deref()
            .unwrap_or_default()
            .iter()
            .collect();
        expression.sort_by(|left, right| left.tag_key.cmp(&right.tag_key));
        key.push('{');
        for tag in expression {
            push_text(&mut key, &tag.tag_key);
            push_set(&mut key, &tag.tag_values);
        }
        key.push('}');
    } else if let Some(expression) = &resource.lf_tag_expression {
        key.push_str("LF_TAG_EXPRESSION|");
        push_optional(&mut key, expression.catalog_id.as_deref());
        push_text(&mut key, &expression.name);
    } else if let Some(policy) = &resource.rls_policy {
        key.push_str("RLS_POLICY|");
        push_optional(&mut key, policy.catalog_id.as_deref());
        push_text(&mut key, &policy.name);
    } else {
        // Unreachable through a handler: `validate_resource` refuses a `Resource` naming
        // no member before a grant or a filter is keyed.
        key.push_str("NONE");
    }
    key
}

/// The identity of one grant record: the (catalog, principal, resource, condition) tuple
/// a `GrantPermissions` call addresses.
///
/// Catalog first, then principal, so [`ComAmazonawsLakeformationAwslakeformationState::permission_grants`]
/// enumerates in a stable order a `ListPermissions` page can resume from.
pub fn grant_key(
    catalog_id: &str,
    principal: &str,
    resource: &Resource,
    condition: Option<&str>,
) -> String {
    let mut key = String::new();
    push_text(&mut key, catalog_id);
    push_text(&mut key, principal);
    push_optional(&mut key, condition);
    key.push_str(&resource_key(resource));
    key
}

/// Does the grant on `stored` fall under a listing filtered by the resource `filter`?
///
/// Identity is the base case. Beyond it the model's own documentation fixes two
/// relations, both only for a table named by `Name`:
///
/// * `ListPermissions$Resource` — "This operation does not support getting privileges on
///   a table with columns. Instead, call this operation on the table, and the operation
///   returns the table and the table w columns" — so a table filter also selects the
///   column-level grants on that same table;
/// * `ListPermissions$IncludeRelated` — "Set the field to TRUE to show the cell filters
///   on a table resource" — so with `include_related` a table filter also selects the
///   data-cells-filter grants on that same table.
pub fn resource_filter_matches(filter: &Resource, stored: &Resource, include_related: bool) -> bool {
    if resource_key(filter) == resource_key(stored) {
        return true;
    }
    let Some(table) = filter.table.as_ref() else {
        return false;
    };
    let Some(name) = table.name.as_deref() else {
        return false;
    };
    if let Some(columns) = &stored.table_with_columns {
        return columns.catalog_id == table.catalog_id
            && columns.database_name == table.database_name
            && columns.name == name;
    }
    if include_related {
        if let Some(cells) = &stored.data_cells_filter {
            return cells.table_catalog_id == table.catalog_id
                && cells.database_name.as_deref() == Some(table.database_name.as_str())
                && cells.table_name.as_deref() == Some(name);
        }
    }
    false
}

/// The most permissions one grant or revoke request can name, in either list.
///
/// Grounded in the model, and INHERENT rather than enforced: each member of
/// `PermissionList` targets the `Permission` enum, whose values are
/// [`crate::ops::types::PERMISSION_VALUES`], and a write naming a value the enum does not
/// carry records nothing — it is refused with the service's invalid-permissions sentence
/// ([`unmodelled_permissions`] and [`invalid_permissions`]) before anything is stored
/// (pinned by `permissions.rs::a_permission_the_model_does_not_carry_is_refused_on_both_write_paths`).
/// So once a handler has de-duplicated a list with [`dedup_values`], it can hold at most
/// this many entries, whatever `PermissionList`'s own lack of a length constraint admits
/// on the wire. That is what bounds the set walks below.
///
/// Nothing reads this constant at run time, and nothing should: the bound is a consequence
/// of the enum, so there is no handler-side refusal to fire on it and no allocation to
/// clamp with it. It names the number the walks rely on, and one test asserts it.
pub const MAX_PERMISSIONS_PER_GRANT: usize = crate::ops::types::PERMISSION_VALUES.len();

/// The values `named` carries that the `Permission` enum does not, in first-named order
/// with duplicates dropped: what a grant or a revoke is refused for naming.
///
/// `Permission`'s values are [`crate::ops::types::PERMISSION_VALUES`], and a value outside
/// that set names no permission at all, so neither write path may record it. The
/// comparison is exact — the enum's values are upper-case, and a value either is one of
/// them or is not — and the order is the request's, because the refusal lists the rejected
/// values back to the caller.
///
/// The check lives HERE, in the service, rather than at the request boundary
/// (`skip_validation` on both write paths' inputs in `src/ops/types/`), because Lake
/// Formation answers such a request with the modelled `InvalidInputException` its
/// operations declare, carrying [`invalid_permissions`] — not with the framework's own
/// constraint refusal.
pub fn unmodelled_permissions<'a, I>(named: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a String>,
{
    let modelled: BTreeSet<&str> = crate::ops::types::PERMISSION_VALUES
        .iter()
        .copied()
        .collect();
    dedup_values(
        named
            .into_iter()
            .filter(|value| !modelled.contains(value.as_str())),
    )
}

/// Adds `added` to `held`, keeping it a set in first-granted order.
///
/// The membership test is a `BTreeSet`, so the cost grows with `n log n` rather than with
/// `held` times `added`: both lists are request-supplied, de-duplicated by the caller and
/// so bounded by [`MAX_PERMISSIONS_PER_GRANT`].
pub fn merge_permissions(held: &mut Vec<String>, added: &[String]) {
    let mut present: BTreeSet<&str> = held.iter().map(String::as_str).collect();
    let mut appended: Vec<String> = Vec::new();
    for permission in added {
        if present.insert(permission.as_str()) {
            appended.push(permission.clone());
        }
    }
    held.extend(appended);
}

/// The entries of `requested` that `held` does not carry, in first-named order: what a
/// revoke cannot take away because it was never granted.
///
/// Set-based for the same reason as [`merge_permissions`].
pub fn missing_permissions(held: &[String], requested: &[String]) -> Vec<String> {
    let present: BTreeSet<&str> = held.iter().map(String::as_str).collect();
    let mut named: BTreeSet<&str> = BTreeSet::new();
    let mut missing: Vec<String> = Vec::new();
    for permission in requested {
        if !present.contains(permission.as_str()) && named.insert(permission.as_str()) {
            missing.push(permission.clone());
        }
    }
    missing
}

/// Drops every entry of `removed` from `held`.
///
/// Set-based for the same reason as [`merge_permissions`].
pub fn remove_permissions(held: &mut Vec<String>, removed: &[String]) {
    let dropped: BTreeSet<&str> = removed.iter().map(String::as_str).collect();
    held.retain(|permission| !dropped.contains(permission.as_str()));
}

/// The field separator inside a continuation token's plaintext.
///
/// `U+001F` (unit separator) cannot appear in either field: `CatalogIdString` is
/// `^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$` and `LFTagKey` is
/// `^([\p{L}\p{Z}\p{N}_.:\/=+\-@%]*)$`, neither of which admits a C0 control other than
/// tab — so splitting on it can never cut a value in half.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// The tag identifying a token this service minted, and the token layout's version.
const TOKEN_PREFIX: &str = "lftags";
const TOKEN_VERSION: &str = "1";

/// An opaque continuation token standing for "resume strictly after this (catalog, key)".
///
/// Hex-wrapped so a caller treats it as opaque and round-trips it unchanged, and
/// self-describing so a token this service did not mint is detectable (see
/// [`decode_page_token`]).
pub fn encode_page_token(catalog_id: &str, tag_key: &str) -> String {
    let plain =
        format!("{TOKEN_PREFIX}{TOKEN_SEPARATOR}{TOKEN_VERSION}{TOKEN_SEPARATOR}{catalog_id}{TOKEN_SEPARATOR}{tag_key}");
    hex::encode(plain)
}

/// The `(catalog id, tag key)` exclusive start a token names, or `None` when the token
/// was not minted by [`encode_page_token`] (so the caller must be refused rather than
/// silently served page one).
pub fn decode_page_token(token: &str) -> Option<(String, String)> {
    let bytes = hex::decode(token).ok()?;
    let plain = String::from_utf8(bytes).ok()?;
    let mut fields = plain.split(TOKEN_SEPARATOR);
    let prefix = fields.next()?;
    let version = fields.next()?;
    let catalog_id = fields.next()?;
    let tag_key = fields.next()?;
    if prefix != TOKEN_PREFIX || version != TOKEN_VERSION || fields.next().is_some() {
        return None;
    }
    Some((catalog_id.to_string(), tag_key.to_string()))
}

/// The tag identifying a `ListPermissions` continuation token this service minted. It
/// differs from [`TOKEN_PREFIX`] so a token minted for one paginated operation is not
/// accepted by the other.
const PERMISSIONS_TOKEN_PREFIX: &str = "lfperms";

/// An opaque continuation token standing for "resume strictly after this grant key".
///
/// Hex-wrapped and self-describing for the same reasons as [`encode_page_token`].
pub fn encode_permissions_page_token(grant_key: &str) -> String {
    let plain = format!(
        "{PERMISSIONS_TOKEN_PREFIX}{TOKEN_SEPARATOR}{TOKEN_VERSION}{TOKEN_SEPARATOR}{grant_key}"
    );
    hex::encode(plain)
}

/// The grant key a token names an exclusive start at, or `None` when the token was not
/// minted by [`encode_permissions_page_token`] (so the caller is refused rather than
/// silently served page one).
///
/// The key is the token's LAST field and is taken whole (`splitn`), because a grant key
/// embeds request-supplied text — a `DataLocation` ARN is modelled with no pattern at
/// all — and must survive the round trip byte for byte.
pub fn decode_permissions_page_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    let plain = String::from_utf8(bytes).ok()?;
    let mut fields = plain.splitn(3, TOKEN_SEPARATOR);
    let prefix = fields.next()?;
    let version = fields.next()?;
    let grant_key = fields.next()?;
    if prefix != PERMISSIONS_TOKEN_PREFIX || version != TOKEN_VERSION {
        return None;
    }
    Some(grant_key.to_string())
}

/// The tag identifying a `ListDataCellsFilter` continuation token this service minted. It
/// differs from the prefixes above so a token minted for one paginated operation is not
/// accepted by another.
const FILTERS_TOKEN_PREFIX: &str = "lffilters";

/// An opaque continuation token standing for "resume strictly after this filter key".
///
/// Hex-wrapped and self-describing for the same reasons as [`encode_page_token`].
pub fn encode_filters_page_token(filter_key: &str) -> String {
    let plain = format!(
        "{FILTERS_TOKEN_PREFIX}{TOKEN_SEPARATOR}{TOKEN_VERSION}{TOKEN_SEPARATOR}{filter_key}"
    );
    hex::encode(plain)
}

/// The filter key a token names an exclusive start at, or `None` when the token was not
/// minted by [`encode_filters_page_token`] (so the caller is refused rather than silently
/// served page one).
///
/// The key is the token's LAST field and is taken whole (`splitn`), for the reason
/// [`decode_permissions_page_token`] records: it embeds request-supplied text and must
/// survive the round trip byte for byte.
pub fn decode_filters_page_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    let plain = String::from_utf8(bytes).ok()?;
    let mut fields = plain.splitn(3, TOKEN_SEPARATOR);
    let prefix = fields.next()?;
    let version = fields.next()?;
    let filter_key = fields.next()?;
    if prefix != FILTERS_TOKEN_PREFIX || version != TOKEN_VERSION {
        return None;
    }
    Some(filter_key.to_string())
}

/// The largest page `ListDataCellsFilter` answers, and the page size it uses when the
/// caller names none.
///
/// Grounded in the model: `ListDataCellsFilter$MaxResults` is `range 1..1000`, and the
/// output member `DataCellsFilters` targets `DataCellsFilterList`, which carries no length
/// constraint of its own — so the request's own published maximum is what bounds a page.
pub const MAX_DATA_CELLS_FILTERS_PAGE: i32 = 1000;

/// Why the `Table` a listing names cannot name a table, or `Ok(())` when it names one — the
/// message of the modelled `InvalidInputException` the reader answers with.
///
/// `TableResource` carries the same choice `validate_resource` holds a grant's table to:
/// "At least one of `TableResource$Name` or `TableResource$TableWildcard` is required."
/// (`smithy_query(query="doc",shape="TableResource",member="TableWildcard")`), and naming
/// both names no table ([`TABLE_NAME_AND_WILDCARD`]). Stated here as well as there because
/// a listing names its table OUTSIDE a `Resource`, and the two readings of the one rule are
/// the same sentences.
pub fn validate_listed_table(table: &TableResource) -> Result<(), String> {
    match (table.name.is_some(), table.table_wildcard.is_some()) {
        (true, true) => Err(TABLE_NAME_AND_WILDCARD.to_string()),
        (false, false) => Err("Table must name either Name or TableWildcard".to_string()),
        _ => Ok(()),
    }
}

/// The table permissions Lake Formation records at COLUMN scope.
///
/// `SELECT` is the permission Lake Formation scopes to columns — it is what
/// `TableWithColumnsResource`'s `ColumnNames` / `ColumnWildcard` exist to narrow, and the
/// reason `ListPermissions$Resource` documents that a table read "returns the table and
/// the table w columns": the column-scoped part of a table grant is a record of its own.
/// Every other `Permission` applies to the table as a whole.
///
/// The model names no such set, so this constant is where the set is widened if another
/// permission is ever observed reported at column scope: the recorded behaviour this set
/// is grounded in is a `SELECT` + `DESCRIBE` grant on a table splitting into a table
/// entry carrying `DESCRIBE` and a column-wildcard entry carrying `SELECT`.
/// `ALL` is deliberately NOT in this set: Lake Formation's `ALL` ("Super") is the
/// permission on the resource as a whole, and the recorded split that grounds this set
/// shows only `SELECT` moving to a column-wildcard entry, so a grant naming `ALL` on a
/// table is recorded as the table's own record. Widen the set here — and only on
/// recorded evidence — if `ALL` is ever observed reported at column scope too.
pub const COLUMN_SCOPED_TABLE_PERMISSIONS: [&str; 1] = ["SELECT"];

/// The permission a REVOKE made DIRECTLY on `TableWithColumns` names that cannot be held
/// at column scope, as the message for `InvalidInputException` — `None` when every
/// permission it names can.
///
/// `TableWithColumnsResource` "is only used when granting a SELECT permission"
/// (`smithy_query(query="doc",shape="TableWithColumnsResource")`), so a request that
/// addresses columns and asks for a permission that is not column-scoped
/// ([`COLUMN_SCOPED_TABLE_PERMISSIONS`]) names a scope that permission has no meaning at
/// and is refused rather than applied there. This bites only a request that NAMES
/// `TableWithColumns`: a revoke on the table itself is split by
/// [`scoped_grant_records`], which is what puts the table-scoped permissions on the table.
///
/// The GRANT paths hold a column-scoped request to the same set, as the one instance the
/// model states of the general rule [`inapplicable_permissions`] carries, and answer the
/// service's own sentence for an invalid modification. The model's sentence is about
/// GRANTING, so the revoke side is an extension of it rather than a quotation — one that
/// changes no error class: before it, such a revoke was refused as a permission "the
/// grantee does not have", which is the same modelled `InvalidInputException`.
pub fn column_scope_violation(
    resource: &Resource,
    permissions: &[String],
    permissions_with_grant_option: &[String],
) -> Option<String> {
    resource.table_with_columns.as_ref()?;
    for (member, named) in [
        ("Permissions", permissions),
        ("PermissionsWithGrantOption", permissions_with_grant_option),
    ] {
        if let Some(permission) = named
            .iter()
            .find(|permission| !COLUMN_SCOPED_TABLE_PERMISSIONS.contains(&permission.as_str()))
        {
            return Some(format!(
                "{member} names {permission}, which cannot be granted on a TableWithColumns \
                 resource: that resource is only used when granting a SELECT permission"
            ));
        }
    }
    None
}

/// The records one grant request writes: one per resource SCOPE the requested permissions
/// apply at.
///
/// A grant on a table named by `Name` splits, because Lake Formation holds that table's
/// column-scoped permissions ([`COLUMN_SCOPED_TABLE_PERMISSIONS`]) as a
/// `TableWithColumns` record over every column — the "table w columns" entry
/// `ListPermissions$Resource` documents — and the rest as the table's own record. Every
/// other resource takes the permissions as asked: there is no narrower scope to move them
/// to, and a `TableWildcard` grant names no single table a `TableWithColumns` record
/// could be about.
pub fn scoped_grant_records(
    resource: &Resource,
    permissions: &[String],
    permissions_with_grant_option: &[String],
) -> Vec<(Resource, Vec<String>, Vec<String>)> {
    let scoped = |permissions: &[String], on_columns: bool| -> Vec<String> {
        permissions
            .iter()
            .filter(|permission| {
                COLUMN_SCOPED_TABLE_PERMISSIONS.contains(&permission.as_str()) == on_columns
            })
            .cloned()
            .collect()
    };

    // Only a table named by `Name` has a narrower scope to split into.
    let Some(table) = resource
        .table
        .as_ref()
        .filter(|table| table.name.as_deref().is_some_and(|name| !name.is_empty()))
    else {
        return vec![(
            resource.clone(),
            permissions.to_vec(),
            permissions_with_grant_option.to_vec(),
        )];
    };

    let on_table = (
        scoped(permissions, false),
        scoped(permissions_with_grant_option, false),
    );
    let on_columns = (
        scoped(permissions, true),
        scoped(permissions_with_grant_option, true),
    );

    let mut records = Vec::new();
    if !on_table.0.is_empty() || !on_table.1.is_empty() {
        records.push((resource.clone(), on_table.0, on_table.1));
    }
    if !on_columns.0.is_empty() || !on_columns.1.is_empty() {
        records.push((
            Resource {
                table_with_columns: Some(TableWithColumnsResource {
                    catalog_id: table.catalog_id.clone(),
                    database_name: table.database_name.clone(),
                    name: table.name.clone().unwrap_or_default(),
                    column_names: None,
                    // Every column of the table, with none excluded.
                    column_wildcard: Some(ColumnWildcard {
                        excluded_column_names: None,
                    }),
                }),
                ..Resource::default()
            },
            on_columns.0,
            on_columns.1,
        ));
    }
    records
}

/// The refusal message for the Data Catalog entity a grant names that this emulator does
/// not hold — `None` when every entity it names exists.
///
/// The message is one of the two per-kind constants below, so the refusal says WHICH kind
/// of Data Catalog entity could not be resolved and nothing about the request beyond that.
///
/// Lake Formation grants permissions ON Data Catalog resources, so a grant naming a
/// database or table the catalog does not hold names nothing and is refused. The
/// databases and tables belong to GLUE, so they are read from Glue's own store in the
/// shared engine rather than kept a second time here — the same seam AmplifyBackend's
/// `ListS3Buckets` uses to read S3's buckets.
///
/// `default_catalog` is the Data Catalog the REQUEST addresses, which a resource member
/// that names no `CatalogId` of its own belongs to.
///
/// Call this BEFORE locking this service's store: it takes Glue's lock itself, and never
/// holds the two at the same time.
///
/// Only the catalog's own entities are checked. A resource kind this emulator keeps no
/// store of what EXISTS (a data location, a cell filter, a named tag expression, an RLS
/// policy) cannot be reported absent, so nothing is claimed about it: a refusal has to be
/// grounded in what a store says, never assumed. The same holds for a Data Catalog this
/// emulator holds nothing for — an id naming a catalog in another account, or one no Glue
/// in this process serves (Glue's own [`glue_state::is_known_catalog`] is that
/// predicate): nothing is known about its contents, so no grant on it is refused. An
/// EMPTY catalog this emulator DOES hold is a different matter: it holds no database, so
/// a grant on one is refused — exactly as Glue's own `GetDatabase` answers
/// `EntityNotFoundException` there. `TableWithColumns` column names are not checked
/// either — a table may be stored with no `StorageDescriptor`, so the catalog does not
/// always know its columns.
pub fn missing_grant_entity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    resource: &Resource,
    default_catalog: &str,
) -> Option<String> {
    let in_catalog = |named: Option<&String>| -> String {
        resource_catalog(named.map(String::as_str), default_catalog)
    };

    // The Data Catalog entity a database, table or table-with-columns resource names.
    let (catalog_id, database_name, table_name) = if let Some(database) = &resource.database {
        (
            in_catalog(database.catalog_id.as_ref()),
            database.name.clone(),
            None,
        )
    } else if let Some(table) = &resource.table {
        (
            in_catalog(table.catalog_id.as_ref()),
            table.database_name.clone(),
            // A `TableWildcard` grant is about every table the database holds, so only
            // the database has to exist.
            table.name.clone().filter(|name| !name.is_empty()),
        )
    } else if let Some(table) = &resource.table_with_columns {
        (
            in_catalog(table.catalog_id.as_ref()),
            table.database_name.clone(),
            Some(table.name.clone()),
        )
    } else {
        return None;
    };

    // A catalog id this emulator holds NO Data Catalog under answers nothing: it names
    // one in another account (reachable on the real service only through a Data Catalog
    // resource policy), and a grant is refused only by what a catalog this emulator HOLDS
    // says. `is_known_catalog` is Glue's own predicate for that, so an EMPTY catalog is a
    // catalog that holds nothing — not one nothing is known about.
    if !glue_state::is_known_catalog(&catalog_id) {
        return None;
    }
    // The account's catalog, as GLUE holds it. `service_state` answers `Some` for any
    // `InMemoryEngine` — the engine initialises absent state with `Default`
    // (`vendor/engine/src/lib.rs`) — so an engine serving no Glue answers an EMPTY
    // catalog, which holds no database: a grant on one is refused there too, exactly as
    // Glue's own `GetDatabase` would be. The `?` below is for an engine that is not an
    // `InMemoryEngine` at all, where no store can be reached and nothing is known.
    let catalog = glue_state::service_state(engine)?;
    let catalog = catalog.lock();
    let Some(database) = catalog.database(&catalog_id, &database_name) else {
        return Some(DATABASE_NOT_FOUND.to_string());
    };
    let table_name = table_name?;
    (!database
        .tables
        .contains_key(&glue_state::fold_name(&table_name)))
    .then(|| TABLE_NOT_FOUND.to_string())
}

/// The refusal message for a grant whose `Resource` names a database the Data Catalog
/// does not hold — including a table resource whose `DatabaseName` cannot be resolved,
/// because the database is what is looked up first.
///
/// The refusal that matters on the wire is the HTTP 400 and the `InvalidInputException`
/// code (see [`missing_grant_entity`]'s callers); the real service's message for this
/// condition names only the KIND of entity it could not resolve, never the name the
/// request asked for, and this constant is that message.
pub const DATABASE_NOT_FOUND: &str = "Database not found.";

/// The refusal message for a grant whose `Resource` names a table the Data Catalog does
/// not hold in a database it DOES hold. The per-kind counterpart of
/// [`DATABASE_NOT_FOUND`].
pub const TABLE_NOT_FOUND: &str = "Table not found.";

/// The refusal for a grant whose `Principal` does not name an identity this account's IAM
/// holds — `None` when nothing shows it does not.
///
/// Lake Formation grants permissions TO a principal, so a grant to an identity that does
/// not exist is a grant to nobody: the grantee is resolved against this account's IAM
/// before anything is written. IAM holds an identity under a name AND a path and reports
/// both, so an identifier can fail to name one in two ways, and the refusal says which:
///
/// * IAM holds nothing of that entity type under that name, so the ARN names no identity at
///   all ([`invalid_principal`]);
/// * IAM holds that name under ANOTHER path than the ARN's path portion names, so the ARN
///   names the path rather than the identity ([`invalid_principal_path`]).
///
/// Either way the refusal names the identifier the request carried, and nothing is written.
///
/// The resolution is [`::service_common::iam_principal::resolve_identity`], the tree's
/// one reading of a principal against IAM, so a refusal is grounded in what IAM ANSWERED.
/// Everything else refuses nothing, because nothing about it is known: a value that names
/// no IAM role or user of this account (the [`IAM_ALLOWED_PRINCIPALS`] group, an account id,
/// an identity of ANOTHER account whose IAM this emulator is not the one to ask, a
/// principal of another identity system), an answer this emulator could not read, and
/// an emulator with no IAM mounted at all.
///
/// Called BEFORE this service's own lock is taken: the question travels over the operation
/// callback's in-process transport, so no store is held while it is outstanding.
pub async fn unresolved_grant_principal(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    principal: &str,
) -> Option<String> {
    use ::service_common::iam_principal::IdentityAnswer;

    let region = request_region(ctx.parts());
    match ::service_common::iam_principal::resolve_identity(ctx, &region, principal).await {
        IdentityAnswer::Resolved => None,
        IdentityAnswer::Absent => Some(invalid_principal(principal)),
        IdentityAnswer::AtAnotherPath => Some(invalid_principal_path(principal)),
    }
}

/// The refusal text for a principal IAM does not hold: fixed text, and the one variable
/// part is the `DataLakePrincipalIdentifier` the request named.
pub fn invalid_principal(identifier: &str) -> String {
    format!("Invalid principal, arn: {identifier}")
}

/// The refusal text for a principal whose name IAM holds under another path than the
/// identifier's path portion names: fixed text, and the one variable part is the
/// `DataLakePrincipalIdentifier` the request named.
pub fn invalid_principal_path(identifier: &str) -> String {
    format!("Invalid path for user, arn: {identifier}")
}

/// The region this request was signed for, which the IAM lookup above is addressed to.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region a request carrying no SigV4 credential scope is read as: the AWS default
/// region, which is where an unsigned in-process call is addressed.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLakeformationAwslakeformationState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLakeformationAwslakeformationState>)
}
