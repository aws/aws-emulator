// SPDX-License-Identifier: CC0-1.0

//! The tables an Amazon Web Services service creates in a table bucket on the account's
//! behalf, and the one place such a table is written.
//!
//! `GetTable` reports "The service that manages the table" (`managedByService`) and a
//! `type` of `aws` for "a table Amazon Web Services created on the account's behalf"
//! (`TableType`), so a table this store holds under those two values is one a service
//! provisioned rather than one a caller created through `CreateTable`. A delivery service
//! that promises its customer a table — "Amazon Kinesis Data Streams creates this table in
//! the specified table bucket" — provisions it HERE, through
//! [`ComAmazonawsS3tablesS3tablebucketsState::create_service_managed_table`], so the record
//! it leaves behind is an ordinary table of this store: `GetTable`, `ListTables` and every
//! other read resolve it exactly as they resolve a table a caller created, and the identity
//! rules a table carries (the minted id behind its ARN, the ARN index it is addressed
//! through, the storage class it is created in, the per-namespace name uniqueness and the
//! bounded namespace) stay in this crate rather than being restated by the service that asks
//! for the table.
//!
//! A service whose tables live in a table bucket AWS keeps for it — the managed table bucket
//! an S3 Metadata configuration's tables are created in — needs that bucket and a namespace
//! inside it before a table has anywhere to go, so
//! [`ComAmazonawsS3tablesS3tablebucketsState::ensure_service_managed_namespace`] creates
//! those here too, under the modelled `aws` bucket type. A table a service provisioned and
//! nothing came to stand on goes back the same way
//! ([`ComAmazonawsS3tablesS3tablebucketsState::delete_service_managed_table`]), which touches
//! only tables that service manages.

use super::*;

/// Why a table bucket could not take the table a service asked it to create.
///
/// Each case is a fact about THIS store — the bucket, the namespace or the name — which the
/// asking service words in its own modelled error class, so no sentence of this service's
/// travels on another service's wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceManagedTableRefusal {
    /// No table bucket of this account and region carries the ARN the service named.
    NoSuchTableBucket,
    /// The bucket holds no namespace of that name, so there is nowhere to create the table.
    NoSuchNamespace,
    /// The namespace already holds a table of that name, and it is not one the asking
    /// service manages — a table a caller created is never taken over or overwritten.
    NameTaken,
    /// The namespace already holds [`TABLES_PER_NAMESPACE`] tables.
    NamespaceFull,
}

/// Why a table bucket could not take the NAMESPACE a service asked it to hold, or the
/// managed table bucket itself could not be created.
///
/// Each case is a fact about THIS store's bounds, which the asking service words in its own
/// modelled error class — the same division [`ServiceManagedTableRefusal`] keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceManagedNamespaceRefusal {
    /// This account already holds [`TABLE_BUCKETS_PER_ACCOUNT`] table buckets, so there is
    /// no room for the one the service's tables would live in.
    AccountFull,
    /// The table bucket already holds [`NAMESPACES_PER_TABLE_BUCKET`] namespaces.
    TableBucketFull,
    /// No table bucket of that ARN is resident — which this call cannot reach, because it
    /// creates the bucket itself when it is not there; it is here so a store that
    /// nevertheless does not hold it is reported as what it is rather than as a bound that
    /// was not met.
    NoSuchTableBucket,
}

impl ComAmazonawsS3tablesS3tablebucketsState {
    /// The table bucket `name` of `region` that Amazon Web Services keeps for the tables it
    /// creates on the account's behalf, holding the namespace `namespace` — created here when
    /// either is not there yet — answered with the bucket's `tableBucketARN`.
    ///
    /// What a service that provisions a table needs before
    /// [`Self::create_service_managed_table`] has anywhere to put it: "a bucket Amazon Web
    /// Services created on the account's behalf" ([`TableBucketType::Aws`], the modelled `aws`
    /// bucket type) and a namespace inside it. IDEMPOTENT: a bucket or a namespace already
    /// resident is left exactly as it is, including one a CALLER created under that name, so
    /// nothing a client holds is replaced or taken over.
    ///
    /// Both stores are BOUNDED exactly as they are for a create that arrives on the wire
    /// ([`TABLE_BUCKETS_PER_ACCOUNT`], [`NAMESPACES_PER_TABLE_BUCKET`]), so a service cannot
    /// grow this process's memory without limit either.
    pub fn ensure_service_managed_namespace(
        &mut self,
        region: &str,
        name: &str,
        namespace: &str,
        now: SystemTime,
    ) -> Result<String, ServiceManagedNamespaceRefusal> {
        let arn = table_bucket_arn(region, name);
        if !self.table_buckets.contains_key(&arn) {
            if self.is_at_bucket_quota() {
                return Err(ServiceManagedNamespaceRefusal::AccountFull);
            }
            let account = ::engine::emulator_account_id().to_string();
            let bucket_id = self.mint_bucket_id();
            self.table_buckets.insert(
                arn.clone(),
                TableBucket {
                    name: name.to_string(),
                    arn: arn.clone(),
                    region: region.to_string(),
                    owner_account_id: account,
                    bucket_id,
                    // "A bucket Amazon Web Services created on the account's behalf", which
                    // is what a bucket a service provisions is.
                    bucket_type: TableBucketType::Aws,
                    created_at: now,
                    // The bucket names no encryption and no storage class of its own, so both
                    // are reported as this service's documented defaults — the same thing a
                    // create that names neither settles.
                    encryption: None,
                    storage_class: None,
                    metrics: None,
                    replication: None,
                    policy: None,
                    maintenance: BTreeMap::new(),
                    maintenance_schedules: BTreeMap::new(),
                    tags: BTreeMap::new(),
                    namespaces: BTreeMap::new(),
                    // The bucket delivers no server access logs until a put
                    // turns logging on.
                    logging: None,
                },
            );
        }
        if self.holds_namespace(&arn, namespace) {
            return Ok(arn);
        }
        self.ensure_namespace(&arn, namespace, now)?;
        Ok(arn)
    }

    /// Ensure the table bucket `table_bucket_arn` holds a namespace called
    /// `namespace`, creating one when it does not.
    ///
    /// IDEMPOTENT: a namespace already resident is left exactly as it is, including one a
    /// CALLER created under that name. The bucket itself is NOT created here — a caller of
    /// this is adding a namespace to a bucket that already stands — and the store stays
    /// BOUNDED exactly as it is for a create that arrives on the wire
    /// ([`NAMESPACES_PER_TABLE_BUCKET`]).
    pub fn ensure_namespace(
        &mut self,
        table_bucket_arn: &str,
        namespace: &str,
        now: SystemTime,
    ) -> Result<(), ServiceManagedNamespaceRefusal> {
        if self.holds_namespace(table_bucket_arn, namespace) {
            return Ok(());
        }
        let held = self
            .table_buckets
            .get(table_bucket_arn)
            .ok_or(ServiceManagedNamespaceRefusal::NoSuchTableBucket)?;
        if held.namespaces.len() >= NAMESPACES_PER_TABLE_BUCKET {
            return Err(ServiceManagedNamespaceRefusal::TableBucketFull);
        }
        let account = ::engine::emulator_account_id().to_string();
        // Minted only for a namespace this really creates, so an id is not spent on a call
        // that found one already there.
        let namespace_id = self.mint_namespace_id();
        let Some(bucket) = self.table_buckets.get_mut(table_bucket_arn) else {
            // Unreachable: the bucket was resident a few lines above and this call holds the
            // store. The id minted just above goes unused, which costs nothing but the
            // sequence number.
            return Err(ServiceManagedNamespaceRefusal::NoSuchTableBucket);
        };
        bucket.namespaces.insert(
            namespace.to_string(),
            Namespace {
                namespace: vec![namespace.to_string()],
                namespace_id,
                created_at: now,
                created_by: account.clone(),
                owner_account_id: account,
                // A fresh namespace holds no tables; a create is what puts one in it.
                tables: BTreeMap::new(),
            },
        );
        Ok(())
    }

    /// Whether the table bucket `table_bucket_arn` holds a namespace called `namespace` now.
    pub fn holds_namespace(&self, table_bucket_arn: &str, namespace: &str) -> bool {
        self.table_buckets
            .get(table_bucket_arn)
            .is_some_and(|bucket| bucket.namespaces.contains_key(namespace))
    }

    /// Whether a table stands under `name` in `namespace` of the table bucket
    /// `table_bucket_arn` now.
    ///
    /// The existence question a service that provisioned a table there asks before it
    /// provisions another under the same name: a table name is unique inside its namespace,
    /// and WHOSE table it is does not change that — so this answers about any table resident
    /// under the name, one a caller created through `CreateTable` included.
    pub fn holds_table(&self, table_bucket_arn: &str, namespace: &str, name: &str) -> bool {
        self.table_buckets
            .get(table_bucket_arn)
            .and_then(|bucket| bucket.namespaces.get(namespace))
            .is_some_and(|namespace| namespace.tables.contains_key(name))
    }

    /// Delete the table `name` in `namespace` of `table_bucket_arn` when it is one
    /// `managed_by_service` manages, reporting whether a table went.
    ///
    /// The counterpart of [`Self::create_service_managed_table`], for a service that is
    /// taking back a table it provisioned and that nothing came to stand on: a table managed
    /// by any OTHER service, and a table a caller created, is left exactly where it is —
    /// whose table that is belongs to whoever created it, and `DeleteTable` is how a caller
    /// removes one of its own.
    pub fn delete_service_managed_table(
        &mut self,
        table_bucket_arn: &str,
        namespace: &str,
        name: &str,
        managed_by_service: &str,
    ) -> bool {
        let Some(namespace_record) = self
            .table_buckets
            .get_mut(table_bucket_arn)
            .and_then(|bucket| bucket.namespaces.get_mut(namespace))
        else {
            return false;
        };
        let Some(table) = namespace_record.tables.get(name) else {
            return false;
        };
        if table.managed_by_service.as_deref() != Some(managed_by_service) {
            return false;
        }
        let table_arn = table.table_arn.clone();
        namespace_record.tables.remove(name);
        // The ARN index only ever holds live tables, so the entry goes with the record.
        self.tables_by_arn.remove(&table_arn);
        true
    }

    /// Create the table `name` in `namespace` of the table bucket `table_bucket_arn`,
    /// managed by the service principal `managed_by_service`, and answer with its `tableARN`.
    ///
    /// The record is built the way `CreateTable` builds one, with the three differences a
    /// service-provisioned table carries: its `type` is the modelled `aws` rather than
    /// `customer`, it reports the managing service, and it carries no table metadata of its
    /// own — a service that commits the table's first metadata file does so through the
    /// catalog, exactly as a caller-created table with no `metadata` in its create does.
    ///
    /// IDEMPOTENT for the service that owns the table: a namespace that already holds a table
    /// of that name managed by `managed_by_service` is already provisioned, so nothing is
    /// written and the stored table's ARN is answered. A table of that name held by anybody
    /// else is [`ServiceManagedTableRefusal::NameTaken`]: a table name is unique within its
    /// namespace, and a caller's table is never replaced.
    pub fn create_service_managed_table(
        &mut self,
        table_bucket_arn: &str,
        namespace: &str,
        name: &str,
        managed_by_service: &str,
        now: SystemTime,
    ) -> Result<String, ServiceManagedTableRefusal> {
        self.create_service_managed_table_with_id(
            table_bucket_arn,
            namespace,
            name,
            managed_by_service,
            None,
            now,
        )
    }

    /// That same create, with `table_id` the id the table's ARN carries instead of a freshly
    /// minted one.
    ///
    /// For the one case a minted id cannot serve: a service that PUBLISHES the table's ARN
    /// through its own API, which has to be the ARN this store answers to or the two services
    /// would name one table by two ARNs. The id is still what the ARN's last segment is, so
    /// the ARN index resolves the table through it exactly as it resolves a minted one; `None`
    /// mints the id here, which is what every other provisioning service takes.
    pub fn create_service_managed_table_with_id(
        &mut self,
        table_bucket_arn: &str,
        namespace: &str,
        name: &str,
        managed_by_service: &str,
        table_id: Option<&str>,
        now: SystemTime,
    ) -> Result<String, ServiceManagedTableRefusal> {
        let bucket = self
            .table_buckets
            .get(table_bucket_arn)
            .ok_or(ServiceManagedTableRefusal::NoSuchTableBucket)?;
        let bucket_arn = bucket.arn.clone();
        let bucket_id = bucket.bucket_id.clone();
        // A table is created IN a storage class: a service names none, so the table inherits
        // the bucket's default at this instant, which is what `CreateTable` settles for a
        // request that names none.
        let storage_class = Some(effective_bucket_storage_class(bucket.storage_class));
        let namespace_record = bucket
            .namespaces
            .get(namespace)
            .ok_or(ServiceManagedTableRefusal::NoSuchNamespace)?;
        let namespace_id = namespace_record.namespace_id.clone();
        let namespace_names = namespace_record.namespace.clone();

        if let Some(held) = namespace_record.tables.get(name) {
            return if held.managed_by_service.as_deref() == Some(managed_by_service) {
                Ok(held.table_arn.clone())
            } else {
                Err(ServiceManagedTableRefusal::NameTaken)
            };
        }
        // The store is BOUNDED, as it is for a create that arrives on the wire.
        if namespace_record.tables.len() >= TABLES_PER_NAMESPACE {
            return Err(ServiceManagedTableRefusal::NamespaceFull);
        }

        let table_id = match table_id {
            Some(published) => published.to_string(),
            None => self.mint_table_id(),
        };
        let version_token = self.mint_version_token();
        let table_arn = table_arn(&bucket_arn, &table_id);
        // A table ARN names ONE table of this store, so an id another table already carries is
        // no id for this one: taking it would re-point the ARN index at a second record while
        // the first stayed in its namespace. A minted id cannot collide; a published one is the
        // asking service's, so it is checked here rather than trusted.
        if self.tables_by_arn.contains_key(&table_arn) {
            return Err(ServiceManagedTableRefusal::NameTaken);
        }
        let warehouse_location = warehouse_location(&table_id);
        let account = ::engine::emulator_account_id().to_string();
        let table = Table {
            name: name.to_string(),
            table_arn: table_arn.clone(),
            // "A table Amazon Web Services created on the account's behalf", which is what a
            // table a service provisions is.
            table_type: TableType::Aws,
            managed_by_service: Some(managed_by_service.to_string()),
            namespace: namespace_names,
            namespace_id,
            table_bucket_id: bucket_id,
            // The one format the model declares, and the one S3 Tables creates.
            format: OpenTableFormat::Iceberg,
            version_token,
            // No metadata file exists for the table until something writes one.
            metadata_location: None,
            iceberg: None,
            warehouse_location,
            created_at: now,
            created_by: account.clone(),
            // A table nothing has modified since it was created was last
            // modified when it was created, and no account has modified it.
            modified_at: now,
            modified_by: None,
            owner_account_id: account,
            // The table names no encryption of its own, so it is encrypted the way its
            // bucket is — the same thing a create that names none settles.
            encryption: None,
            storage_class,
            record_expiration: None,
            // A freshly created table replicates nowhere until a put sets a
            // replication configuration on it.
            replication: None,
            maintenance: BTreeMap::new(),
            maintenance_schedules: BTreeMap::new(),
            // No migration created this table, so there is no migration of it to
            // complete.
            migration_completed_at: None,
            tags: BTreeMap::new(),
        };

        let Some(namespace_record) = self
            .table_buckets
            .get_mut(table_bucket_arn)
            .and_then(|bucket| bucket.namespaces.get_mut(namespace))
        else {
            return Err(ServiceManagedTableRefusal::NoSuchNamespace);
        };
        namespace_record.tables.insert(name.to_string(), table);
        // The index the `tableArn` addressing mode resolves through, so every ARN-addressed
        // read reaches this table too.
        self.tables_by_arn.insert(
            table_arn.clone(),
            TableLocation {
                table_bucket_arn: bucket_arn,
                namespace: namespace.to_string(),
                name: name.to_string(),
            },
        );
        Ok(table_arn)
    }
}
