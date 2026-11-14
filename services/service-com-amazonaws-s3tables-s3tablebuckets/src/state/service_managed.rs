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

impl ComAmazonawsS3tablesS3tablebucketsState {
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

        let table_id = self.mint_table_id();
        let version_token = self.mint_version_token();
        let table_arn = table_arn(&bucket_arn, &table_id);
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
            modified_at: now,
            modified_by: account.clone(),
            owner_account_id: account,
            // The table names no encryption of its own, so it is encrypted the way its
            // bucket is — the same thing a create that names none settles.
            encryption: None,
            storage_class,
            record_expiration: None,
            maintenance: BTreeMap::new(),
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
