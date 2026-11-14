// SPDX-License-Identifier: CC0-1.0

//! WHO OWNS the Secrets Manager secret RDS manages for a master user password.
//!
//! Minting, rotating and deleting that secret live in [`super::managed_master_secret`];
//! this module answers the one question that module does not: whose the secret IS once it
//! exists. The secret belongs to the resource it was minted FOR, and two things follow
//! from that:
//!
//! * it goes WITH that resource — `DeleteDBInstance` deletes the secret the instance it is
//!   deleting owns, so nothing of it outlives the database whose credential it holds
//!   ([`owned_master_user_secret`]);
//! * a resource that merely INHERITS another's master credential — a read replica of it,
//!   an instance restored from a snapshot of it — owns none, so it stores and reports no
//!   `MasterUserSecret`, and deleting it deletes nothing (the rule is written on
//!   [`DbInstanceRecord::master_user_secret`], where a create that builds its record from
//!   another's finds it).
//!
//! Split out of [`super`] like the other record concerns; every name here is re-exported
//! from it, so a handler reads them as `crate::state::<name>`.

use super::*;

/// The managed master user secret `record` OWNS, as the ARN a delete of that instance
/// hands to Secrets Manager.
///
/// `None` for an instance whose master user password the caller supplied, and for one that
/// inherited another instance's credential: neither owns a secret, so neither delete
/// deletes one.
pub fn owned_master_user_secret(record: &DbInstanceRecord) -> Option<String> {
    record
        .master_user_secret
        .as_ref()
        .map(|secret| secret.secret_arn.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two answers `DeleteDBInstance` depends on: the ARN of the secret an instance
    /// manages, and nothing at all for an instance that manages none — which is the state
    /// an instance that inherited another's credential is in.
    #[test]
    fn an_instance_owns_the_secret_it_manages_and_no_other() {
        let mut record = a_db_instance();
        assert_eq!(
            owned_master_user_secret(&record),
            None,
            "an instance whose master user password the caller supplied owns no secret"
        );
        let arn = "arn:aws:secretsmanager:us-west-2:000000000000:secret:rds-db-AAAA-abcdef";
        record.master_user_secret = Some(ManagedMasterSecret::minted(
            arn.to_string(),
            DEFAULT_KMS_KEY_FOR_TESTS.to_string(),
        ));
        assert_eq!(
            owned_master_user_secret(&record).as_deref(),
            Some(arn),
            "an instance that manages a secret owns that one secret"
        );
    }

    /// The key a managed secret this test mints is encrypted under; the value is immaterial
    /// to ownership, which is what the test is about.
    const DEFAULT_KMS_KEY_FOR_TESTS: &str = "alias/aws/secretsmanager";

    /// One stored DB instance, as `CreateDBInstance` would have written it for a MySQL
    /// instance whose password the caller supplied: it manages no secret.
    fn a_db_instance() -> DbInstanceRecord {
        DbInstanceRecord {
            identifier: "emu-owned".to_string(),
            region: "us-west-2".to_string(),
            dbi_resource_id: "db-AAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
            endpoint_host: "aaaaaaaaaaaa".to_string(),
            db_instance_class: "db.t3.micro".to_string(),
            engine: "mysql".to_string(),
            engine_version: None,
            status: DB_INSTANCE_STATUS_AVAILABLE.to_string(),
            pending_status: None,
            db_name: None,
            allocated_storage: Some(20),
            max_allocated_storage: None,
            master_username: Some("emuadmin".to_string()),
            master_user_password: None,
            master_user_secret: None,
            endpoint_port: 3306,
            served_endpoint: None,
            multi_az: false,
            storage_type: "gp3".to_string(),
            storage_encrypted: false,
            kms_key_id: None,
            iops: None,
            license_model: None,
            availability_zone: "us-west-2a".to_string(),
            backup_retention_period: 1,
            preferred_backup_window: None,
            preferred_maintenance_window: None,
            publicly_accessible: false,
            auto_minor_version_upgrade: true,
            copy_tags_to_snapshot: false,
            deletion_protection: false,
            db_subnet_group_name: None,
            db_parameter_group_name: None,
            option_group_name: None,
            vpc_security_group_ids: Vec::new(),
            db_security_group_names: Vec::new(),
            db_cluster_identifier: None,
            read_replica_source_db_instance_identifier: None,
            read_replica_db_instance_identifiers: Vec::new(),
            domain: None,
            ca_certificate_identifier: DEFAULT_CA_CERTIFICATE_IDENTIFIER.to_string(),
            monitoring_interval: 0,
            performance_insights_enabled: false,
            performance_insights_kms_key_id: None,
            performance_insights_retention_period: None,
            iam_database_authentication_enabled: false,
            database_insights_mode: DEFAULT_DATABASE_INSIGHTS_MODE.to_string(),
            dedicated_log_volume: false,
            customer_owned_ip_enabled: false,
            network_type: DEFAULT_NETWORK_TYPE.to_string(),
            backup_target: DEFAULT_BACKUP_TARGET.to_string(),
            engine_lifecycle_support: DEFAULT_ENGINE_LIFECYCLE_SUPPORT.to_string(),
            storage_throughput: 125,
            instance_create_time: now_as_wire_timestamp(),
            engine_started_at: now_as_wire_timestamp(),
            tags: Vec::new(),
            associated_roles: Vec::new(),
            deferred_modification: None,
            engine_log: InstanceLog::default(),
        }
    }
}
