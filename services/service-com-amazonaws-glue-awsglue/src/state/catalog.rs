// SPDX-License-Identifier: CC0-1.0

//! The Data Catalog's own records: a catalog, the databases it holds, their tables and
//! partitions, the table versions archived from them, and the projections a read reports
//! each of those through.

use super::*;

/// One Data Catalog: the databases stored under a single `CatalogId`.
#[derive(Debug, Default)]
pub struct Catalog {
    /// Databases keyed by their folded (lowercased) name.
    pub databases: HashMap<String, DatabaseEntry>,
    /// Connection definitions keyed by `ConnectionInput$Name`, in name order.
    pub connections: BTreeMap<String, ConnectionEntry>,
    /// The security configuration `PutDataCatalogEncryptionSettings` last set on this
    /// catalog, as that write carried it, or `None` while no write has set one.
    ///
    /// ONE record per catalog rather than one per resource: every operation that
    /// addresses it names only a `CatalogId`, and the write takes the whole
    /// `DataCatalogEncryptionSettings` structure, so a write REPLACES this record rather
    /// than merging into it. What a read reports from it, defaults included, is
    /// [`ComAmazonawsGlueAwsglueState::data_catalog_encryption_settings`].
    pub encryption_settings: Option<DataCatalogEncryptionSettings>,
    /// The EXPORT configuration a `PutDataCatalogExportConfiguration` last wrote for this
    /// catalog, or `None` while no write has configured one.
    ///
    /// ONE record per catalog, like [`Self::encryption_settings`]: neither operation that
    /// addresses the export configuration carries a `CatalogId` at all, so the account's own
    /// Data Catalog is the whole address and a write REPLACES this record rather than merging
    /// into it. What a read reports from it — including for a catalog that has none — is
    /// [`ComAmazonawsGlueAwsglueState::data_catalog_export_configuration`].
    pub export_configuration: Option<DataCatalogExportConfigurationEntry>,
    /// The MIGRATION of an Athena-managed data catalog into this catalog that an
    /// `ImportCatalogToGlue` performed, or `None` while no such request has migrated one.
    ///
    /// ONE record per catalog, like [`Self::encryption_settings`]: every operation that
    /// starts or reports a migration names only a `CatalogId`, so a later migration of the
    /// same catalog REPLACES this record rather than adding to it. What a read reports from
    /// it is [`ComAmazonawsGlueAwsglueState::catalog_import_status`], which reports the
    /// instant of the migration this record holds for a catalog that has one and the
    /// store's own stamp ([`CatalogStatusStamp`]) for a catalog that has none.
    pub migration: Option<CatalogMigrationEntry>,
    /// The catalog RESOURCE record a `CreateCatalog` wrote for this catalog id, or
    /// `None` for a catalog that is only a container — the account's own Data Catalog,
    /// which no create brought into existence and which therefore has no name,
    /// description or federation of its own.
    ///
    /// It sits on the container rather than in a map of its own so that one catalog is
    /// ONE record: the databases and connections stored under a catalog id and the
    /// metadata of the catalog resource under that same id are read and written through
    /// the same entry.
    pub definition: Option<CatalogDefinition>,
}

impl Catalog {
    /// The stored databases ordered by folded name — the deterministic order
    /// `GetDatabases` pages over, the same order [`DatabaseEntry::tables_ordered`] gives
    /// the tables of one database.
    pub fn databases_ordered(&self) -> Vec<&DatabaseEntry> {
        let mut databases: Vec<&DatabaseEntry> = self.databases.values().collect();
        databases.sort_by(|left, right| left.name.cmp(&right.name));
        databases
    }

    /// Whether this catalog can hold another database, i.e. is below
    /// [`MAX_DATABASES_PER_CATALOG`].
    pub fn can_store_database(&self) -> bool {
        self.databases.len() < MAX_DATABASES_PER_CATALOG
    }

    /// Whether this catalog can hold another connection, i.e. is below
    /// [`MAX_CONNECTIONS_PER_CATALOG`].
    pub fn can_store_connection(&self) -> bool {
        self.connections.len() < MAX_CONNECTIONS_PER_CATALOG
    }
}

/// `CatalogImportStatus$ImportedBy` of a Data Catalog: the service's own marker, reported
/// in place of "the name of the person who initiated the migration" because the migration
/// status of a Data Catalog this service serves is one the SERVICE set — no request it
/// serves carries the person who initiated a migration of an Athena-managed catalog, so
/// there is no such person to report and the caller's identity is never put in their place.
///
/// A value of the member's own `NameString` shape (len 1..255), reported for every catalog
/// whose status is read, whether or not an `ImportCatalogToGlue` has stamped that status.
pub const STATUS_SET_BY_DEFAULT: &str = "StatusSetByDefault";

/// The instant a Data Catalog whose migration status NO import has stamped carries that
/// status from: the store's own instant, taken once when the store is materialised.
///
/// `CatalogImportStatus$ImportTime` — "the time that the migration was started" — is part
/// of every migration status the service reports, so a catalog no import has migrated needs
/// an instant of its own; taking it once, with the store, is what keeps it from moving
/// between two reads of the same catalog.
#[derive(Debug, Clone, Copy)]
pub struct CatalogStatusStamp(SystemTime);

impl CatalogStatusStamp {
    /// The instant this stamp was taken.
    pub fn instant(self) -> SystemTime {
        self.0
    }
}

impl Default for CatalogStatusStamp {
    fn default() -> Self {
        Self(SystemTime::now())
    }
}

/// The migration of an Athena-managed data catalog into one Data Catalog that an
/// `ImportCatalogToGlue` performed: WHEN it ran, which is the instant a later
/// `GetCatalogImportStatus` of that catalog reports beside its completion.
///
/// It records no initiator: the status the service reports names its own marker as the
/// importer ([`STATUS_SET_BY_DEFAULT`]) rather than the identity that asked for the import.
#[derive(Debug, Clone)]
pub struct CatalogMigrationEntry {
    /// `CatalogImportStatus$ImportTime`: "the time that the migration was started" — the
    /// instant the import was served, stored so every later read reports the same
    /// timestamp.
    pub import_time: SystemTime,
}

impl CatalogMigrationEntry {
    /// Record the migration an `ImportCatalogToGlue` served at `now`.
    pub fn from_import(now: SystemTime) -> Self {
        Self { import_time: now }
    }
}

/// The catalog RESOURCE a `CreateCatalog` registered: "a logical grouping of databases in
/// the Glue Data Catalog or a federated source" (`Catalog`), stored under the catalog id
/// it answers to.
///
/// Every member of the modelled `CatalogInput` is held, so a later read reports the
/// definition the client wrote and nothing the server invented, plus the three members
/// the SERVER sets: the two instants the write stamped and the id the catalog is
/// addressed by. The create's `Tags` are held too, as `CatalogInput` does not carry them.
#[derive(Debug, Clone)]
pub struct CatalogDefinition {
    /// `Catalog$Name`: "the name of the catalog", exactly as the create spelled it. The
    /// catalog is KEYED by the folded name ([`fold_name`]), so a read reaches it under
    /// any casing ("This should be all lowercase" — `GetCatalog`), while the reported
    /// name is the one the client chose.
    pub name: String,
    /// `Catalog$CatalogId`: "the ID of the catalog" — the id every other operation
    /// addresses this catalog by, which for a catalog created inside an account's own
    /// Data Catalog is that account's id and the catalog's name
    /// ([`child_catalog_id`]).
    pub catalog_id: String,
    /// `CatalogInput$Description`, stored as sent.
    pub description: Option<String>,
    /// `CatalogInput$Parameters`: "a map array of key-value pairs that define the
    /// parameters and properties of the catalog", stored as sent — an absent map and an
    /// empty one are different definitions, so the absence is kept.
    pub parameters: Option<HashMap<String, String>>,
    /// `CatalogInput$FederatedCatalog`: the entity outside the Glue Data Catalog this
    /// catalog points at, stored as sent.
    pub federated_catalog: Option<FederatedCatalog>,
    /// `CatalogInput$TargetRedshiftCatalog`: the catalog this one links its resources
    /// from, stored as sent.
    pub target_redshift_catalog: Option<TargetRedshiftCatalog>,
    /// `CatalogInput$CatalogProperties`, stored as sent. A read reports it through the
    /// OUTPUT shape of the same properties ([`Self::to_catalog`]).
    pub catalog_properties: Option<CatalogProperties>,
    /// `CatalogInput$CreateTableDefaultPermissions`, stored as sent: an empty list is the
    /// governance decision the doc calls for ("Typically should be explicitly set as an
    /// empty list"), so it is kept apart from a create that named no list at all.
    pub create_table_default_permissions: Option<Vec<PrincipalPermissions>>,
    /// `CatalogInput$CreateDatabaseDefaultPermissions`, stored as sent, for the same
    /// reason as [`Self::create_table_default_permissions`].
    pub create_database_default_permissions: Option<Vec<PrincipalPermissions>>,
    /// `CatalogInput$AllowFullTableExternalDataAccess`, stored as sent.
    pub allow_full_table_external_data_access: Option<String>,
    /// `Catalog$CreateTime`: "the time at which the catalog was created", the instant the
    /// create was served, so every later read reports the same timestamp.
    pub create_time: SystemTime,
    /// `Catalog$UpdateTime`: "the time at which the catalog was last updated" — the
    /// create's own instant until a write changes the definition.
    pub update_time: SystemTime,
    /// The create's `Tags`, which the `Catalog` shape does not carry.
    ///
    /// Kept so the create does not discard what the client sent, but not yet reachable
    /// through the tagging operations: [`TaggedResourceKind`] carries no catalog variant,
    /// and that table is what every tag lookup resolves an ARN against.
    pub tags: HashMap<String, String>,
}

impl CatalogDefinition {
    /// Build a stored catalog from a `CreateCatalog` request served at `now`: `name` as
    /// the client spelled it, `catalog_id` the id the catalog answers to, the definition
    /// from `input`, and both instants from the clock.
    pub fn from_create_input(
        name: &str,
        catalog_id: String,
        input: &CatalogInput,
        tags: Option<&HashMap<String, String>>,
        now: SystemTime,
    ) -> Self {
        Self {
            name: name.to_string(),
            catalog_id,
            description: input.description.clone(),
            parameters: input.parameters.clone(),
            federated_catalog: input.federated_catalog.clone(),
            target_redshift_catalog: input.target_redshift_catalog.clone(),
            catalog_properties: input.catalog_properties.clone(),
            create_table_default_permissions: input.create_table_default_permissions.clone(),
            create_database_default_permissions: input.create_database_default_permissions.clone(),
            allow_full_table_external_data_access: input
                .allow_full_table_external_data_access
                .clone(),
            create_time: now,
            update_time: now,
            tags: tags.cloned().unwrap_or_default(),
        }
    }

    /// Rebuild the modelled `Catalog` shape from what is stored, as served in `region`.
    ///
    /// `ResourceArn` is minted per request rather than stored, like every other ARN this
    /// crate reports: a Glue record carries no Region of its own, so the ARN names the
    /// Region the read was served in ([`catalog_resource_arn`]).
    ///
    /// The members the catalog itself determines are reported DEFINITELY, so a client
    /// never has to read an absent member as "unknown": the catalog's own identifier
    /// ([`Self::catalog_id`]), its kind ([`Self::catalog_type`]) and — empty where nothing
    /// is configured — its properties and both Lake Formation default-permission lists.
    pub fn to_catalog(&self, region: &str) -> crate::ops::types::Catalog {
        crate::ops::types::Catalog {
            catalog_id: Some(self.catalog_id.clone()),
            catalog_identifier: Some(self.catalog_id.clone()),
            catalog_type: Some(self.catalog_type().to_string()),
            name: self.name.clone(),
            resource_arn: Some(catalog_resource_arn(
                region,
                &parse_catalog_reference(&self.catalog_id).account_id,
                &self.name,
            )),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
            create_time: Some(to_date_time(self.create_time)),
            update_time: Some(to_date_time(self.update_time)),
            target_redshift_catalog: self.target_redshift_catalog.clone(),
            federated_catalog: self.federated_catalog.clone(),
            catalog_properties: Some(
                self.catalog_properties
                    .as_ref()
                    .map(|properties| to_catalog_properties_output(properties, self.update_time))
                    .unwrap_or_default(),
            ),
            create_table_default_permissions: Some(reported_default_permissions(
                self.create_table_default_permissions.as_deref(),
            )),
            create_database_default_permissions: Some(reported_default_permissions(
                self.create_database_default_permissions.as_deref(),
            )),
            allow_full_table_external_data_access: self
                .allow_full_table_external_data_access
                .clone(),
        }
    }

    /// `Catalog$CatalogType`: which kind of catalog this record is, read off the source
    /// its definition names.
    ///
    /// A catalog whose definition points at an entity outside the Glue Data Catalog — a
    /// `FederatedCatalog` source or a `TargetRedshiftCatalog` resource link — is
    /// [`CATALOG_TYPE_FEDERATED`]; one whose databases live in the Glue Data Catalog
    /// itself is [`CATALOG_TYPE_NATIVE`].
    pub fn catalog_type(&self) -> &'static str {
        if self.federated_catalog.is_some() || self.target_redshift_catalog.is_some() {
            CATALOG_TYPE_FEDERATED
        } else {
            CATALOG_TYPE_NATIVE
        }
    }
}

/// The Lake Formation default-permission grants a READ reports, from what is stored.
///
/// Every grant is tagged the way the catalog tags a nested structure
/// ([`tagged_principal_permissions`]), and a catalog that configured no grants reports the
/// EMPTY list rather than omitting the member: "no default permissions configured" and
/// "not reported" are different answers, and only the first is true here.
pub(super) fn reported_default_permissions(
    stored: Option<&[PrincipalPermissions]>,
) -> Vec<PrincipalPermissions> {
    stored
        .unwrap_or_default()
        .iter()
        .map(tagged_principal_permissions)
        .collect()
}

/// The stored `CatalogProperties` as a read REPORTS them, in the output shape of the same
/// properties: every member the write carried, plus the members only the server can fill.
///
/// `IcebergOptimizationPropertiesOutput$LastUpdatedTime` is one of those: it is the
/// instant the stored properties were last written, which is the catalog's own update
/// instant. The `DataLakeAccessPropertiesOutput` members that report a MANAGED Redshift
/// Serverless compute ("The managed Redshift Serverless compute name that is created for
/// your catalog resource") stay unset, because this emulator provisions no such compute —
/// reporting a name or a status for one would describe something that does not exist.
pub fn to_catalog_properties_output(
    stored: &CatalogProperties,
    updated: SystemTime,
) -> CatalogPropertiesOutput {
    CatalogPropertiesOutput {
        data_lake_access_properties: stored.data_lake_access_properties.as_ref().map(
            |properties| DataLakeAccessPropertiesOutput {
                data_lake_access: properties.data_lake_access,
                data_transfer_role: properties.data_transfer_role.clone(),
                kms_key: properties.kms_key.clone(),
                catalog_type: properties.catalog_type.clone(),
                managed_workgroup_name: None,
                managed_workgroup_status: None,
                redshift_database_name: None,
                status_message: None,
            },
        ),
        iceberg_optimization_properties: stored.iceberg_optimization_properties.as_ref().map(
            |properties| IcebergOptimizationPropertiesOutput {
                role_arn: properties.role_arn.clone(),
                compaction: properties.compaction.clone(),
                retention: properties.retention.clone(),
                orphan_file_deletion: properties.orphan_file_deletion.clone(),
                last_updated_time: Some(to_date_time(updated)),
            },
        ),
        custom_properties: stored.custom_properties.clone(),
    }
}

/// One resource policy: the policy document attached to a single Glue resource, and when
/// it was attached and last written.
///
/// The record is what every read of that policy reports, so the `PolicyHash` a write
/// answered and the one a later read reports are the same string.
#[derive(Debug, Clone)]
pub struct ResourcePolicyEntry {
    /// `PolicyInJson`: "the requested policy document, in JSON format", exactly as the
    /// write carried it \u2014 a policy is reported as stored, never re-serialised.
    pub policy_in_json: String,
    /// `PolicyHash`: the hash of [`Self::policy_in_json`], from [`policy_hash`].
    pub policy_hash: String,
    /// `CreateTime`: "the date and time at which the policy was created" \u2014 the instant the
    /// FIRST write attached a policy to this resource, kept across later writes.
    pub create_time: SystemTime,
    /// `UpdateTime`: "the date and time at which the policy was last updated".
    pub update_time: SystemTime,
}

impl ResourcePolicyEntry {
    /// A policy attached at `now`, whose create and update instants are therefore the
    /// same.
    pub fn new(policy_in_json: String, now: SystemTime) -> Self {
        Self {
            policy_hash: policy_hash(&policy_in_json),
            policy_in_json,
            create_time: now,
            update_time: now,
        }
    }

    /// Replace this record's document with the one a later write carried, at `now`.
    ///
    /// The document is replaced whole and the hash recomputed from it, while
    /// [`Self::create_time`] stays where the first write put it: the policy is the same
    /// attachment, rewritten.
    pub fn replace_document(&mut self, policy_in_json: String, now: SystemTime) {
        self.policy_hash = policy_hash(&policy_in_json);
        self.policy_in_json = policy_in_json;
        self.update_time = now;
    }
}

/// The `PolicyHash` of a policy document: the SHA-256 of the document, lowercase
/// hexadecimal.
///
/// Derived from the document's CONTENT, which is what the member is for: it is "the hash
/// value returned when the previous policy was set using `PutResourcePolicy`", whose
/// "purpose is to prevent concurrent modifications of a policy"
/// (`PutResourcePolicyRequest$PolicyHashCondition`) \u2014 so the same document always hashes
/// to the same value, and a document changed in any way hashes to a different one. Glue
/// publishes no digest for it, so the algorithm is this emulator's; the 64 hexadecimal
/// characters are inside `HashString`'s `length 1..255` and match its pattern.
///
/// This is not a per-run value and not a credential: it is computed from the request's
/// own document, so two runs that attach the same policy report the same hash.
pub fn policy_hash(policy_in_json: &str) -> String {
    use ::sha2::Digest;

    ::hex::encode(::sha2::Sha256::digest(policy_in_json.as_bytes()))
}

/// The resource whose policy a resource-policy request addresses: the `ResourceArn` it
/// names, else this account's Data Catalog ("If not supplied, the Data Catalog resource
/// policy is returned" \u2014 `GetResourcePolicyRequest$ResourceArn`).
///
/// `Err` carries the sentence for a `ResourceArn` this endpoint can hold no policy for \u2014
/// one that is not a well-formed Glue ARN, or one naming another account's or another
/// Region's estate, whose policies this process does not hold.
///
/// The ONE resolution all three resource-policy operations go through, so a policy
/// written under one spelling of its resource is read back under the other.
pub fn resolve_policy_resource_arn(
    resource_arn: Option<&String>,
    region: &str,
) -> Result<String, String> {
    let Some(named) = resource_arn.map(String::as_str).filter(|arn| !arn.is_empty()) else {
        // The catalog of the account this endpoint serves, which is the one a request
        // that names no catalog addresses ([`resolve_catalog_id`]).
        return Ok(catalog_arn(region, &resolve_catalog_id(None)));
    };
    match parse_glue_arn(named) {
        Some(parsed) if parsed.is_local(region) => Ok(named.to_string()),
        _ => Err(messages::invalid_policy_resource_arn(named)),
    }
}

/// Whether `resource_arn` names a Data Catalog ITSELF rather than a resource inside one:
/// its resource portion is [`CATALOG_ARN_RESOURCE`] alone, with no name after it.
///
/// This is what separates the ACCOUNT-LEVEL Data Catalog policy — the one a
/// `PutResourcePolicy` that names no `ResourceArn` sets — from a policy attached to an
/// individual resource, which is the distinction `GetResourcePolicies` enumerates by.
pub fn names_a_catalog(resource_arn: &str) -> bool {
    parse_glue_arn(resource_arn).is_some_and(|parsed| parsed.resource == CATALOG_ARN_RESOURCE)
}

/// A stored database, plus the tables it contains.
#[derive(Debug, Clone)]
pub struct DatabaseEntry {
    /// The folded name, which is also this entry's key in [`Catalog::databases`].
    pub name: String,
    pub description: Option<String>,
    pub location_uri: Option<String>,
    pub parameters: Option<HashMap<String, String>>,
    /// The Lake Formation default grants for tables created in this database. Always a
    /// definite list: the create either sent one (an empty list included) or the
    /// catalog's default from [`default_create_table_default_permissions`] was recorded.
    pub create_table_default_permissions: Vec<PrincipalPermissions>,
    pub target_database: Option<DatabaseIdentifier>,
    pub federated_database: Option<FederatedDatabase>,
    pub catalog_id: String,
    /// The opaque id the catalog assigned this database at create
    /// ([`new_database_id`]). Server-set and written once, so every later read of this
    /// database reports the same id and the tables it holds report it too — an update
    /// that renames the database does not change it, because it identifies the
    /// database rather than naming it.
    pub database_id: String,
    /// Server-set, written once at create so every later read reports the same instant.
    pub create_time: SystemTime,
    pub tags: HashMap<String, String>,
    /// Tables keyed by their folded (lowercased) name.
    pub tables: HashMap<String, TableEntry>,
    /// The key material the write that last stored this database was encrypted under, or
    /// `None` when that write was encrypted under no key of the caller's.
    ///
    /// Recorded per OBJECT because the catalog's setting applies "to every catalog write
    /// thereafter": a later change to it leaves this database encrypted under the key its
    /// own write used. See [`catalog_encryption`].
    pub encryption: Option<ObjectEncryption>,
}

impl DatabaseEntry {
    /// Whether this database can hold another table, i.e. is below
    /// [`MAX_TABLES_PER_DATABASE`].
    pub fn can_store_table(&self) -> bool {
        self.tables.len() < MAX_TABLES_PER_DATABASE
    }

    /// Build a stored database from the client's `DatabaseInput`.
    pub fn from_input(
        input: &DatabaseInput,
        catalog_id: &str,
        tags: Option<&HashMap<String, String>>,
        now: SystemTime,
    ) -> Self {
        Self {
            name: fold_name(&input.name),
            description: input.description.clone(),
            location_uri: input.location_uri.clone(),
            parameters: input.parameters.clone(),
            // Recorded at create, not filled in per read, so every later read of this
            // database reports the same grants.
            create_table_default_permissions: input
                .create_table_default_permissions
                .clone()
                .unwrap_or_else(default_create_table_default_permissions),
            target_database: input.target_database.clone(),
            federated_database: input.federated_database.clone(),
            catalog_id: catalog_id.to_string(),
            database_id: new_database_id(),
            create_time: now,
            tags: tags.cloned().unwrap_or_default(),
            tables: HashMap::new(),
            // The key material is not in any `DatabaseInput`: it is what the catalog's
            // own encryption setting had KMS produce for this write, so the handler
            // records it on the entry it has just built.
            encryption: None,
        }
    }

    /// Rebuild the modelled `Database` output shape from what is stored.
    pub fn to_database(&self) -> Database {
        Database {
            name: self.name.clone(),
            description: self.description.clone(),
            location_uri: self.location_uri.clone(),
            parameters: self.parameters.clone(),
            create_time: Some(to_date_time(self.create_time)),
            create_table_default_permissions: Some(
                self.create_table_default_permissions
                    .iter()
                    .map(tagged_principal_permissions)
                    .collect(),
            ),
            target_database: self.target_database.clone(),
            catalog_id: Some(self.catalog_id.clone()),
            federated_database: self.federated_database.clone(),
            // The catalog attributes every database it returns to the catalog that owns
            // it and to the id it assigned the database, both reported DEFINITELY: a
            // client builds an ARN or correlates the database across Lake Formation and
            // Athena from these, so an absent member would leave it with nothing to key
            // on. Both come from what is stored, so repeated reads agree.
            catalog_identifier: Some(self.catalog_id.clone()),
            database_id: Some(self.database_id.clone()),
        }
    }

    /// Overwrite this database's definition with the one the client submitted.
    ///
    /// `DatabaseInput` is the WHOLE client-settable definition of a database — the
    /// same shape a create submits — so an update writes every member it carries and
    /// clears the ones it omits; what the client sent is exactly what the next read
    /// reports. The members no `DatabaseInput` carries are left alone, because the
    /// client cannot set them: `CatalogId` and `CreateTime` belong to the server, the
    /// tags are written through the tagging operations, and the tables this database
    /// owns are their own resources.
    ///
    /// The submitted `Name` is folded and written like any other member, so a request
    /// that submits a different one has MOVED the database: its tables' reported
    /// `DatabaseName` moves with it. Relocating the entry under its new key is the
    /// caller's job, since the key lives in [`Catalog::databases`].
    pub fn apply_input(&mut self, input: &DatabaseInput) {
        let folded = fold_name(&input.name);
        for table in self.tables.values_mut() {
            table.database_name = folded.clone();
        }
        self.name = folded;
        self.description = input.description.clone();
        self.location_uri = input.location_uri.clone();
        self.parameters = input.parameters.clone();
        self.create_table_default_permissions = input
            .create_table_default_permissions
            .clone()
            // An update that names no grants records the catalog's default, exactly as
            // a create does: the stored list is always definite, and an EMPTY list is
            // the opposite governance decision, so it is stored only when sent.
            .unwrap_or_else(default_create_table_default_permissions);
        self.target_database = input.target_database.clone();
        self.federated_database = input.federated_database.clone();
    }

    /// The stored tables ordered by folded name — the deterministic order
    /// `GetTables` pages over.
    pub fn tables_ordered(&self) -> Vec<&TableEntry> {
        let mut tables: Vec<&TableEntry> = self.tables.values().collect();
        tables.sort_by(|left, right| left.name.cmp(&right.name));
        tables
    }
}

/// A stored table inside its parent database.
#[derive(Debug, Clone)]
pub struct TableEntry {
    /// The folded name, which is also this entry's key in [`DatabaseEntry::tables`].
    pub name: String,
    pub database_name: String,
    pub catalog_id: String,
    pub description: Option<String>,
    pub owner: Option<String>,
    pub table_type: Option<String>,
    pub retention: i32,
    pub parameters: Option<HashMap<String, String>>,
    pub storage_descriptor: Option<StorageDescriptor>,
    pub partition_keys: Option<Vec<Column>>,
    pub view_original_text: Option<String>,
    pub view_expanded_text: Option<String>,
    pub target_table: Option<TableIdentifier>,
    pub federated_table: Option<FederatedTable>,
    /// Client-supplied, echoed back exactly as sent.
    pub last_access_time: Option<::aws_smithy_types::DateTime>,
    /// Client-supplied, echoed back exactly as sent.
    pub last_analyzed_time: Option<::aws_smithy_types::DateTime>,
    /// Server-set, written once at create.
    pub create_time: SystemTime,
    /// Server-set; moves with every write to this table.
    pub update_time: SystemTime,
    /// Server-set version counter, [`FIRST_TABLE_VERSION_ID`] for a freshly created
    /// table.
    pub version_id: String,
    /// Server-set: the principal that issued the create.
    pub created_by: Option<String>,
    /// The versions of this table the catalog has ARCHIVED, keyed by the counter of the
    /// `VersionId` each one was current under.
    ///
    /// "By default, `UpdateTable` always creates an archived version of the table before
    /// updating it" (`UpdateTableRequest$SkipArchive`), so a write that moves
    /// [`Self::version_id`] leaves the definition it replaced in here. The CURRENT
    /// version is NOT in this map — it is the live definition above, which is itself the
    /// version [`Self::version_id`] names — so a table that has never been updated has
    /// exactly one version and an empty archive.
    pub archived_versions: BTreeMap<u64, ArchivedTableVersion>,
    /// The PARTITIONS of this table — "a slice of table data" (`Partition`) — keyed by
    /// the `Values` that address one.
    ///
    /// A partition lives on its table rather than beside the catalog tree because every
    /// operation that addresses one names its catalog, database and table and then the
    /// partition's values (`GetPartition$PartitionValues`), so the values identify a
    /// partition only WITHIN one table. The key is the value list exactly as sent: the
    /// model documents the Hive case fold for a database and a table name and for
    /// neither a partition key nor a partition value, and a value is table DATA, which a
    /// fold would rewrite.
    ///
    /// Ordered, so a reader of a table's partitions has one deterministic order to walk
    /// — lexicographic by value list, which for the partition-key order the values are
    /// passed in ("ordered in the same order as the partition keys",
    /// `PartitionInput$Values`) groups a table's partitions by their first key.
    pub partitions: BTreeMap<Vec<String>, PartitionEntry>,
    /// The key material the write that last stored this table's definition was encrypted
    /// under, or `None` when that write was encrypted under no key of the caller's. See
    /// [`DatabaseEntry::encryption`].
    pub encryption: Option<ObjectEncryption>,
    /// The COLUMN STATISTICS TASK SETTINGS of this table — "the settings for a column
    /// statistics task" (`ColumnStatisticsTaskSettings`) — or `None` while no create has
    /// written them.
    ///
    /// On the table rather than beside the catalog tree, exactly as a partition is:
    /// every operation that addresses one settings record names its catalog, its database
    /// and then its table (`GetColumnStatisticsTaskSettings$TableName`) and nothing else,
    /// so the record is addressed only WITHIN one table and there is at most one of it per
    /// table. That is also why a second create under the same table is the duplicate
    /// [`messages::COLUMN_STATISTICS_TASK_SETTINGS_ALREADY_EXIST`] reports rather than an
    /// overwrite, and why dropping the table drops its settings with it.
    pub column_statistics_task_settings: Option<ColumnStatisticsTaskSettingsEntry>,
    /// The COLUMN STATISTICS of this table — "the generated column-level statistics for a
    /// table" (`ColumnStatistics`) — keyed by the folded name of the column each entry
    /// belongs to.
    ///
    /// On the table rather than beside the catalog tree, exactly as its column statistics
    /// settings are: every operation that addresses them names its catalog, its database,
    /// its table and then the COLUMNS (`GetColumnStatisticsForTable$ColumnNames`), so a
    /// column's statistics are addressed only WITHIN one table and there is at most one
    /// entry per column — which is why a later run that covers the same column replaces
    /// what the earlier one generated, and why dropping the table drops its statistics with
    /// it. The key is folded, because the catalog addresses a column name without regard to
    /// case; the declared spelling travels inside the entry. Ordered, so a reader has one
    /// deterministic order.
    ///
    /// Written by the statistics WRITE of a table (`UpdateColumnStatisticsForTable`, which
    /// "creates or updates table statistics of columns"), and when a column statistics task
    /// run SETTLES
    /// ([`ComAmazonawsGlueAwsglueState::settle_due_column_statistics_task_runs`]): the
    /// statistics a table carries are the ones a client stored for it or a run computed from
    /// the data behind it, so a table neither has happened to carries none. Reached through
    /// [`stored_column_statistics`], [`store_column_statistics`] and
    /// [`remove_column_statistics`], so one keying rule serves every operation that
    /// addresses them.
    pub column_statistics: BTreeMap<String, ColumnStatistics>,
    /// The TABLE OPTIMIZERS of this table — "an optimizer associated with a table"
    /// (`TableOptimizer`) — keyed by the `TableOptimizerType` each one is of.
    ///
    /// On the table rather than beside the catalog tree, exactly as its column statistics
    /// settings are: every operation that addresses an optimizer names its catalog, its
    /// database, its table and then the TYPE (`GetTableOptimizerRequest$Type`), so there is
    /// at most one optimizer of each type per table — which is why a second create of the
    /// same type is a duplicate rather than an overwrite, and why dropping the table drops
    /// its optimizers with it. Ordered by type, so a reader has one deterministic order.
    pub optimizers: BTreeMap<String, TableOptimizerEntry>,
}

/// One stored table optimizer: the configuration a create or update wrote for one
/// `TableOptimizerType` of one table.
///
/// `lastRun` is absent on every stored optimizer: a run is work an optimizer performs over
/// the table's data files, and nothing in this emulator performs it, so there is no run to
/// report ([`TableOptimizerEntry::to_table_optimizer`]).
#[derive(Debug, Clone)]
pub struct TableOptimizerEntry {
    /// The optimizer's type, which is also this entry's key in [`TableEntry::optimizers`].
    pub r#type: String,
    /// "A `TableOptimizerConfiguration` object that was specified when creating or
    /// updating a table optimizer" (`TableOptimizer$configuration`), stored exactly as
    /// sent so a read reports the configuration this write recorded.
    pub configuration: TableOptimizerConfiguration,
}

impl TableOptimizerEntry {
    /// Build a stored optimizer from the configuration a create submitted for `r#type`.
    pub fn new(r#type: &str, configuration: &TableOptimizerConfiguration) -> Self {
        Self {
            r#type: r#type.to_string(),
            configuration: configuration.clone(),
        }
    }

    /// This optimizer in the `TableOptimizer` shape every read of one answers with.
    pub fn to_table_optimizer(&self) -> TableOptimizer {
        TableOptimizer {
            r#type: Some(self.r#type.clone()),
            configuration: Some(self.configuration.clone()),
            // No run: nothing here compacts data files, expires snapshots or deletes
            // orphan files, so reporting a run would report work nothing did.
            last_run: None,
            // `configurationSource` "indicates how the table optimizer was configured and
            // which entity or service initiated the configuration"; the service publishes
            // no rule mapping a create to one of its two values, so none is reported.
            configuration_source: None,
        }
    }
}

/// The table parameter whose value marks a stored table as an open table format, and the
/// value an Iceberg table carries in it. Set by a `CreateTable` that names an
/// `OpenTableFormatInput`, and read back by every later read of the table, which is how a
/// client — and a service that consumes Glue tables — tells an Iceberg table from a
/// Hive-style one.
pub const ICEBERG_TABLE_TYPE_PARAMETER: &str = "table_type";
/// See [`ICEBERG_TABLE_TYPE_PARAMETER`]: the value an Iceberg table carries in it.
pub const ICEBERG_TABLE_TYPE: &str = "ICEBERG";
/// The table parameter carrying the location of an Iceberg table's current metadata file,
/// which is what an Iceberg client reads a table's schema and snapshots from.
pub const ICEBERG_METADATA_LOCATION_PARAMETER: &str = "metadata_location";
/// The table parameter carrying an Iceberg table's format version — the property Iceberg
/// itself names that version under, which is where the version a create asked for
/// (`IcebergInput$Version`) is observable afterwards.
pub const ICEBERG_FORMAT_VERSION_PARAMETER: &str = "format-version";
/// The Iceberg format version a create that names none asks for: "The table version for
/// the Iceberg table. Defaults to 2" (`IcebergInput$Version`).
pub const DEFAULT_ICEBERG_FORMAT_VERSION: &str = "2";
/// The directory, under an Iceberg table's own location, that its metadata files live in,
/// as Iceberg lays a table out.
pub(super) const ICEBERG_METADATA_DIRECTORY: &str = "metadata";
/// How many 64-bit words the random part of an Iceberg metadata file name carries: two,
/// i.e. the 128 bits a UUID is, which is what Iceberg names a metadata file with.
pub(super) const ICEBERG_METADATA_ID_WORDS: usize = 2;

/// How many Iceberg metadata file names this process has minted. A nonce source for
/// [`new_iceberg_metadata_location`], exactly as [`MINTED_DATABASE_IDS`] is for a database
/// id.
pub(super) static MINTED_ICEBERG_METADATA_FILES: AtomicU64 = AtomicU64::new(0);

/// The Iceberg metadata a create of an open-table-format table mints for it, and records
/// on the stored table so every later read serves the same metadata location.
///
/// A create "mints the Iceberg metadata" for the table: the first metadata file of a table
/// whose history is one commit long, under the table's own location.
#[derive(Debug, Clone)]
pub struct IcebergTableMarker {
    /// Where the table's current metadata file lives
    /// ([`new_iceberg_metadata_location`]).
    pub metadata_location: String,
    /// The Iceberg format version the create asked for, else
    /// [`DEFAULT_ICEBERG_FORMAT_VERSION`].
    pub format_version: String,
}

impl IcebergTableMarker {
    /// Mint the metadata of an Iceberg table stored at `location`, at the format `version`
    /// the create named.
    pub fn mint(location: &str, version: Option<&str>) -> Self {
        Self {
            metadata_location: new_iceberg_metadata_location(location),
            format_version: version
                .unwrap_or(DEFAULT_ICEBERG_FORMAT_VERSION)
                .to_string(),
        }
    }

    /// Record this metadata on the `TableInput` about to be stored, so the stored table —
    /// and every read of it — carries the three parameters that make it an Iceberg table.
    ///
    /// The client's own parameters are kept: only the three the service sets are written
    /// over, because they describe what the create minted rather than what it asked for.
    pub fn apply(&self, input: &mut TableInput) {
        let parameters = input.parameters.get_or_insert_with(HashMap::new);
        parameters.insert(
            ICEBERG_TABLE_TYPE_PARAMETER.to_string(),
            ICEBERG_TABLE_TYPE.to_string(),
        );
        parameters.insert(
            ICEBERG_METADATA_LOCATION_PARAMETER.to_string(),
            self.metadata_location.clone(),
        );
        parameters.insert(
            ICEBERG_FORMAT_VERSION_PARAMETER.to_string(),
            self.format_version.clone(),
        );
    }
}

/// The location of the FIRST metadata file of an Iceberg table stored at `location`.
///
/// Iceberg lays a table's metadata out under the table's own location, in a file named by
/// the commit it records and a random id —
/// `<location>/metadata/00000-<id>.metadata.json` for the first commit, which is the one a
/// create records. The id is 128 bits from [`mint_hex_id`], spelled the way a UUID is, so
/// two tables created in the same nanosecond cannot be given the same metadata file.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_iceberg_metadata_location(location: &str) -> String {
    let base = location.trim_end_matches('/');
    let digits = mint_hex_id(&MINTED_ICEBERG_METADATA_FILES, ICEBERG_METADATA_ID_WORDS);
    // The 32 hexadecimal characters grouped 8-4-4-4-12, the way a UUID is spelled. Built
    // by walking the characters rather than by slicing, so the spelling cannot depend on
    // the id's width.
    let mut id = String::with_capacity(digits.len() + 4);
    for (position, character) in digits.chars().enumerate() {
        if matches!(position, 8 | 12 | 16 | 20) {
            id.push('-');
        }
        id.push(character);
    }
    format!("{base}/{ICEBERG_METADATA_DIRECTORY}/00000-{id}.metadata.json")
}

/// The Hive-style table definition an Iceberg create describes, for a request that carries
/// no `TableInput` of its own.
///
/// `CreateIcebergTableInput` is "the configuration parameters required to create a new
/// Iceberg table": it names the table's location, its schema and its table properties,
/// which with the request's `Name` is everything a stored table needs.
pub fn iceberg_table_definition(name: &str, create: &CreateIcebergTableInput) -> TableInput {
    TableInput {
        name: name.to_string(),
        storage_descriptor: Some(StorageDescriptor {
            columns: Some(iceberg_schema_columns(&create.schema)),
            location: Some(create.location.clone()),
            ..StorageDescriptor::default()
        }),
        parameters: create.properties.clone(),
        ..TableInput::default()
    }
}

/// One catalog `Column` per field of an Iceberg schema, in the order the schema declares
/// them: a field's `Name` is the column's name, its `Doc` the column's comment.
///
/// A field's `Type` is an Iceberg type document. A PRIMITIVE Iceberg type is spelled as a
/// string (`"long"`, `"string"`), which is the column type; a nested one is an object, and
/// this slice maps no nested Iceberg type onto a Hive column type, so such a column is
/// reported with its name and no type rather than with a type nothing produced.
pub(super) fn iceberg_schema_columns(schema: &IcebergSchema) -> Vec<Column> {
    schema
        .fields
        .iter()
        .map(|field| Column {
            name: field.name.clone(),
            r#type: match &field.r#type {
                ::aws_smithy_types::Document::String(spelling) => Some(spelling.clone()),
                _ => None,
            },
            comment: field.doc.clone(),
            parameters: None,
        })
        .collect()
}

/// Where an Iceberg table being created stores its data: the location its `TableInput`
/// names, else the one its `CreateIcebergTableInput` names, and `None` when neither does.
///
/// The table's metadata is minted UNDER that location
/// ([`new_iceberg_metadata_location`]), so a create that names none has nowhere to put the
/// metadata the table is read through.
pub fn iceberg_table_location(
    definition: &TableInput,
    create: Option<&CreateIcebergTableInput>,
) -> Option<String> {
    definition
        .storage_descriptor
        .as_ref()
        .and_then(|descriptor| descriptor.location.clone())
        .or_else(|| create.map(|create| create.location.clone()))
        .filter(|location| !location.is_empty())
}

/// Why a read that has to resolve a table's metadata through the source a FEDERATED
/// database points at cannot be served, as the message of the modelled
/// `FederationSourceRetryableException` — or `None` for a database whose metadata the
/// catalog itself holds.
///
/// "A database that points to an entity outside the Glue Data Catalog"
/// (`FederatedDatabase`) keeps its tables in that entity, reached through the connection
/// its registration names, so a read of one of its tables is answered by the SOURCE and not
/// out of the catalog. This emulator connects to no such source — a registration is
/// accepted, and nothing can reach what it points at — so that read fails the way a source
/// that is there but unreachable fails: retryably, which is the code that tells a client to
/// retry rather than give up.
pub fn federated_source_refusal(database: &DatabaseEntry) -> Option<String> {
    let federated = database.federated_database.as_ref()?;
    let source = federated
        .connection_name
        .clone()
        .or_else(|| federated.identifier.clone())
        .unwrap_or_default();
    Some(messages::federated_source_unreachable(
        &database.name,
        &source,
    ))
}

/// Why a partition could not be created on a table.
///
/// One refusal reason per way a create can fail ON the table it addresses, so the
/// single-partition create and the batch create make the SAME decision: the batch
/// reports it as a per-item [`ErrorDetail`] inside `PartitionError`, where "the errors
/// encountered when trying to create the requested partitions" belong, and a
/// single-partition create raises the matching modelled exception named by
/// [`Self::error_code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionCreateRefusal {
    /// The `PartitionInput` named no partition value at all. "Although this parameter is
    /// not required by the SDK, you must specify this parameter for a valid input"
    /// (`PartitionInput$Values`), so an input without values is not a partition the
    /// catalog can address.
    ValuesRequired,
    /// The values do not line up with the table's partition keys: a partition is one
    /// value per key of the table it belongs to, passed "in the same order as the
    /// partition keys" (`PartitionInput$Values`), so a list of another length cannot
    /// name a slice of this table.
    ValuesDoNotMatchPartitionKeys {
        /// How many partition keys the addressed table defines.
        keys: usize,
        /// How many values the input carried.
        values: usize,
    },
    /// The table already holds a partition with those values.
    AlreadyExists,
    /// The store cannot hold another partition ([`MAX_PARTITIONS_PER_TABLE`],
    /// [`MAX_STORED_PARTITIONS`]).
    LimitExceeded {
        /// The ceiling that was reached.
        limit: usize,
    },
}

impl PartitionCreateRefusal {
    /// The modelled error shape this refusal is reported under: the `ErrorCode` of a
    /// batch's per-item [`ErrorDetail`], and the exception a single-partition create
    /// raises.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::ValuesRequired | Self::ValuesDoNotMatchPartitionKeys { .. } => {
                "InvalidInputException"
            }
            Self::AlreadyExists => "AlreadyExistsException",
            Self::LimitExceeded { .. } => "ResourceNumberLimitExceededException",
        }
    }

    /// This refusal's wording. See [`messages`] for how this crate's unrecorded
    /// refusals are worded.
    pub fn message(&self) -> String {
        match self {
            Self::ValuesRequired => messages::PARTITION_VALUES_REQUIRED.to_string(),
            Self::ValuesDoNotMatchPartitionKeys { keys, values } => {
                messages::partition_values_do_not_match_keys(*keys, *values)
            }
            Self::AlreadyExists => messages::PARTITION_ALREADY_EXISTS.to_string(),
            Self::LimitExceeded { limit } => {
                format!("the Data Catalog already holds the maximum of {limit} partitions")
            }
        }
    }

    /// This refusal as the per-item error shape a batch reports it in.
    pub fn to_error_detail(self) -> ErrorDetail {
        catalog_error_detail(self.error_code(), self.message())
    }
}

/// Why a partition of a table could not be updated.
///
/// One refusal reason per way an update can fail ON the table it addresses, so the
/// single-partition update and each item of a batch update accept and refuse exactly the
/// same entries: the batch reports it as a per-item [`ErrorDetail`] inside
/// `BatchUpdatePartitionFailureEntry` — "the errors encountered when trying to update the
/// requested partitions" — and a single-partition update raises the matching modelled
/// exception named by [`Self::error_code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionUpdateRefusal {
    /// The submitted `PartitionInput` named no partition value at all. "Although this
    /// parameter is not required by the SDK, you must specify this parameter for a valid
    /// input" (`PartitionInput$Values`), the same requirement a create is held to.
    ValuesRequired,
    /// The submitted `PartitionInput` named values other than the ones the entry
    /// addresses. "The Values property can't be changed. If you want to change the
    /// partition key values for a partition, delete and recreate the partition"
    /// (`UpdatePartitionRequest$PartitionInput`), so an update cannot move a partition to
    /// another address.
    ValuesCannotBeChanged,
    /// The addressed table holds no partition at those values, so there is nothing to
    /// update.
    NotFound,
}

impl PartitionUpdateRefusal {
    /// The modelled error shape this refusal is reported under: the `ErrorCode` of a
    /// batch's per-item [`ErrorDetail`], and the exception a single-partition update
    /// raises.
    pub fn error_code(self) -> &'static str {
        match self {
            Self::ValuesRequired | Self::ValuesCannotBeChanged => "InvalidInputException",
            Self::NotFound => "EntityNotFoundException",
        }
    }

    /// This refusal's wording, over the values the request addressed the partition by. See
    /// [`messages`] for how this crate's refusals are worded.
    ///
    /// The miss names the partition it could not find
    /// ([`messages::partition_to_update_not_found`]), which is a different sentence from the
    /// one a partition READ answers ([`messages::PARTITION_NOT_FOUND`]): a mutation of an
    /// absent partition and a read of one are worded as this service words each.
    pub fn message(self, values: &[String]) -> String {
        match self {
            Self::ValuesRequired => messages::PARTITION_VALUES_REQUIRED.to_string(),
            Self::ValuesCannotBeChanged => {
                messages::PARTITION_VALUES_CANNOT_BE_CHANGED.to_string()
            }
            Self::NotFound => messages::partition_to_update_not_found(values),
        }
    }

    /// This refusal as the per-item error shape a batch reports it in.
    pub fn to_error_detail(self, values: &[String]) -> ErrorDetail {
        catalog_error_detail(self.error_code(), self.message(values))
    }
}

/// Why a partition of a table could not be deleted.
///
/// One refusal reason per way a delete can fail ON the table it addresses, so the
/// single-partition delete and each item of a batch delete accept and refuse exactly the
/// same value lists: the batch reports it as a per-item [`ErrorDetail`] inside
/// `PartitionError` — "the errors encountered when trying to delete the requested
/// partitions" — and a single-partition delete raises the matching modelled exception
/// named by [`Self::error_code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionDeleteRefusal {
    /// The addressed table holds no partition at those values, so there is nothing to
    /// delete. A delete addresses a partition by the values it was created under
    /// (`PartitionValueList$Values`), matched exactly as a read matches them, so a list
    /// this table holds no partition for names no partition of it at all.
    NotFound,
}

impl PartitionDeleteRefusal {
    /// The modelled error shape this refusal is reported under: the `ErrorCode` of a
    /// batch's per-item [`ErrorDetail`], and the exception a single-partition delete
    /// raises.
    pub fn error_code(self) -> &'static str {
        match self {
            Self::NotFound => "EntityNotFoundException",
        }
    }

    /// This refusal's wording, the one this service answers a partition miss with
    /// wherever it reports one. See [`messages`] for how this crate's refusals are
    /// worded.
    pub fn message(self) -> String {
        match self {
            Self::NotFound => messages::PARTITION_NOT_FOUND.to_string(),
        }
    }

    /// This refusal as the per-item error shape a batch reports it in.
    pub fn to_error_detail(self) -> ErrorDetail {
        catalog_error_detail(self.error_code(), self.message())
    }
}

/// A stored partition of a table: the client-settable definition plus the one member
/// only the server sets, `CreationTime`.
#[derive(Debug, Clone)]
pub struct PartitionEntry {
    /// The values that address this partition, which are also this entry's key in
    /// [`TableEntry::partitions`].
    pub values: Vec<String>,
    pub storage_descriptor: Option<StorageDescriptor>,
    pub parameters: Option<HashMap<String, String>>,
    /// Client-supplied, echoed back exactly as sent.
    pub last_access_time: Option<::aws_smithy_types::DateTime>,
    /// Client-supplied, echoed back exactly as sent.
    pub last_analyzed_time: Option<::aws_smithy_types::DateTime>,
    /// Server-set, written once at create so every later read reports the same instant.
    pub creation_time: SystemTime,
    /// The key material the write that stored this partition was encrypted under, or
    /// `None` when that write was encrypted under no key of the caller's. See
    /// [`DatabaseEntry::encryption`].
    pub encryption: Option<ObjectEncryption>,
}

impl PartitionEntry {
    /// Build a stored partition from the client's `PartitionInput` plus the server-set
    /// `CreationTime`, which no `PartitionInput` carries.
    ///
    /// `values` is passed separately because it is the entry's ADDRESS: the caller has
    /// already checked the input carries a value list that matches the table's partition
    /// keys, and that same list is the map key the entry is stored under.
    pub fn from_input(values: Vec<String>, input: &PartitionInput, now: SystemTime) -> Self {
        Self {
            values,
            storage_descriptor: input.storage_descriptor.clone(),
            parameters: input.parameters.clone(),
            last_access_time: input.last_access_time,
            last_analyzed_time: input.last_analyzed_time,
            creation_time: now,
            // Recorded by the create, which is where the catalog's encryption setting is
            // read and the key material produced (see [`TableEntry::create_partition`]).
            encryption: None,
        }
    }

    /// Overwrite this partition's definition with the one the client submitted.
    ///
    /// `PartitionInput` is the WHOLE client-settable definition of a partition — the same
    /// shape a create submits — so an update writes every member it carries and clears the
    /// ones it omits; what the client sent is exactly what the next read reports. The
    /// members only the server sets keep their own history: the partition stays at the
    /// address it was created under and reports the `CreationTime` the create wrote,
    /// because an update produces no new partition.
    ///
    /// `encryption` is the key material THIS write is encrypted under, which replaces the
    /// create's: "the specified encryption is applied to every catalog write thereafter"
    /// (`PutDataCatalogEncryptionSettings`), so the stored definition is read back through
    /// the key the write that stored it used (see [`catalog_encryption`]).
    pub fn apply_input(&mut self, input: &PartitionInput, encryption: Option<ObjectEncryption>) {
        self.storage_descriptor = input.storage_descriptor.clone();
        self.parameters = input.parameters.clone();
        self.last_access_time = input.last_access_time;
        self.last_analyzed_time = input.last_analyzed_time;
        self.encryption = encryption;
    }

    /// Rebuild the modelled `Partition` output shape from what is stored.
    ///
    /// `table` is the table this partition belongs to, read by the caller rather than
    /// copied onto the partition: the catalog, database and table a partition resides in
    /// are the table's own attribution, so there is ONE place they are stored and a
    /// partition read cannot report a database or table that has drifted from the
    /// table read's.
    pub fn to_partition(&self, table: &TableEntry) -> Partition {
        Partition {
            values: Some(self.values.clone()),
            database_name: Some(table.database_name.clone()),
            table_name: Some(table.name.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            last_access_time: self.last_access_time,
            // Projected the way a table's own storage is
            // ([`to_storage_descriptor`]), so a partition and its table report a
            // descriptor the same way.
            storage_descriptor: self.storage_descriptor.as_ref().map(to_storage_descriptor),
            parameters: self.parameters.clone(),
            last_analyzed_time: self.last_analyzed_time,
            catalog_id: Some(table.catalog_id.clone()),
        }
    }
}

/// One ARCHIVED version of a table: the definition the table carried at an earlier
/// `VersionId`, kept so a version read can still serve it once the live definition has
/// moved on.
#[derive(Debug, Clone)]
pub struct ArchivedTableVersion {
    /// The client-settable definition that was current at this version, in the shape a
    /// client submits it in — so a version read rebuilds exactly what that write stored
    /// and nothing a later write changed.
    pub definition: TableInput,
    /// The instant that definition became current: the `UpdateTime` this version
    /// reports.
    pub update_time: SystemTime,
    /// The key material the write that stored THIS definition was encrypted under, kept
    /// with the definition it belongs to: a later write re-keys the live table
    /// ([`TableEntry::encryption`]) and leaves the versions it archived under the keys
    /// their own writes used. See [`catalog_encryption`].
    pub encryption: Option<ObjectEncryption>,
}

impl TableEntry {
    /// Build a stored table from the client's `TableInput` plus the server-derived
    /// members (`DatabaseName`, `CatalogId`, `CreateTime`, `UpdateTime`, `VersionId`,
    /// `CreatedBy`), none of which exist on `TableInput`.
    pub fn from_input(
        input: &TableInput,
        database_name: &str,
        catalog_id: &str,
        created_by: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            name: fold_name(&input.name),
            database_name: database_name.to_string(),
            catalog_id: catalog_id.to_string(),
            description: input.description.clone(),
            owner: input.owner.clone(),
            table_type: input.table_type.clone(),
            retention: input.retention,
            parameters: input.parameters.clone(),
            storage_descriptor: input.storage_descriptor.clone(),
            partition_keys: input.partition_keys.clone(),
            view_original_text: input.view_original_text.clone(),
            view_expanded_text: input.view_expanded_text.clone(),
            target_table: input.target_table.clone(),
            federated_table: input.federated_table.clone(),
            last_access_time: input.last_access_time,
            last_analyzed_time: input.last_analyzed_time,
            create_time: now,
            update_time: now,
            version_id: FIRST_TABLE_VERSION_ID.to_string(),
            created_by,
            // A create archives nothing: it replaced no definition, and the one it
            // writes is the current version.
            archived_versions: BTreeMap::new(),
            // A table is created empty of partitions: `TableInput` defines the partition
            // KEYS of a table, never a slice of its data, so the first partition arrives
            // through a partition create.
            partitions: BTreeMap::new(),
            // Not part of any `TableInput`: the handler records what the catalog's own
            // encryption setting had KMS produce for this write.
            encryption: None,
            // `TableInput` carries no column statistics task settings either: they arrive
            // through `CreateColumnStatisticsTaskSettings`.
            column_statistics_task_settings: None,
            // Nor any column statistics: those are generated by a column statistics task
            // run over the data behind the table, and a freshly created table has had
            // none.
            column_statistics: BTreeMap::new(),
            // Nor any optimizer: an optimizer of a table arrives through
            // `CreateTableOptimizer`, which addresses a table that already exists.
            optimizers: BTreeMap::new(),
        }
    }

    /// How many partition keys this table defines: the number of values that address one
    /// of its partitions.
    pub fn partition_key_count(&self) -> usize {
        self.partition_keys.as_ref().map_or(0, Vec::len)
    }

    /// Whether this table is an open-table-format (Iceberg) table, i.e. whether its
    /// parameters carry the marker an Iceberg table is recognised by
    /// ([`ICEBERG_TABLE_TYPE_PARAMETER`] holding [`ICEBERG_TABLE_TYPE`]).
    ///
    /// That marker is what every client and every service that consumes Glue tables tells
    /// an Iceberg table from a Hive one by, so it is read off the STORED parameters
    /// whichever write put it there: the create that minted the table's Iceberg metadata
    /// ([`IcebergTableMarker::parameters`]) or a `TableInput` that carried the parameter
    /// itself. The value is compared without regard to case, because the parameter is a
    /// client-written table property.
    pub fn is_iceberg_table(&self) -> bool {
        self.parameters.as_ref().is_some_and(|parameters| {
            parameters
                .get(ICEBERG_TABLE_TYPE_PARAMETER)
                .is_some_and(|table_type| table_type.eq_ignore_ascii_case(ICEBERG_TABLE_TYPE))
        })
    }

    /// The location of this table's current Iceberg metadata file, from the parameter an
    /// Iceberg table carries it in ([`ICEBERG_METADATA_LOCATION_PARAMETER`]), or `None` for
    /// a table whose parameters name none.
    ///
    /// An empty value is no filepath: the parameter is client-writable, so a table can carry
    /// the key with nothing in it.
    pub fn iceberg_metadata_location(&self) -> Option<&str> {
        self.parameters
            .as_ref()?
            .get(ICEBERG_METADATA_LOCATION_PARAMETER)
            .map(String::as_str)
            .filter(|location| !location.is_empty())
    }

    /// The stored partition addressed by `values`, or `None` when this table holds no
    /// such partition.
    pub fn partition(&self, values: &[String]) -> Option<&PartitionEntry> {
        self.partitions.get(values)
    }

    /// Whether this table can hold another partition, i.e. is below
    /// [`MAX_PARTITIONS_PER_TABLE`].
    pub fn can_store_partition(&self) -> bool {
        self.partitions.len() < MAX_PARTITIONS_PER_TABLE
    }

    /// Create ONE partition of this table from a client's `PartitionInput`, or say why
    /// it cannot be created.
    ///
    /// The whole create decision for a single partition, in one place, so the
    /// single-partition create and each item of a batch create accept and refuse exactly
    /// the same inputs: a partition is one value per partition key of its table, it is
    /// addressed by those values, and a value list the table already holds a partition
    /// for is a duplicate rather than an overwrite.
    ///
    /// `store_has_room` is whether the STORE as a whole can hold another partition
    /// ([`ComAmazonawsGlueAwsglueState::can_store_partition`]), which this table cannot
    /// see from here; the per-table ceiling is checked here.
    ///
    /// `encryption` is the key material this write is encrypted under, which the caller
    /// resolved from the catalog's own settings before it took the store's guard: a
    /// partition created while the catalog encrypts its writes under a KMS key is recorded
    /// under that key, and a read of it has to recover that key material first (see
    /// [`catalog_encryption`]).
    pub fn create_partition(
        &mut self,
        input: &PartitionInput,
        now: SystemTime,
        store_has_room: bool,
        encryption: Option<ObjectEncryption>,
    ) -> Result<&PartitionEntry, PartitionCreateRefusal> {
        let values = input.values.clone().unwrap_or_default();
        if values.is_empty() {
            return Err(PartitionCreateRefusal::ValuesRequired);
        }
        let keys = self.partition_key_count();
        if values.len() != keys {
            return Err(PartitionCreateRefusal::ValuesDoNotMatchPartitionKeys {
                keys,
                values: values.len(),
            });
        }
        if self.partitions.contains_key(&values) {
            return Err(PartitionCreateRefusal::AlreadyExists);
        }
        if !self.can_store_partition() {
            return Err(PartitionCreateRefusal::LimitExceeded {
                limit: MAX_PARTITIONS_PER_TABLE,
            });
        }
        if !store_has_room {
            return Err(PartitionCreateRefusal::LimitExceeded {
                limit: MAX_STORED_PARTITIONS,
            });
        }
        let mut entry = PartitionEntry::from_input(values.clone(), input, now);
        // The key material this write was encrypted under, which the caller read from the
        // catalog's settings and had KMS produce before taking the store's guard.
        entry.encryption = encryption;
        Ok(self.partitions.entry(values).or_insert(entry))
    }

    /// Update ONE partition of this table from a client's `PartitionInput`, or say why it
    /// cannot be updated.
    ///
    /// The whole update decision for a single partition, in one place, so the
    /// single-partition update and each entry of a batch update accept and refuse exactly
    /// the same entries: a partition is addressed by the values it was created under, an
    /// update replaces its definition rather than its address, and a value list this table
    /// holds no partition for is nothing to update.
    ///
    /// `values` is the ADDRESS the entry names (`PartitionValueList`) and `input` the
    /// definition to write; `encryption` is the key material this write is encrypted
    /// under, resolved from the catalog's own settings by the caller before it took the
    /// store's guard, exactly as a create's is.
    pub fn update_partition(
        &mut self,
        values: &[String],
        input: &PartitionInput,
        encryption: Option<ObjectEncryption>,
    ) -> Result<&PartitionEntry, PartitionUpdateRefusal> {
        let submitted = input.values.as_deref().unwrap_or_default();
        if submitted.is_empty() {
            return Err(PartitionUpdateRefusal::ValuesRequired);
        }
        if submitted != values {
            return Err(PartitionUpdateRefusal::ValuesCannotBeChanged);
        }
        let Some(entry) = self.partitions.get_mut(values) else {
            return Err(PartitionUpdateRefusal::NotFound);
        };
        entry.apply_input(input, encryption);
        Ok(entry)
    }

    /// Delete ONE partition of this table, or say why it cannot be deleted.
    ///
    /// The whole delete decision for a single partition, in one place, so the
    /// single-partition delete and each item of a batch delete accept and refuse exactly
    /// the same value lists: a partition is addressed by the values it was created under,
    /// matched exactly as a read matches them, and a list this table holds no partition
    /// for is nothing to delete. A deleted partition leaves the record every partition
    /// read consults ([`Self::partitions`]), so it is gone for every later reader, and the
    /// table's other partitions are untouched.
    ///
    /// Nothing of the removed partition is handed back: a delete reports the removal and
    /// no definition, and neither the single-partition `DeletePartitionResponse` nor a
    /// batch's `PartitionError` carries one.
    pub fn delete_partition(&mut self, values: &[String]) -> Result<(), PartitionDeleteRefusal> {
        if self.partitions.remove(values).is_some() {
            Ok(())
        } else {
            Err(PartitionDeleteRefusal::NotFound)
        }
    }

    /// This table's client-settable definition, in the `TableInput` shape a client
    /// submits it in — the inverse of [`Self::from_input`], and what archiving a version
    /// keeps of the definition a write is about to replace.
    ///
    /// `ViewDefinition` is absent because no stored table carries one: `CreateTable` and
    /// `UpdateTable` refuse a view definition rather than storing it.
    pub fn to_table_input(&self) -> TableInput {
        TableInput {
            name: self.name.clone(),
            description: self.description.clone(),
            owner: self.owner.clone(),
            last_access_time: self.last_access_time,
            last_analyzed_time: self.last_analyzed_time,
            retention: self.retention,
            storage_descriptor: self.storage_descriptor.clone(),
            partition_keys: self.partition_keys.clone(),
            view_original_text: self.view_original_text.clone(),
            view_expanded_text: self.view_expanded_text.clone(),
            table_type: self.table_type.clone(),
            parameters: self.parameters.clone(),
            target_table: self.target_table.clone(),
            federated_table: self.federated_table.clone(),
            view_definition: None,
        }
    }

    /// Whether this table can archive another version, i.e. holds fewer than
    /// [`MAX_ARCHIVED_TABLE_VERSIONS_PER_TABLE`].
    pub fn can_archive_version(&self) -> bool {
        self.archived_versions.len() < MAX_ARCHIVED_TABLE_VERSIONS_PER_TABLE
    }

    /// The counters of every version of this table the catalog holds, ASCENDING.
    ///
    /// A version of a table is either the live definition — the version
    /// [`Self::version_id`] names — or one the catalog archived
    /// ([`Self::archived_versions`]), so the two together are the whole history a version
    /// listing walks, in the order the counters were minted ("Each version is incremented
    /// by 1", `GetTableVersionRequest$VersionId`). Each counter is readable through
    /// [`Self::table_version`], so the list is exactly the versions that answer.
    pub fn version_counters(&self) -> Vec<u64> {
        let mut counters: Vec<u64> = self.archived_versions.keys().copied().collect();
        if let Some(current) = parse_version_counter(&self.version_id) {
            counters.push(current);
        }
        counters.sort_unstable();
        counters.dedup();
        counters
    }

    /// This table as of one of its versions, as the modelled `TableVersion` shape, or
    /// `None` when the catalog holds no such version of it.
    ///
    /// `version` is the counter a request named, or `None` for a request that named no
    /// version, which addresses the table's current one. The current definition IS a
    /// version — the one [`Self::version_id`] names — so a version read of a table that
    /// has never been updated reports exactly what a plain read of the table reports.
    /// Every earlier version comes from [`Self::archived_versions`] and is projected
    /// through the same `Table` builder: the definition that was current then, with the
    /// members that belong to the table rather than to one of its versions
    /// (`CreateTime`, `CreatedBy`, `CatalogId`, `DatabaseName`) as they stand, and
    /// `UpdateTime` the instant that version became current.
    pub fn table_version(&self, version: Option<u64>, database_id: &str) -> Option<TableVersion> {
        let Some(wanted) = self.archived_version_wanted(version) else {
            return Some(TableVersion {
                table: Some(self.to_table(database_id)),
                version_id: Some(self.version_id.clone()),
            });
        };
        let archived = self.archived_versions.get(&wanted)?;
        let mut historical = Self::from_input(
            &archived.definition,
            &self.database_name,
            &self.catalog_id,
            self.created_by.clone(),
            archived.update_time,
        );
        // `from_input` dates a FRESH table, so `CreateTime` — which belongs to the
        // table and not to one of its versions — is restored from the live record, and
        // the version id is the one this archived definition is keyed by.
        historical.create_time = self.create_time;
        historical.version_id = wanted.to_string();
        Some(TableVersion {
            table: Some(historical.to_table(database_id)),
            version_id: Some(wanted.to_string()),
        })
    }

    /// Which ARCHIVED version a version read addresses, or `None` when it addresses the
    /// live definition: a request that names no version, or names the one
    /// [`Self::version_id`] is under, addresses the current version.
    ///
    /// The one selection both [`Self::table_version`] and [`Self::version_encryption`] go
    /// through, so the definition a version read answers and the key material it is
    /// answered through can never come from different versions.
    pub(super) fn archived_version_wanted(&self, version: Option<u64>) -> Option<u64> {
        match version {
            None => None,
            Some(wanted) if Some(wanted) == parse_version_counter(&self.version_id) => None,
            Some(wanted) => Some(wanted),
        }
    }

    /// How this table words the miss of ONE of its versions: which of the service's two
    /// version-miss sentences the `version` a request named earns.
    ///
    /// The two are told apart by whether the id is a version this table ever MINTED. "Each
    /// version is incremented by 1" (`GetTableVersionRequest$VersionId`), so the counter the
    /// live definition carries is the newest id the catalog has minted for this table: an id
    /// at or below it stood for a definition of this table, which the history no longer
    /// holds, and an id above it was never a version of this table at all. The second is
    /// what a read of a table whose history restarted (a `DeleteTable` and a create under
    /// the same name) addresses with an id of the old history.
    ///
    /// Shared by every operation that resolves one version by id, so the read and the
    /// single-version delete cannot word one miss two ways.
    ///
    /// Both sentences are RECORDED, on two reads of the SAME table in one wire fixture
    /// (`glue-table-versioning-and-column-statistics`, its two version reads): the one that
    /// addresses an id past the table's newest earns the first sentence, and the one that
    /// addresses a version the table had and the history no longer holds earns the second.
    /// A later recording that showed the split anywhere else is the first thing to re-check
    /// here.
    pub fn version_miss_message(&self, version: Option<u64>) -> &'static str {
        let minted = version.is_some_and(|wanted| {
            parse_version_counter(&self.version_id).is_some_and(|current| wanted <= current)
        });
        if minted {
            messages::TABLE_VERSION_NOT_FOUND
        } else {
            messages::VERSION_NOT_FOUND
        }
    }

    /// The key material the write that stored the definition of ONE of this table's
    /// versions was encrypted under — the live record's for the current version, and the
    /// archived record's own for an earlier one.
    ///
    /// `None` is a version whose write was encrypted under no key of the caller's, and
    /// also a version this table does not hold, which [`Self::table_version`] reports as
    /// the miss it is.
    pub fn version_encryption(&self, version: Option<u64>) -> Option<ObjectEncryption> {
        match self.archived_version_wanted(version) {
            None => self.encryption.clone(),
            Some(wanted) => self
                .archived_versions
                .get(&wanted)
                .and_then(|archived| archived.encryption.clone()),
        }
    }

    /// Overwrite this table's definition with the one the client submitted.
    ///
    /// `TableInput` is the WHOLE client-settable definition of a table — the same
    /// shape a create submits — so an update writes every member it carries and clears
    /// the ones it omits; what the client sent is exactly what the next read reports.
    /// The server-set members keep their own history: `CreateTime`, `CreatedBy`,
    /// `CatalogId` and `DatabaseName` stay as the create left them, `UpdateTime` moves
    /// to this write's instant, and `VersionId` advances to the next version, because
    /// the update produced a new version of the table.
    ///
    /// `TableInput$ViewDefinition` is NOT carried here: this slice stores no view
    /// representations, so an update that submits one is refused by the handler rather
    /// than accepted and dropped.
    ///
    /// The submitted `Name` is folded and written like any other member, but it can
    /// only ever be the name this table already has: `UpdateTable` refuses a request
    /// whose `TableInput$Name` is not the table it addresses, so the entry's key in
    /// [`DatabaseEntry::tables`] stays correct. A caller that ever applies an input
    /// under a different name must relocate the entry under its new key, the way
    /// `UpdateDatabase` does for [`DatabaseEntry::apply_input`].
    ///
    /// `archive_previous` keeps the definition this write replaces as the version it was
    /// current under ([`Self::archived_versions`]), which is what a later version read
    /// serves; the caller passes `false` for a write that asks not to archive it.
    pub fn apply_input(&mut self, input: &TableInput, now: SystemTime, archive_previous: bool) {
        if archive_previous {
            if let Some(current) = parse_version_counter(&self.version_id) {
                self.archived_versions.insert(
                    current,
                    ArchivedTableVersion {
                        definition: self.to_table_input(),
                        update_time: self.update_time,
                        // The definition being archived was encrypted by the write that
                        // stored it, so its key material is archived with it rather than
                        // left to the key this write is about to record on the table.
                        encryption: self.encryption.clone(),
                    },
                );
            }
        }
        self.name = fold_name(&input.name);
        self.description = input.description.clone();
        self.owner = input.owner.clone();
        self.table_type = input.table_type.clone();
        self.retention = input.retention;
        self.parameters = input.parameters.clone();
        self.storage_descriptor = input.storage_descriptor.clone();
        self.partition_keys = input.partition_keys.clone();
        self.view_original_text = input.view_original_text.clone();
        self.view_expanded_text = input.view_expanded_text.clone();
        self.target_table = input.target_table.clone();
        self.federated_table = input.federated_table.clone();
        self.last_access_time = input.last_access_time;
        self.last_analyzed_time = input.last_analyzed_time;
        self.update_time = now;
        self.version_id = next_version_id(&self.version_id);
    }

    /// Rebuild the modelled `Table` output shape from what is stored.
    ///
    /// `database_id` is the catalog-assigned id of the database this table belongs to,
    /// read from that [`DatabaseEntry`] by the caller rather than copied onto the table:
    /// the id belongs to the database, so there is one place it is stored and the table
    /// read cannot report an id that has drifted from the database read's.
    pub fn to_table(&self, database_id: &str) -> Table {
        // `Table$RLSProperties` is "the default row-level security policy of the table".
        // No stored table carries one, because no `TableInput` can set one — so the
        // catalog holds no row filter for any table it serves. Bound once here, because
        // `IsRowFilteringEnabled` below is derived from it: the flag says exactly whether
        // such a policy is in force, so the two can never report different things.
        let rls_properties: Option<RLSProperties> = None;
        Table {
            name: self.name.clone(),
            database_name: Some(self.database_name.clone()),
            description: self.description.clone(),
            owner: self.owner.clone(),
            create_time: Some(to_date_time(self.create_time)),
            update_time: Some(to_date_time(self.update_time)),
            last_access_time: self.last_access_time,
            last_analyzed_time: self.last_analyzed_time,
            retention: self.retention,
            storage_descriptor: self.storage_descriptor.as_ref().map(to_storage_descriptor),
            partition_keys: self.partition_keys.clone(),
            view_original_text: self.view_original_text.clone(),
            view_expanded_text: self.view_expanded_text.clone(),
            table_type: self.table_type.clone(),
            parameters: self.parameters.clone(),
            created_by: self.created_by.clone(),
            is_registered_with_lake_formation: false,
            target_table: self.target_table.clone(),
            catalog_id: Some(self.catalog_id.clone()),
            version_id: Some(self.version_id.clone()),
            federated_table: self.federated_table.clone(),
            // `Table$ViewDefinition` is "all the information that defines the view": no
            // stored table has one, because `CreateTable` REFUSES a `ViewDefinition`
            // rather than dropping it (see `ops/create_table.rs`).
            view_definition: None,
            // `IsMultiDialectView` and `IsMaterializedView` are three-state
            // `NullableBoolean` members the catalog answers DEFINITELY on every read: a
            // table is one of them exactly when the catalog holds the matching view
            // definition for it. No stored table does (see `view_definition` above), so
            // both are `false` — reported rather than omitted, so a client branching on
            // three states never reads "unknown" for a table whose kind the catalog
            // knows.
            is_multi_dialect_view: Some(false),
            is_materialized_view: Some(false),
            iceberg_table_metadata: None,
            status: None,
            // The catalog attributes every table it returns to the catalog that owns it
            // and to the database that holds it, by the ids it assigned them — the same
            // `DatabaseId` the database's own read reports, so the two responses agree.
            catalog_identifier: Some(self.catalog_id.clone()),
            database_id: Some(database_id.to_string()),
            // Row filtering is on exactly when the catalog holds a row-level security
            // policy for the table, so the flag is derived from `rls_properties` rather
            // than restated: `false` for every stored table today, and DEFINITELY false
            // rather than omitted, so a client that distinguishes "no filter" from
            // "unknown" reads the first — which is what the catalog knows.
            is_row_filtering_enabled: Some(rls_properties.is_some()),
            rls_properties,
        }
    }
}

/// Project a stored `StorageDescriptor` the way the Data Catalog reports one.
///
/// `StorageDescriptor$Columns` is "a list of the `Columns` in the table" and
/// `StorageDescriptor$SortColumns` "a list specifying the sort order of each bucket in the
/// table" — both are part of the definition the catalog holds, so a descriptor that lists
/// no column and defines no sort order has an EMPTY list of each rather than an unknown
/// one, and the catalog reports `[]`. A client that iterates either then loops zero times
/// instead of having to handle an absent member. Every other member is reported exactly as
/// it was stored: a member the client never sent is one the catalog does not determine, so
/// it stays absent.
pub fn to_storage_descriptor(stored: &StorageDescriptor) -> StorageDescriptor {
    StorageDescriptor {
        columns: Some(stored.columns.clone().unwrap_or_default()),
        sort_columns: Some(stored.sort_columns.clone().unwrap_or_default()),
        ..stored.clone()
    }
}
