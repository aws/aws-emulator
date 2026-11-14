// SPDX-License-Identifier: CC0-1.0

//! The tenant database records `CreateTenantDatabase`, `DescribeTenantDatabases` and
//! `DeleteTenantDatabase` share.
//!
//! A tenant database is a pluggable database INSIDE a DB instance running one of the
//! Oracle container database (CDB) engines, so every record here names the DB instance it
//! lives in and is reached only through a DB instance this service already holds.

use std::time::Instant;

use super::{ManagedMasterSecret, PendingChange, PendingStatus, ARN_NAMESPACE};
use crate::ops::types::{Tag, TenantDatabase};

/// How many tenant databases one DB instance holds. Amazon RDS publishes a per-instance
/// limit for the Oracle multi-tenant configuration; the cap here is this emulator's, and
/// it bounds the store the same way the per-region quotas in `src/state.rs` do.
pub const TENANT_DATABASE_QUOTA_PER_INSTANCE: usize = 10;

/// The status a just-created tenant database reports while this service brings it up.
pub const TENANT_DATABASE_STATUS_CREATING: &str = "creating";

/// The status a tenant database reports once it is usable.
pub const TENANT_DATABASE_STATUS_AVAILABLE: &str = "available";

/// The status a tenant database reports once a delete has been accepted for it.
pub const TENANT_DATABASE_STATUS_DELETING: &str = "deleting";

/// The ARN resource type of a tenant database.
const ARN_RESOURCE_TYPE: &str = "tenant-database";

/// Whether a DB instance running `engine` uses the MULTI-TENANT configuration, which is
/// what decides whether it can hold tenant databases at all.
///
/// `CreateDBInstance$MultiTenant`: "Specifies whether to use the multi-tenant
/// configuration or the single-tenant configuration (default). This parameter only applies
/// to RDS for Oracle container database (CDB) engines." The CDB engines are the ones whose
/// `Engine` value ends in `-cdb` (`oracle-ee-cdb`, `oracle-se2-cdb` and their `custom-`
/// forms), so the architecture follows from the engine.
pub fn is_multi_tenant_engine(engine: &str) -> bool {
    engine.to_ascii_lowercase().ends_with("-cdb")
}

/// The second half of a tenant database's store key: the DB instance it lives in and its
/// own name, so one region's tenant databases are ordered by instance and then by name and
/// a scan of one instance's tenants is an ordered range.
pub fn tenant_database_key(db_instance_identifier: &str, tenant_db_name: &str) -> String {
    format!("{db_instance_identifier}/{tenant_db_name}")
}

/// The immutable resource id reported as `TenantDatabaseResourceId`, of the same minted
/// alphabet as the other server-assigned ids this service hands out.
pub fn mint_tenant_database_resource_id(mint: u64) -> String {
    format!("tdb-{}", super::minted_token("tenant-database", mint, 26))
}

/// The name of the secret RDS manages for a TENANT DATABASE's master user password: the
/// tenant database is addressed by its DB instance and its own name, which is what names
/// the secret, carrying the instance's immutable `DbiResourceId` — under the same reserved
/// prefix every secret RDS manages carries; see
/// [`super::cluster_master_user_secret_name`] for the form these names share.
pub fn tenant_database_master_user_secret_name(
    dbi_resource_id: &str,
    tenant_db_name: &str,
) -> String {
    format!(
        "{}tenantdb-{dbi_resource_id}-{tenant_db_name}",
        super::MANAGED_SECRET_NAME_PREFIX
    )
}

/// A persisted tenant database.
#[derive(Debug, Clone)]
pub struct TenantDatabaseRecord {
    /// The region whose credential scope the create was signed with.
    pub region: String,
    /// The `DBInstanceIdentifier` of the instance this tenant database lives in.
    pub db_instance_identifier: String,
    /// `TenantDBName`.
    pub tenant_db_name: String,
    /// The minted `TenantDatabaseResourceId`.
    pub resource_id: String,
    /// The `DbiResourceId` of the instance this tenant database lives in, which is what
    /// `TenantDatabase$DbiResourceId` reports.
    pub dbi_resource_id: String,
    /// `MasterUsername`.
    pub master_username: String,
    /// `MasterUserSecret`: the secret RDS manages for this tenant database's master user
    /// password, for one created with `ManageMasterUserPassword` turned on; `None` for one
    /// whose password the caller supplied, which has no managed secret to report.
    pub master_user_secret: Option<ManagedMasterSecret>,
    /// `CharacterSetName`.
    pub character_set_name: Option<String>,
    /// `NcharCharacterSetName`.
    pub nchar_character_set_name: Option<String>,
    /// `Status`.
    pub status: String,
    /// The status change the clock owes this record, if any.
    pub pending_status: Option<PendingStatus>,
    /// `TenantDatabaseCreateTime`.
    pub create_time: ::aws_smithy_types::DateTime,
    /// `TagList`.
    pub tags: Vec<Tag>,
}

impl TenantDatabaseRecord {
    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, so the caller drops it from the store.
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        if self.pending_status.as_ref().is_some_and(|p| now >= p.due_at) {
            let pending = self.pending_status.take().expect("checked just above");
            match pending.change {
                PendingChange::Status(status) => self.status = status,
                PendingChange::Removal => return true,
            }
        }
        false
    }

    /// `TenantDatabaseARN`, from the stored region and the minted resource id — the id
    /// rather than the name, because the name can be changed and the ARN cannot.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("{ARN_RESOURCE_TYPE}:{}", self.resource_id),
        )
    }

    /// Whether a delete has already been accepted for this tenant database.
    pub fn is_deleting(&self) -> bool {
        self.status == TENANT_DATABASE_STATUS_DELETING
    }

    /// Accept a delete: the tenant database reports `deleting` from now on and the clock
    /// owes its removal from the store. Nothing sleeps — the due instant is stored.
    pub fn begin_deletion(&mut self) {
        self.status = TENANT_DATABASE_STATUS_DELETING.to_string();
        self.pending_status = Some(PendingStatus::removal_in_a_moment());
    }

    /// Project the stored record onto the modelled `TenantDatabase` shape.
    ///
    /// `DeletionProtection` is "whether deletion protection is enabled for the DB
    /// INSTANCE" — a property of the instance this tenant database lives in, so it is
    /// passed in from that record rather than stored a second time here.
    ///
    /// `PendingModifiedValues` is left out rather than invented: nothing here defers a
    /// change to a maintenance window. `MasterUserSecret` is reported for a tenant database
    /// whose master user password RDS manages, and only for one — one whose password the
    /// caller supplied has no secret of RDS's own.
    pub fn to_wire(&self, deletion_protection: bool) -> TenantDatabase {
        TenantDatabase {
            tenant_database_create_time: Some(self.create_time),
            db_instance_identifier: Some(self.db_instance_identifier.clone()),
            tenant_db_name: Some(self.tenant_db_name.clone()),
            status: Some(self.status.clone()),
            master_username: Some(self.master_username.clone()),
            dbi_resource_id: Some(self.dbi_resource_id.clone()),
            tenant_database_resource_id: Some(self.resource_id.clone()),
            tenant_database_arn: Some(self.arn()),
            character_set_name: self.character_set_name.clone(),
            nchar_character_set_name: self.nchar_character_set_name.clone(),
            deletion_protection: Some(deletion_protection),
            pending_modified_values: None,
            master_user_secret: self
                .master_user_secret
                .as_ref()
                .map(ManagedMasterSecret::to_wire),
            tag_list: Some(self.tags.clone()),
        }
    }
}
