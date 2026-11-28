// SPDX-License-Identifier: CC0-1.0

//! The Data Catalog's EXPORT configuration: the record a
//! `PutDataCatalogExportConfiguration` writes for one catalog and
//! `GetDataCatalogExportConfiguration` reports, with the status the clock owes it while a
//! change it asked for is still being applied.
//!
//! ONE record per catalog, exactly as the catalog's security configuration is
//! ([`Catalog::encryption_settings`]): neither operation that addresses the export
//! configuration carries a `CatalogId`, so the account's own Data Catalog
//! ([`resolve_catalog_id`]) is the whole address and a write REPLACES the record rather
//! than adding to it.
//!
//! The SETTING is what a write asked for and the STATUS is how far the service has got with
//! it: "Specify ENABLED to start exporting catalog metadata to S3 Tables, or DISABLED to
//! stop exporting" against a status of "ENABLING, ENABLED, DISABLING, DISABLED, and FAILED".
//! So a write that changes the setting leaves the catalog in the transitional status of that
//! direction, and the clock owes it the settled one ([`EXPORT_CONFIGURATION_SETTLES_AFTER`]);
//! the status is DERIVED from the stored due instant whenever the record is read, so nothing
//! sleeps and no read has to write.

use super::*;

/// `ExportSetting` for a catalog that exports its metadata to S3 Tables: one of the two
/// values the modelled enum carries ([`crate::ops::types::EXPORT_SETTING_VALUES`]).
pub const EXPORT_SETTING_ENABLED: &str = "ENABLED";

/// `ExportSetting` for a catalog that does not export its metadata: the other value of
/// [`crate::ops::types::EXPORT_SETTING_VALUES`], and the setting a catalog no write has
/// configured carries, since nothing has asked for its metadata to be exported.
pub const EXPORT_SETTING_DISABLED: &str = "DISABLED";

/// The `ExportStatus` of a catalog whose export is being turned on — "The export
/// configuration controls whether catalog metadata is exported to S3 Tables" and this is the
/// status while that is still being set up. One of
/// [`crate::ops::types::EXPORT_STATUS_VALUES`].
pub const EXPORT_STATUS_ENABLING: &str = "ENABLING";

/// The `ExportStatus` the clock owes a catalog whose export has finished being turned on.
pub const EXPORT_STATUS_ENABLED: &str = "ENABLED";

/// The `ExportStatus` of a catalog whose export is being turned off.
pub const EXPORT_STATUS_DISABLING: &str = "DISABLING";

/// The `ExportStatus` the clock owes a catalog whose export has finished being turned off,
/// and the status of a catalog no write has configured.
pub const EXPORT_STATUS_DISABLED: &str = "DISABLED";

/// The server-side encryption algorithms an export may be configured with: "The server-side
/// encryption algorithm used for the exported data. Valid values are `AES256` and `aws:kms`"
/// (`ExportEncryptionConfiguration$SseAlgorithm`).
///
/// The member is a plain string rather than a modelled enum, so the walk that validates a
/// request cannot refuse a value outside this pair; the write does
/// ([`messages::unsupported_export_sse_algorithm`]), because an algorithm the service does
/// not implement would otherwise read back out of the configuration as if the exported data
/// were encrypted with it.
pub const EXPORT_SSE_ALGORITHMS: &[&str] = &["AES256", "aws:kms"];

/// How long a catalog stays in the transitional export status a write put it in
/// ([`EXPORT_STATUS_ENABLING`], [`EXPORT_STATUS_DISABLING`]) before the clock owes it the
/// settled one.
///
/// CHOSEN, not documented: Glue publishes no duration for setting a catalog's export up or
/// tearing it down, so this is the emulator's own span — the one this crate already gives a
/// resource whose provisioning a caller can watch ([`BLUEPRINT_SETTLES_AFTER`]) and
/// deliberately not zero, so a caller that reads the configuration straight after a write
/// still observes the change being applied, and the refusal a second write earns while it
/// is. The due instant is stored on the record and compared with the instant of the call
/// that reads it; nothing sleeps.
pub const EXPORT_CONFIGURATION_SETTLES_AFTER: Duration = BLUEPRINT_SETTLES_AFTER;

/// The `ExportSetting` and `ExportStatus` of the catalog's DATA QUALITY export and of its
/// LINEAGE export, which the service reports beside the metadata export's own.
///
/// RECORDED, not modelled: the published `GetDataCatalogExportConfigurationOutput` binds
/// `ExportSetting`/`Status` for the catalog's metadata export only, and
/// `PutDataCatalogExportConfiguration` carries no member for either of these two exports, so
/// nothing can ask for a catalog's data quality or its lineage to be exported and no catalog
/// this service holds exports either. The service reports both regardless, so a client
/// reading the catalog's export configuration sees the state each of those exports is in —
/// which, with nothing to turn them on, is off ([`EXPORT_SETTING_DISABLED`]) and settled
/// there ([`EXPORT_STATUS_DISABLED`]).
const UNEXPORTED: (&str, &str) = (EXPORT_SETTING_DISABLED, EXPORT_STATUS_DISABLED);

/// ONE catalog's export configuration: the setting the last write asked for, how the
/// exported data is encrypted, when the configuration was first written and last changed,
/// and the instant from which the clock owes the setting's settled status.
#[derive(Debug, Clone)]
pub struct DataCatalogExportConfigurationEntry {
    /// `ExportSetting` as the last write sent it: [`EXPORT_SETTING_ENABLED`] or
    /// [`EXPORT_SETTING_DISABLED`], the direction the status below is moving in.
    pub setting: String,
    /// `ExportEncryptionConfiguration` as the last write sent it, or `None` where that write
    /// named none — "If not specified, the default encryption settings are used", which is
    /// a choice the service makes for the exported data and not a value of this record.
    pub encryption_configuration: Option<ExportEncryptionConfiguration>,
    /// `CreatedAt`: "The timestamp at which the export configuration was created" — the
    /// instant of the FIRST write to this catalog's configuration, which a later write does
    /// not move.
    pub created_at: SystemTime,
    /// `UpdatedAt`: "The timestamp at which the export configuration was last updated" — the
    /// instant of the write that left the record as it stands.
    pub updated_at: SystemTime,
    /// The instant from which the clock owes this catalog the settled status of its
    /// [`Self::setting`]; before it, the catalog is in that setting's transitional status.
    pub settled_at: SystemTime,
    /// The `ClientToken` of the write that left this record, or `None` where it carried
    /// none: "A unique, case-sensitive identifier that you provide to ensure the idempotency
    /// of the request", so a retry carrying the same token is answered with this record
    /// rather than treated as a second change.
    pub client_token: Option<String>,
}

impl DataCatalogExportConfigurationEntry {
    /// The record a FIRST `PutDataCatalogExportConfiguration` on a catalog writes at `now`.
    ///
    /// A catalog with no record of its own is not exporting — that is what a read of it
    /// reports ([`unconfigured_data_catalog_export`]) — so this write CHANGES the setting only
    /// where it enables the export, and only then is there a transition for the clock to owe
    /// ([`EXPORT_CONFIGURATION_SETTLES_AFTER`]). A first write asking for the export to be off
    /// leaves the catalog where it already was, so its status is settled from the start.
    pub fn from_put(
        setting: &str,
        encryption_configuration: Option<&ExportEncryptionConfiguration>,
        client_token: Option<&str>,
        now: SystemTime,
    ) -> Self {
        let settled_at = if setting == EXPORT_SETTING_ENABLED {
            now.checked_add(EXPORT_CONFIGURATION_SETTLES_AFTER)
                .unwrap_or(now)
        } else {
            now
        };
        Self {
            setting: setting.to_string(),
            encryption_configuration: encryption_configuration.cloned(),
            created_at: now,
            updated_at: now,
            settled_at,
            client_token: client_token.map(str::to_string),
        }
    }

    /// Apply the write `setting` asks for to the record already stored, as served at `now`.
    ///
    /// The whole configuration is replaced, as a put of one is: a write that names no
    /// encryption configuration leaves the record carrying none rather than what an earlier
    /// write put there. [`Self::created_at`] stays where it is — the configuration was
    /// created by the first write, not by this one.
    ///
    /// The transition is re-armed only where the write CHANGES the setting: that is the
    /// change the service has to carry out, while a write that asks for the setting already
    /// in effect leaves the catalog where it already is.
    pub fn apply_put(
        &mut self,
        setting: &str,
        encryption_configuration: Option<&ExportEncryptionConfiguration>,
        client_token: Option<&str>,
        now: SystemTime,
    ) {
        if self.setting != setting {
            self.settled_at = now
                .checked_add(EXPORT_CONFIGURATION_SETTLES_AFTER)
                .unwrap_or(now);
            self.setting = setting.to_string();
        }
        self.encryption_configuration = encryption_configuration.cloned();
        self.client_token = client_token.map(str::to_string);
        self.updated_at = now;
    }

    /// The `ExportStatus` this catalog is in at `at`: the transitional status of its setting
    /// while the due instant is still ahead, the settled one from the instant it is reached.
    pub fn status(&self, at: SystemTime) -> &'static str {
        match (self.exports(), self.is_changing(at)) {
            (true, true) => EXPORT_STATUS_ENABLING,
            (true, false) => EXPORT_STATUS_ENABLED,
            (false, true) => EXPORT_STATUS_DISABLING,
            (false, false) => EXPORT_STATUS_DISABLED,
        }
    }

    /// Whether the stored setting asks for the catalog's metadata to be exported.
    pub fn exports(&self) -> bool {
        self.setting == EXPORT_SETTING_ENABLED
    }

    /// Whether the change the last write asked for is still being applied at `at`, i.e. the
    /// catalog is in a transitional status.
    pub fn is_changing(&self, at: SystemTime) -> bool {
        at < self.settled_at
    }

    /// Whether this record was left by a write carrying `client_token`, which is what makes
    /// a request carrying that token a RETRY of it rather than a new change.
    pub fn was_written_with(&self, client_token: &str) -> bool {
        self.client_token.as_deref() == Some(client_token)
    }

    /// This configuration as `GetDataCatalogExportConfiguration` reports it at `at`.
    ///
    /// `s3_table_bucket_arn` — "The ARN of the S3 Tables bucket where catalog metadata is
    /// exported" — is the bucket S3 TABLES holds for this account
    /// ([`export_table_bucket_arn`]), so the read names a bucket only while the service that
    /// owns it answers for one, and `None` where it holds none.
    pub fn to_output(
        &self,
        at: SystemTime,
        s3_table_bucket_arn: Option<String>,
    ) -> crate::ops::types::GetDataCatalogExportConfigurationOutput {
        crate::ops::types::GetDataCatalogExportConfigurationOutput {
            export_setting: Some(self.setting.clone()),
            status: Some(self.status(at).to_string()),
            encryption_configuration: self.encryption_configuration.clone(),
            s3_table_bucket_arn,
            created_at: Some(to_date_time(self.created_at)),
            updated_at: Some(to_date_time(self.updated_at)),
            // Neither of the other two exports can be asked for, so each reports the state
            // it is in, which is off ([`UNEXPORTED`]).
            data_quality_export_setting: Some(UNEXPORTED.0.to_string()),
            data_quality_export_status: Some(UNEXPORTED.1.to_string()),
            lineage_export_setting: Some(UNEXPORTED.0.to_string()),
            lineage_export_status: Some(UNEXPORTED.1.to_string()),
        }
    }
}

/// The export configuration a catalog NOTHING has configured reports.
///
/// Every account HAS a Data Catalog ([`account_data_catalog`]), so a read of its export
/// configuration reports the configuration that catalog is in rather than missing an entity:
/// nothing has asked for its metadata to be exported, so the export is off
/// ([`EXPORT_SETTING_DISABLED`], [`EXPORT_STATUS_DISABLED`]) — the same way a catalog no
/// write has configured reports each of its encryption settings' defaults
/// ([`ComAmazonawsGlueAwsglueState::data_catalog_encryption_settings`]).
///
/// It carries NO instants: `CreatedAt` and `UpdatedAt` are the instants of the writes to the
/// configuration, and there has been no write to timestamp — exactly as the account's own
/// Data Catalog carries no create instant because no create wrote it. Nor an encryption
/// configuration, since nothing is being exported to encrypt.
pub fn unconfigured_data_catalog_export() -> crate::ops::types::GetDataCatalogExportConfigurationOutput
{
    crate::ops::types::GetDataCatalogExportConfigurationOutput {
        export_setting: Some(EXPORT_SETTING_DISABLED.to_string()),
        status: Some(EXPORT_STATUS_DISABLED.to_string()),
        encryption_configuration: None,
        s3_table_bucket_arn: None,
        created_at: None,
        updated_at: None,
        data_quality_export_setting: Some(UNEXPORTED.0.to_string()),
        data_quality_export_status: Some(UNEXPORTED.1.to_string()),
        lineage_export_setting: Some(UNEXPORTED.0.to_string()),
        lineage_export_status: Some(UNEXPORTED.1.to_string()),
    }
}

/// The S3 TABLES store the account's table buckets live in.
///
/// It is the S3 Tables service's OWN state struct, reached through the SAME shared engine
/// this service's store lives in — exactly as the account's Identity Center instances are
/// ([`IdentityCenterState`]) — so the bucket the catalog export writes into is the very
/// bucket `ListTableBuckets` reports and `GetTableBucketEncryption` answers for, and not a
/// second copy of it kept here.
pub type S3TablesState =
    ::service_com_amazonaws_s3tables_s3tablebuckets::state::ComAmazonawsS3tablesS3tablebucketsState;

/// The name of the table bucket the Data Catalog exports its metadata into.
///
/// The SERVICE's own name for that bucket, not a value from any request: neither operation
/// that addresses the export configuration carries a bucket, so the export has one bucket per
/// account and Region and this is what it is called — the name the refusal of an export
/// configuration that disagrees with the bucket already there states
/// ([`messages::export_table_bucket_encryption_conflict`]).
pub const EXPORT_TABLE_BUCKET_NAME: &str = "aws-catalog";

/// The ARN of the table bucket the catalog export writes into IN `region`, as S3 Tables holds
/// it, or `None` when that service holds no such bucket for this account — including an
/// emulator with no S3 Tables mounted to ask.
///
/// A READ of the owning service's store: the bucket is an S3 Tables resource, so whether one
/// exists is answered by the service that owns it rather than claimed here.
pub fn export_table_bucket_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> Option<String> {
    use ::service_com_amazonaws_s3tables_s3tablebuckets::state as s3tables;

    let arn = s3tables::table_bucket_arn(region, EXPORT_TABLE_BUCKET_NAME);
    let state = s3tables::service_state(engine)?;
    let held = state.lock().table_buckets.contains_key(&arn);
    held.then_some(arn)
}

/// The encryption the table bucket of [`EXPORT_TABLE_BUCKET_NAME`] in `region` is protected
/// by, as its own service reports it — the configuration set on it, else that service's
/// default for a bucket nothing configured — or `None` when there is no such bucket.
///
/// The pair is `(SseAlgorithm, KmsKeyArn)`, in the spelling both services put those values on
/// the wire in (`AES256` / `aws:kms`), which is what makes the comparison with a request's
/// `ExportEncryptionConfiguration` a comparison of like with like.
pub fn export_table_bucket_encryption<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> Option<(String, Option<String>)> {
    use ::service_com_amazonaws_s3tables_s3tablebuckets::state as s3tables;

    let arn = s3tables::table_bucket_arn(region, EXPORT_TABLE_BUCKET_NAME);
    let state = s3tables::service_state(engine)?;
    let store = state.lock();
    let bucket = store.table_buckets.get(&arn)?;
    let encryption = s3tables::effective_bucket_encryption(bucket.encryption.as_ref());
    Some((
        encryption.sse_algorithm.as_wire().to_string(),
        encryption.kms_key_arn.clone(),
    ))
}

/// Whether `requested` asks for an encryption the bucket already there does not use.
///
/// Only the members the request NAMES are compared: a write that names an algorithm or a key
/// is asking for that one, while a member it leaves unset asks for nothing — "If not
/// specified, the default encryption settings are used" — and so disagrees with nothing the
/// bucket holds.
pub fn export_encryption_differs(
    requested: &ExportEncryptionConfiguration,
    bucket_sse_algorithm: &str,
    bucket_kms_key_arn: Option<&str>,
) -> bool {
    let algorithm_differs = requested
        .sse_algorithm
        .as_deref()
        .is_some_and(|algorithm| algorithm != bucket_sse_algorithm);
    let key_differs = requested
        .kms_key_arn
        .as_deref()
        .is_some_and(|key| Some(key) != bucket_kms_key_arn);
    algorithm_differs || key_differs
}

/// Create the table bucket the catalog export writes into, in `region` and under
/// `encryption_configuration`, unless S3 Tables already holds it; answer its ARN, or `None`
/// when that service cannot take the bucket — an emulator with no S3 Tables mounted, or an
/// account already at that service's own bucket ceiling.
///
/// The bucket is created in the SERVICE THAT OWNS IT, so the ARN this answers is one
/// `GetTableBucket` resolves and `ListTableBuckets` reports. It is created as a bucket Amazon
/// Web Services made on the account's behalf
/// (`TableBucketType::Aws`), because no caller asked for it: enabling the catalog export is
/// what brings it into existence. An existing bucket is LEFT as it is — its encryption is the
/// bucket's own and is not re-set by an export configuration, which is what makes a write
/// naming a different one a conflict ([`export_encryption_differs`]).
pub fn create_export_table_bucket<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    encryption_configuration: Option<&ExportEncryptionConfiguration>,
) -> Option<String> {
    use ::service_com_amazonaws_s3tables_s3tablebuckets::state as s3tables;

    let arn = s3tables::table_bucket_arn(region, EXPORT_TABLE_BUCKET_NAME);
    let state = s3tables::service_state(engine)?;
    let mut store = state.lock();
    if store.table_buckets.contains_key(&arn) {
        return Some(arn);
    }
    if store.is_at_bucket_quota() {
        return None;
    }
    let encryption = encryption_configuration
        .and_then(|configuration| configuration.sse_algorithm.as_deref())
        .and_then(s3tables::SseAlgorithm::from_wire)
        .map(|sse_algorithm| s3tables::BucketEncryption {
            sse_algorithm,
            kms_key_arn: encryption_configuration
                .and_then(|configuration| configuration.kms_key_arn.clone()),
        });
    let bucket_id = store.mint_bucket_id();
    let now = SystemTime::now();
    store.table_buckets.insert(
        arn.clone(),
        s3tables::TableBucket {
            name: EXPORT_TABLE_BUCKET_NAME.to_string(),
            arn: arn.clone(),
            region: region.to_string(),
            owner_account_id: ::engine::emulator_account_id().to_string(),
            bucket_id,
            bucket_type: s3tables::TableBucketType::Aws,
            created_at: now,
            encryption,
            // The export asks for no storage class, no metrics, no replication, no policy,
            // no maintenance configuration and no tags, so the bucket carries none of them:
            // each is reported from what a caller of that service asked for, and this
            // bucket's caller asked for nothing but its encryption.
            storage_class: None,
            metrics: None,
            replication: None,
            policy: None,
            maintenance: BTreeMap::new(),
            // Nor any maintenance schedule of its own: every task has been
            // eligible to run on the bucket since it was created, under its
            // default status.
            maintenance_schedules: BTreeMap::new(),
            tags: BTreeMap::new(),
            // A bucket S3 Tables has just taken holds no namespaces: what a namespace or a
            // table in it is created by is that service's own writes.
            namespaces: BTreeMap::new(),
            // Nor does it deliver any server access logs: a put on that service's own
            // logging surface is what turns them on.
            logging: None,
        },
    );
    Some(arn)
}

