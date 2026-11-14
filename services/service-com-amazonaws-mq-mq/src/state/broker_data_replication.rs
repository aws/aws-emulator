// SPDX-License-Identifier: CC0-1.0

//! Whether a broker takes part in CROSS-REGION DATA REPLICATION, and in which role.
//!
//! RULE: only a broker whose data replication mode is `CRDR` is one of a replication
//! pair, and only such a broker can be promoted; every other broker is refused a
//! promotion, in Amazon MQ's own words, and is left exactly as it was.
//!
//! `DataReplicationMetadataOutput` is documented "The replication details of the data
//! replication-enabled broker. Only returned if dataReplicationMode or
//! pendingDataReplicationMode is set to CRDR", its `DataReplicationRole` "Defines the
//! role of this broker in a data replication pair. When a replica broker is promoted to
//! primary, this role is interchanged", and its `DataReplicationCounterpart` "Describes
//! the replica/primary broker" by `BrokerId` and `Region`. So the role is held on the
//! broker, and the counterpart is DERIVED from the primary broker ARN the create named:
//! that ARN is the one record of which broker in which Region this one replicates.

use super::*;

/// The `DataReplicationMode` of a broker that replicates across Regions. From
/// `DataReplicationMode`'s modelled values (`DATA_REPLICATION_MODE_VALUES`).
pub const DATA_REPLICATION_MODE_CRDR: &str = "CRDR";

/// The `DataReplicationRole` of the broker a pair replicates FROM.
pub const DATA_REPLICATION_ROLE_PRIMARY: &str = "PRIMARY";

/// The `DataReplicationRole` of the broker a pair replicates TO, which is the role a
/// broker created against a primary broker's ARN starts in.
pub const DATA_REPLICATION_ROLE_REPLICA: &str = "REPLICA";

/// How Amazon MQ refuses a promotion of a broker that is not part of a replication
/// pair. The text is wholly fixed: nothing in it comes from the request.
pub const NOT_CROSS_REGION_REPLICATED_MESSAGE: &str =
    "This broker is not configured for cross Region data replication.";

/// The vendor (service namespace) field every Amazon MQ resource ARN carries: this
/// service's own `aws.api#service$arnNamespace`
/// (`smithy_query(query="service_info")` — `arnNamespace: "mq"`). A counterpart named by
/// an ARN of any other vendor is no broker of this service.
pub const ARN_VENDOR: &str = "mq";

/// The resource type a broker's ARN names, as this service mints one
/// (`broker:<name>:<id>`) and as Amazon MQ writes one (`broker:<id>`).
pub const ARN_RESOURCE_TYPE_BROKER: &str = "broker";

impl Broker {
    /// Whether this broker is configured for cross-Region data replication at all.
    pub fn replicates_cross_region(&self) -> bool {
        self.data_replication_mode.as_deref() == Some(DATA_REPLICATION_MODE_CRDR)
    }

    /// This broker's role in its replication pair, or `None` for a broker that is not in
    /// one.
    ///
    /// A broker created against a primary broker's ARN is that broker's REPLICA until
    /// something promotes it ([`Broker::promote_to_primary`]).
    pub fn data_replication_role(&self) -> Option<&str> {
        if !self.replicates_cross_region() {
            return None;
        }
        Some(
            self.data_replication_role
                .as_deref()
                .unwrap_or(DATA_REPLICATION_ROLE_REPLICA),
        )
    }

    /// Make this broker the PRIMARY of its pair, which is what a promotion does: the
    /// role of the two brokers "is interchanged", so the one promoted holds the role it
    /// did not hold before.
    pub fn promote_to_primary(&mut self) {
        self.data_replication_role = Some(DATA_REPLICATION_ROLE_PRIMARY.to_string());
    }

    /// The broker this one replicates with, as `DataReplicationCounterpart` names it:
    /// its `BrokerId` and its `Region`, read off the primary broker ARN this broker's
    /// create named.
    ///
    /// The ARN itself is read by the one shared parse (`service_common::arn`), held to
    /// this service's own rule: a counterpart is a BROKER of Amazon MQ's own namespace,
    /// so an ARN of another vendor or naming another resource type names no counterpart.
    ///
    /// `None` when the broker names no counterpart ARN, or when that ARN carries no
    /// Region and broker id to name one with — the member is reported only when there
    /// IS a counterpart to report, never as an invented pair.
    pub fn data_replication_counterpart(&self) -> Option<(String, String)> {
        let arn = self.data_replication_primary_broker_arn.as_deref()?;
        let region = ::service_common::arn::region_of(arn, ARN_VENDOR)?;
        let resource =
            ::service_common::arn::resource_id_of_type(arn, ARN_RESOURCE_TYPE_BROKER)?;
        // A broker's resource field is written `broker:<name>:<id>` or `broker:<id>`, so
        // the id is the last segment of whichever form the ARN carries.
        let broker_id = resource.rsplit([':', '/']).next().unwrap_or(resource);
        (!broker_id.is_empty()).then(|| (broker_id.to_string(), region.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the counterpart a broker reports is the BROKER of Amazon MQ's own namespace
    /// named by the primary broker ARN it was created against — its Region from that
    /// ARN's Region field and its id from the broker segment of its resource — and an ARN
    /// that names no such broker reports no counterpart rather than a made-up one.
    #[test]
    fn a_replicas_counterpart_is_read_off_the_primary_broker_arn() {
        // Rows span the ARN forms this member can carry: the resource form this service
        // mints (`broker:<name>:<id>`), the shorter `broker:<id>` form, the `broker/<id>`
        // separator AWS also writes, another partition, and the values that name no
        // broker of this service at all.
        // One row: what it is about, the ARN the broker records, and the counterpart
        // that ARN names (its broker id and Region), or none.
        type Row = (&'static str, &'static str, Option<(&'static str, &'static str)>);
        let rows: &[Row] = &[
            (
                "the form this emulator mints",
                "arn:aws:mq:us-west-2:123456789012:broker:primary:b-1111",
                Some(("b-1111", "us-west-2")),
            ),
            (
                "a resource naming only the id",
                "arn:aws:mq:eu-central-1:123456789012:broker:b-2222",
                Some(("b-2222", "eu-central-1")),
            ),
            (
                "the slash separator",
                "arn:aws:mq:ap-southeast-2:123456789012:broker/b-5555",
                Some(("b-5555", "ap-southeast-2")),
            ),
            (
                "another partition",
                "arn:aws-cn:mq:cn-north-1:123456789012:broker:p:b-3333",
                Some(("b-3333", "cn-north-1")),
            ),
            (
                "another resource type of this service",
                "arn:aws:mq:us-east-1:123456789012:configuration:c-1",
                None,
            ),
            (
                "another vendor's resource",
                "arn:aws:s3:us-east-1:123456789012:broker:b-6666",
                None,
            ),
            ("not an ARN at all", "primary-broker", None),
            (
                "an ARN with no Region",
                "arn:aws:mq::123456789012:broker:p:b-4444",
                None,
            ),
            (
                "a broker resource with no id",
                "arn:aws:mq:us-east-1:123456789012:broker",
                None,
            ),
        ];
        for (case, arn, expected) in rows {
            let broker = replica_of(arn);
            let counterpart = broker.data_replication_counterpart();
            let actual = counterpart
                .as_ref()
                .map(|(id, region)| (id.as_str(), region.as_str()));
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: only a CRDR broker has a role, and a promotion is what interchanges it.
    #[test]
    fn only_a_replicating_broker_has_a_role_and_a_promotion_interchanges_it() {
        let mut replica = replica_of("arn:aws:mq:us-west-2:123456789012:broker:primary:b-1111");
        assert_eq!(
            replica.data_replication_role(),
            Some(DATA_REPLICATION_ROLE_REPLICA),
            "case \"a broker created against a primary\": expected the replica role, got \
             {:?}",
            replica.data_replication_role()
        );
        replica.promote_to_primary();
        assert_eq!(
            replica.data_replication_role(),
            Some(DATA_REPLICATION_ROLE_PRIMARY),
            "case \"a promoted replica\": expected the primary role, got {:?}",
            replica.data_replication_role()
        );

        let mut alone = replica.clone();
        alone.data_replication_mode = None;
        alone.data_replication_primary_broker_arn = None;
        alone.data_replication_role = None;
        assert_eq!(
            alone.data_replication_role(),
            None,
            "case \"a broker in no pair\": expected no role, got {:?}",
            alone.data_replication_role()
        );
        assert!(
            !alone.replicates_cross_region(),
            "case \"a broker in no pair\": it must not report cross-Region replication"
        );
    }

    /// A stored broker that replicates from `primary_arn`, with every other member at a
    /// value these cases do not read.
    fn replica_of(primary_arn: &str) -> Broker {
        Broker {
            broker_id: "b-9999".to_string(),
            broker_arn: "arn:aws:mq:us-east-1:123456789012:broker:replica:b-9999".to_string(),
            broker_name: "replica".to_string(),
            engine_type: ENGINE_TYPE_ACTIVEMQ.to_string(),
            engine_version: "5.18.7".to_string(),
            deployment_mode: DEPLOYMENT_MODE_SINGLE_INSTANCE.to_string(),
            host_instance_type: "mq.t3.micro".to_string(),
            publicly_accessible: false,
            auto_minor_version_upgrade: true,
            authentication_strategy: DEFAULT_AUTHENTICATION_STRATEGY.to_string(),
            storage_type: STORAGE_TYPE_EBS.to_string(),
            storage_size: None,
            security_groups: Vec::new(),
            subnet_ids: Vec::new(),
            data_replication_mode: Some(DATA_REPLICATION_MODE_CRDR.to_string()),
            data_replication_primary_broker_arn: Some(primary_arn.to_string()),
            created: SystemTime::UNIX_EPOCH,
            available_at: SystemTime::UNIX_EPOCH,
            creator_request_id: None,
            creation_fingerprint: String::new(),
            users: BTreeMap::new(),
            tags: BTreeMap::new(),
            logs: BrokerLogs::default(),
            encryption_options: BrokerEncryptionOptions::aws_owned_key(),
            maintenance_window_start_time: assigned_maintenance_window("b-9999"),
            ldap_server_metadata: None,
            configuration: BrokerConfigurationRef {
                id: "c-1".to_string(),
                revision: FIRST_CONFIGURATION_REVISION,
            },
            restart_completes_at: None,
            data_replication_role: None,
            published_host: "127.0.0.1".to_string(),
        }
    }
}
