// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mq-mq service, plus the record types and
//! the small helpers more than one operation needs.
//!
//! The crate's `lib.rs` is generated and declares exactly four modules
//! (`ops`, `scaffolded`, `service`, `state`), so a helper several operations
//! share — the broker records, the `ListBrokers` page-token codec, and the
//! model's documented value constraints — lives here rather than in a module
//! `lib.rs` could not name.
//!
//! The readings a BROKER's own answer needs, each one concern, live in this
//! module's own files: [`broker_published_values`] (what the service decides for a
//! broker its create did not), [`broker_endpoints`] (where a client reaches a running
//! broker) and [`broker_configuration`] (the configuration a broker runs). The form a
//! configuration revision's document is stored in lives in
//! [`configuration_document`].

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime};

/// Which configuration revision a broker runs, and where it comes from.
mod broker_configuration;
/// The form a configuration revision's document is stored in.
mod configuration_document;
/// Whether a broker takes part in cross-Region data replication, and in which role.
mod broker_data_replication;
/// Where a client reaches a running broker: its console and its wire endpoints.
mod broker_endpoints;
/// The values the service decides for a broker rather than echoing from its create.
mod broker_published_values;
/// Restarting a broker, and the user changes a restart applies.
mod broker_restart;

pub use broker_configuration::*;
pub use broker_data_replication::*;
pub use broker_endpoints::*;
pub use broker_restart::*;
pub use configuration_document::*;
pub use broker_published_values::*;

/// Region used for an ARN when the request carries no SigV4 credential scope to
/// read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after `CreateBroker` a broker stops reporting
/// `CREATION_IN_PROGRESS` and reports `RUNNING`.
///
/// `CreateBroker` is documented as asynchronous ("Note: This API is
/// asynchronous"), so the broker it mints is returned in a non-terminal state
/// and the move to `RUNNING` is owed by the clock, not by any later call. Real
/// provisioning takes minutes; the emulator picks a span short enough to test
/// yet non-zero, so a caller that describes the broker immediately still
/// observes the transient state.
pub const BROKER_PROVISIONING: Duration = Duration::from_millis(500);

/// `BrokerState` while the broker is still being provisioned.
/// From `BrokerState`'s modelled values (`BROKER_STATE_VALUES`).
pub const BROKER_STATE_CREATION_IN_PROGRESS: &str = "CREATION_IN_PROGRESS";

/// `BrokerState` once provisioning is done.
pub const BROKER_STATE_RUNNING: &str = "RUNNING";

/// The `AuthenticationStrategy` applied when the request omits it — the model
/// documents "The default is SIMPLE".
pub const DEFAULT_AUTHENTICATION_STRATEGY: &str = "SIMPLE";

/// Applied to `AutoMinorVersionUpgrade` when the request omits it — the model
/// documents "Set to true by default, if no value is specified".
pub const DEFAULT_AUTO_MINOR_VERSION_UPGRADE: bool = true;

/// Amazon MQ's default per-Region, per-account broker quota (Amazon MQ
/// Developer Guide, "Quotas in Amazon MQ": 50 brokers per Region). It is also
/// the bound that keeps the store from growing with the number of requests a
/// caller makes: a create past it is refused rather than persisted.
///
/// The store holds one broker map rather than one per Region, so this quota and
/// the broker-name namespace are account-wide here; the emulator serves one
/// account and the Region only reaches a broker through its ARN.
pub const MAX_BROKERS: usize = 50;

/// Amazon MQ's documented per-broker user quota (same quota table: 250 users
/// per ActiveMQ broker), and the bound on one broker record's `users` map.
pub const MAX_USERS_PER_BROKER: usize = 250;

/// A broker is created together with its initial user, so `CreateBroker` has to
/// be given at least one.
///
/// `Users`' own documentation: "The list of broker users (persons or
/// applications) who can access queues and topics. For Amazon MQ for RabbitMQ
/// brokers, an administrative user is required if using simple authentication
/// and authorization. For brokers using OAuth2, this user is optional."
/// (`smithy_query(query="doc", shape="CreateBrokerRequest", member="Users")`).
/// The OAuth2 exemption is not reachable through this model — its
/// `AuthenticationStrategy` carries only CONFIG_MANAGED, LDAP and SIMPLE — so
/// every broker this service can be asked for needs its first user named in the
/// create. The model marks the member non-required, which is why the
/// precondition lives in the handler.
pub const MIN_USERS_PER_BROKER: usize = 1;

/// `Groups`' own documented bound: "The list of groups (20 maximum) to which the
/// ActiveMQ user belongs"
/// (`smithy_query(query="doc", shape="CreateUserRequest", member="Groups")`).
pub const MAX_GROUPS_PER_USER: usize = 20;

/// The `EngineType` value whose brokers the model excludes from some members —
/// `Logs$audit` "does not apply to RabbitMQ brokers". From `ENGINE_TYPE_VALUES`.
pub const ENGINE_TYPE_RABBITMQ: &str = "RABBITMQ";

/// The other `EngineType` the service supports ("Amazon MQ supports ActiveMQ and
/// RabbitMQ"). From `ENGINE_TYPE_VALUES`.
pub const ENGINE_TYPE_ACTIVEMQ: &str = "ACTIVEMQ";

/// `BrokerStorageType`'s block-storage value. From `BROKER_STORAGE_TYPE_VALUES`.
pub const STORAGE_TYPE_EBS: &str = "EBS";

/// `BrokerStorageType`'s shared-file-system value, the one the model documents as
/// "not supported for RabbitMQ engine type". From `BROKER_STORAGE_TYPE_VALUES`.
pub const STORAGE_TYPE_EFS: &str = "EFS";

/// The `DeploymentMode` of a broker that runs as one instance. From
/// `DEPLOYMENT_MODE_VALUES`.
pub const DEPLOYMENT_MODE_SINGLE_INSTANCE: &str = "SINGLE_INSTANCE";

/// The `DeploymentMode` of an ActiveMQ broker that runs as a highly available pair
/// across two Availability Zones. From `DEPLOYMENT_MODE_VALUES`.
pub const DEPLOYMENT_MODE_ACTIVE_STANDBY_MULTI_AZ: &str = "ACTIVE_STANDBY_MULTI_AZ";

/// The `DeploymentMode` of a RabbitMQ broker that runs as a cluster across
/// Availability Zones. From `DEPLOYMENT_MODE_VALUES`.
pub const DEPLOYMENT_MODE_CLUSTER_MULTI_AZ: &str = "CLUSTER_MULTI_AZ";

/// `SecurityGroups`' own documented bound: "The list of rules (1 minimum, 125
/// maximum) that authorize connections to brokers".
pub const MAX_SECURITY_GROUPS: usize = 125;

/// The model publishes no maximum for `SubnetIds`, and the per-deployment-mode
/// rules it documents fix the count only for `SINGLE_INSTANCE` (one) and
/// `ACTIVE_STANDBY_MULTI_AZ` (two). So that a `CLUSTER_MULTI_AZ` create cannot
/// grow one record without bound, the emulator applies a RESOURCE bound of its
/// own — not an AWS one, and deliberately far above the number of Availability
/// Zones any Region has, so it bounds the record without refusing a cluster a
/// real deployment could ask for.
pub const MAX_SUBNET_IDS: usize = 1_000;

/// AWS's tags-per-resource limit (50), identical for every taggable resource,
/// and the bound on one taggable record's `tags` map.
pub const MAX_TAGS: usize = 50;

/// Amazon MQ's documented per-Region, per-account configuration quota (Amazon MQ
/// Developer Guide, "Quotas in Amazon MQ": 50 configurations per Region), and —
/// like [`MAX_BROKERS`] — the bound that keeps the store from growing with the
/// number of creates a caller makes.
pub const MAX_CONFIGURATIONS: usize = 50;

/// How many revisions one configuration record may hold.
///
/// Amazon MQ publishes no maximum for a configuration's revision chain, and each
/// `UpdateConfiguration` appends one revision carrying the document it was sent,
/// so that an update loop cannot grow one record without bound the emulator
/// applies a RESOURCE bound of its own — not an AWS one, and deliberately far
/// above the number of revisions a real configuration workflow makes, so it
/// bounds the record without refusing a revision a real rollout would ask for.
/// An update past it is refused rather than stored.
pub const MAX_CONFIGURATION_REVISIONS: usize = 1_000;

/// How many retired `ConfigurationId`s one store remembers — the ids that were
/// issued and have since been deleted, which a write is refused as a conflict
/// with ([`ComAmazonawsMqMqState::retire_configuration`]).
///
/// A retired id costs only the id itself, but one is added per delete, so the
/// emulator applies a RESOURCE bound of its own — not an AWS one — and remembers
/// the most recently retired ids, dropping the oldest once the bound is reached.
/// It is deliberately far above the number of configurations an account may hold
/// at once ([`MAX_CONFIGURATIONS`]), so a workflow that recycles its
/// configurations still finds every id it deleted remembered.
pub const MAX_RETIRED_CONFIGURATIONS: usize = 1_000;

/// The engine versions Amazon MQ offers for ActiveMQ, oldest first, so the last
/// entry is the latest available one.
///
/// Amazon MQ publishes the versions it offers for creation at MAJOR.MINOR
/// granularity and retires the older ones as it releases new ones: the recorded
/// `DescribeBrokerInstanceOptions` catalog carries `["5.19", "5.18"]` for
/// ActiveMQ. A patch-level string from a retired release is not a version a
/// create may be defaulted to, which is why the list holds the offered ones
/// only. `DescribeBrokerInstanceOptions` reports the same list, and
/// `CreateConfiguration` defaults `EngineVersion` to its last entry.
pub const ACTIVEMQ_ENGINE_VERSIONS: &[&str] = &["5.18", "5.19"];

/// The engine versions Amazon MQ offers for RabbitMQ, oldest first — see
/// [`ACTIVEMQ_ENGINE_VERSIONS`] for the granularity and where it comes from (the
/// same recorded catalog carries `["3.13"]` for RabbitMQ).
pub const RABBITMQ_ENGINE_VERSIONS: &[&str] = &["3.13"];

/// The engine versions published for `engine_type`, or none for a value outside
/// the model's `EngineType` set — a handler refuses such a value before it gets
/// here, and attributing one engine's versions to another would be worse than
/// reporting none.
pub fn published_engine_versions(engine_type: &str) -> &'static [&'static str] {
    match engine_type {
        ENGINE_TYPE_ACTIVEMQ => ACTIVEMQ_ENGINE_VERSIONS,
        ENGINE_TYPE_RABBITMQ => RABBITMQ_ENGINE_VERSIONS,
        _ => &[],
    }
}

/// The latest published engine version of `engine_type` — what a create that
/// names no `EngineVersion` gets: "Defaults to the latest available version for
/// the specified broker engine type".
pub fn latest_engine_version(engine_type: &str) -> &'static str {
    published_engine_versions(engine_type)
        .last()
        .copied()
        .unwrap_or_default()
}

/// The shortest `Name` a configuration may carry — from the member's own
/// documentation: "This value must be 1-150 characters long"
/// (`smithy_query(query="doc", shape="CreateConfigurationRequest", member="Name")`).
pub const MIN_CONFIGURATION_NAME_LEN: usize = 1;

/// The longest `Name` a configuration may carry — see
/// [`MIN_CONFIGURATION_NAME_LEN`] for the sentence both bounds come from.
pub const MAX_CONFIGURATION_NAME_LEN: usize = 150;

/// A broker user. The password is held so the record is complete, and is never
/// put in a response: no modelled output member carries it.
#[derive(Debug, Clone)]
pub struct BrokerUser {
    pub username: String,
    pub password: String,
    pub console_access: bool,
    pub groups: Vec<String>,
    pub replication_user: bool,
    /// The change this user is waiting for the broker to apply, as
    /// `UserPendingChanges$PendingChange` reports it ("Required. The type of
    /// change pending for the ActiveMQ user"), or `None` when nothing is
    /// pending. A user change does not take effect on the broker until the
    /// broker is rebooted, so the user keeps serving and keeps being reported
    /// while its change is pending; the reboot is what applies it.
    pub pending_change: Option<String>,
}

/// The `ChangeType` of a user the broker is to drop once the change is applied.
/// From `ChangeType`'s modelled values (`CREATE`, `UPDATE`, `DELETE`).
pub const CHANGE_TYPE_DELETE: &str = "DELETE";

/// The `ChangeType` of a user the broker is to ADD once the change is applied.
/// From the same modelled values.
pub const CHANGE_TYPE_CREATE: &str = "CREATE";

/// The `ChangeType` of a user whose members the broker is to change once the change
/// is applied. From the same modelled values.
pub const CHANGE_TYPE_UPDATE: &str = "UPDATE";

/// The kind of logs `LogsSummary$auditLogGroup` names the log group of — the last
/// segment of the group's name ([`Broker::log_group`]).
pub const LOG_KIND_AUDIT: &str = "audit";

/// The kind of logs `LogsSummary$generalLogGroup` names the log group of.
pub const LOG_KIND_GENERAL: &str = "general";

/// The broker logging `CreateBroker` was asked for. `Logs$general` is a plain
/// flag, and `Logs$audit` is kept optional because the member "does not apply to
/// RabbitMQ brokers", so a request that never set it must not report one.
#[derive(Debug, Clone, Default)]
pub struct BrokerLogs {
    pub audit: Option<bool>,
    pub general: bool,
}

/// The broker's encryption options, as the request set them.
#[derive(Debug, Clone)]
pub struct BrokerEncryptionOptions {
    pub kms_key_id: Option<String>,
    pub use_aws_owned_key: bool,
}

/// The broker's maintenance window. `time_zone` is stored as sent; the model
/// documents "UTC by default", which [`DEFAULT_MAINTENANCE_TIME_ZONE`] supplies
/// when the request omitted it.
#[derive(Debug, Clone)]
pub struct BrokerMaintenanceWindow {
    pub day_of_week: String,
    pub time_of_day: String,
    pub time_zone: String,
}

/// The broker's LDAP server metadata. `service_account_password` is held so the
/// record is complete and is never put in a response: `LdapServerMetadataOutput`
/// carries no password member.
#[derive(Debug, Clone)]
pub struct BrokerLdapServerMetadata {
    pub hosts: Vec<String>,
    pub role_base: String,
    pub role_name: Option<String>,
    pub role_search_matching: String,
    pub role_search_subtree: Option<bool>,
    pub service_account_username: String,
    pub service_account_password: String,
    pub user_base: String,
    pub user_role_name: Option<String>,
    pub user_search_matching: String,
    pub user_search_subtree: Option<bool>,
}

/// The time zone a maintenance window reports when the request set none, from
/// `WeeklyStartTime$timeZone`: "The time zone, UTC by default".
pub const DEFAULT_MAINTENANCE_TIME_ZONE: &str = "UTC";

/// A broker, as `CreateBroker` persisted it.
#[derive(Debug, Clone)]
pub struct Broker {
    pub broker_id: String,
    pub broker_arn: String,
    pub broker_name: String,
    pub engine_type: String,
    /// The engine release this broker RUNS — the patch release the create's version
    /// line resolved to ([`resolved_engine_version`]), not the line it named.
    pub engine_version: String,
    pub deployment_mode: String,
    pub host_instance_type: String,
    pub publicly_accessible: bool,
    pub auto_minor_version_upgrade: bool,
    pub authentication_strategy: String,
    /// The storage this broker was given: the `BrokerStorageType` the create named, or
    /// the engine's default ([`default_storage_type`]).
    pub storage_type: String,
    pub storage_size: Option<i32>,
    pub security_groups: Vec<String>,
    pub subnet_ids: Vec<String>,
    pub data_replication_mode: Option<String>,
    pub data_replication_primary_broker_arn: Option<String>,
    pub created: SystemTime,
    /// The instant the clock owes this broker its move to `RUNNING`.
    pub available_at: SystemTime,
    pub creator_request_id: Option<String>,
    /// The members of the `CreateBroker` request that decide WHICH broker was
    /// asked for, so a replay carrying the same `@idempotencyToken` can be told
    /// apart from a different request reusing that token.
    pub creation_fingerprint: String,
    pub users: BTreeMap<String, BrokerUser>,
    pub tags: BTreeMap<String, String>,
    pub logs: BrokerLogs,
    /// How this broker's data is encrypted: the options the create named, or the
    /// Amazon Web Services owned key ([`BrokerEncryptionOptions::aws_owned_key`]).
    pub encryption_options: BrokerEncryptionOptions,
    /// The week's maintenance window this broker is on: the one the create named, or
    /// the one it was assigned ([`assigned_maintenance_window`]).
    pub maintenance_window_start_time: BrokerMaintenanceWindow,
    pub ldap_server_metadata: Option<BrokerLdapServerMetadata>,
    /// The configuration revision this broker runs: the one its create named, or
    /// the configuration the create minted for it
    /// ([`ComAmazonawsMqMqState::mint_broker_default_configuration`]).
    pub configuration: BrokerConfigurationRef,
    /// The instant the clock owes this broker its return to `RUNNING` from a restart,
    /// or `None` when no restart is in flight ([`Broker::restart`]).
    pub restart_completes_at: Option<SystemTime>,
    /// The role a PROMOTION gave this broker, when one has. Unset otherwise: the role a
    /// broker of a replication pair holds before any promotion is derived from the pair
    /// itself ([`Broker::data_replication_role`]).
    pub data_replication_role: Option<String>,
    /// The host this emulator answered the create on, which is the address this
    /// broker is published at ([`Broker::allocated_instances`]). Recorded at create
    /// time so every read of the broker publishes the one address, whichever host a
    /// later caller reached the emulator on.
    pub published_host: String,
}

impl Broker {
    /// This broker's `BrokerState` as of `now`: derived from the stored instants,
    /// so a reporting operation computes it without writing anything.
    pub fn broker_state(&self, now: SystemTime) -> &'static str {
        if now < self.available_at {
            BROKER_STATE_CREATION_IN_PROGRESS
        } else if self.restarting(now) {
            BROKER_STATE_REBOOT_IN_PROGRESS
        } else {
            BROKER_STATE_RUNNING
        }
    }

    /// `Created`, in the modelled `__timestampIso8601` type.
    pub fn created_timestamp(&self) -> ::aws_smithy_types::DateTime {
        ::aws_smithy_types::DateTime::from(self.created)
    }

    /// The CloudWatch Logs log group a broker's logs of `kind` are sent to.
    /// Amazon MQ names it after the broker, so it is DERIVED from the stored id
    /// rather than held: the same broker always reports the same group, and a
    /// broker that was never created has none.
    pub fn log_group(&self, kind: &str) -> String {
        format!("/aws/amazonmq/broker/{}/{kind}", self.broker_id)
    }

    /// Whether AUDIT logging applies to this broker at all: `Logs$audit` "does not
    /// apply to RabbitMQ brokers", so that engine's brokers report neither the flag
    /// nor its log group, and every other broker reports both — the flag being `false`
    /// when audit logging is off, which is a value a client reads, not an absence.
    pub fn audit_logging_applies(&self) -> bool {
        self.engine_type != ENGINE_TYPE_RABBITMQ
    }

    /// `Tags`, as the modelled map, or `None` when the broker carries none.
    pub fn optional_tags(&self) -> Option<std::collections::HashMap<String, String>> {
        optional_tag_map(&self.tags)
    }
}

/// A stored tag map as the modelled `__mapOf__string`, or `None` when the
/// resource carries no tag — restJson1 omits a `None` member rather than
/// emitting an empty object for tags nobody set.
///
/// A free function because every operation that reports a resource's tags goes
/// through it, whichever record holds them.
pub fn optional_tag_map(
    tags: &BTreeMap<String, String>,
) -> Option<std::collections::HashMap<String, String>> {
    if tags.is_empty() {
        None
    } else {
        Some(tag_map(tags))
    }
}

/// A stored tag map as the modelled `__mapOf__string`, whether or not it holds
/// anything — the form a resource whose answer always carries the member
/// reports (see [`ConfigurationRecord::reported_tags`]).
pub fn tag_map(tags: &BTreeMap<String, String>) -> std::collections::HashMap<String, String> {
    tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// The product spelling Amazon MQ publishes for each engine the model's
/// `EngineType` declares, paired with the modelled value a request sends.
///
/// Source: the modelled values (`ENGINE_TYPE_VALUES`) and the engine names
/// Amazon MQ's own documentation spells out for them ("Amazon MQ for ActiveMQ",
/// "Amazon MQ for RabbitMQ"), which is the spelling the recorded configuration
/// responses carry.
const ENGINE_TYPE_PRODUCT_SPELLINGS: &[(&str, &str)] = &[
    (ENGINE_TYPE_ACTIVEMQ, "ActiveMQ"),
    (ENGINE_TYPE_RABBITMQ, "RabbitMQ"),
];

/// How an `EngineType` is spelled on the wire.
///
/// RULE: Amazon MQ ACCEPTS the model's enum token on input (`ACTIVEMQ`) and ANSWERS
/// with the engine's product spelling (`ActiveMQ`) — the spelling the recorded broker
/// and configuration answers both carry, and the one a client comparing the member
/// against a published engine name matches. The stored record keeps the modelled value
/// a request has to send, so this is the one place the reported spelling is derived and
/// the two cannot drift; a value outside the modelled set is reported exactly as stored
/// rather than rewritten into a spelling nothing published.
pub fn reported_engine_type(engine_type: &str) -> String {
    ENGINE_TYPE_PRODUCT_SPELLINGS
        .iter()
        .find(|(modelled, _)| *modelled == engine_type)
        .map_or_else(|| engine_type.to_string(), |(_, spelled)| (*spelled).to_string())
}

/// How an `AuthenticationStrategy` is spelled on the wire: the modelled value
/// lower-cased (`SIMPLE` -> `simple`), which is what the recorded Amazon MQ answers for
/// a broker and for a configuration both carry. As with [`reported_engine_type`], the
/// stored record keeps the modelled value.
pub fn reported_authentication_strategy(strategy: &str) -> String {
    strategy.to_ascii_lowercase()
}

/// How a `BrokerStorageType` is spelled on the wire: the modelled value lower-cased
/// (`EFS` -> `efs`), which is what the recorded Amazon MQ broker answer carries, and
/// the spelling the instance-option catalog already reports. As with
/// [`reported_engine_type`], the stored record keeps the modelled value.
pub fn reported_storage_type(storage_type: &str) -> String {
    storage_type.to_ascii_lowercase()
}

/// One revision of a configuration: the revision number, when it was made, the
/// description it was made with and the configuration document it carries. A
/// configuration holds its revisions oldest first, so the last one is its
/// latest.
#[derive(Debug, Clone)]
pub struct ConfigurationRevisionRecord {
    pub revision: i32,
    pub created: SystemTime,
    /// The description the revision was made with: the sentence Amazon MQ
    /// generates for the revision a create makes, or the `Description` an update
    /// supplied — the EMPTY description for an update that supplied none, which
    /// is the one representation every read of this record reports (the modelled
    /// `Configuration$Description` a listing carries is required, so an absent
    /// member is not a form every read could agree on).
    pub description: String,
    /// The configuration document the revision carries (base64), in the form this
    /// service STORES it: the canonical serialization of the document the update sent
    /// ([`stored_configuration_data`]), not the bytes it sent. `None` for the revision
    /// a create makes: that one holds the engine's own default settings, which no
    /// request supplied.
    pub data: Option<String>,
    /// Whether the engine judged this revision's document unusable, reported as
    /// `ConfigurationRevision$isInvalid`. A revision is only ever made from a
    /// document this service ACCEPTED — a malformed one is refused before any
    /// revision exists ([`ConfigurationRevisionRecord::accepted`]) — so a stored
    /// revision is a usable one.
    pub is_invalid: bool,
}

impl ConfigurationRevisionRecord {
    /// A revision made from a configuration document this service accepted: the
    /// revision is usable, so it reports no invalidity. Both producers of a
    /// revision — the create that generates the first one and the update that
    /// makes the next — go through here, so neither can record a revision whose
    /// document was never checked.
    pub fn accepted(
        revision: i32,
        created: SystemTime,
        description: String,
        data: Option<String>,
    ) -> Self {
        Self {
            revision,
            created,
            description,
            data,
            is_invalid: false,
        }
    }
}

/// A configuration, as `CreateConfiguration` persisted it. Named `*Record` so it
/// cannot be confused with the modelled `Configuration` shape a handler
/// serializes it into.
#[derive(Debug, Clone)]
pub struct ConfigurationRecord {
    pub configuration_id: String,
    pub configuration_arn: String,
    pub name: String,
    pub engine_type: String,
    pub engine_version: String,
    pub authentication_strategy: String,
    pub created: SystemTime,
    /// Every revision, oldest first; a created configuration starts with one.
    pub revisions: Vec<ConfigurationRevisionRecord>,
    pub tags: BTreeMap<String, String>,
    /// The broker this configuration was created FOR, when it is the one a
    /// `CreateBroker` minted for a broker that named none
    /// ([`ComAmazonawsMqMqState::mint_broker_default_configuration`]); `None` for a
    /// configuration a `CreateConfiguration` created.
    pub created_for_broker: Option<String>,
}

/// The revision number a configuration's first revision carries: Amazon MQ
/// numbers revisions from one, and an update makes the next one.
pub const FIRST_CONFIGURATION_REVISION: i32 = 1;

impl ConfigurationRecord {
    /// This configuration's latest revision — the last one made. A configuration
    /// always has one, so a record that somehow holds none reports its first.
    pub fn latest_revision(&self) -> ConfigurationRevisionRecord {
        self.revisions.last().cloned().unwrap_or_else(|| {
            ConfigurationRevisionRecord::accepted(
                FIRST_CONFIGURATION_REVISION,
                self.created,
                default_configuration_description(
                    &self.name,
                    &self.engine_type,
                    &self.engine_version,
                ),
                None,
            )
        })
    }

    /// The number the NEXT revision of this configuration carries: Amazon MQ
    /// numbers revisions from one and an update makes the next one, so the
    /// number is derived from the chain the record already holds rather than
    /// from a counter beside it that could disagree with it.
    pub fn next_revision_number(&self) -> i32 {
        self.latest_revision().revision.saturating_add(1)
    }

    /// `Created`, in the modelled `__timestampIso8601` type.
    pub fn created_timestamp(&self) -> ::aws_smithy_types::DateTime {
        ::aws_smithy_types::DateTime::from(self.created)
    }

    /// `Tags`, as the modelled map — EMPTY when the configuration carries no
    /// tag, rather than absent: the recorded Amazon MQ answer for an untagged
    /// configuration carries `tags: {}`, so a client iterating the member finds
    /// a map on every configuration it reads.
    pub fn reported_tags(&self) -> Option<std::collections::HashMap<String, String>> {
        Some(tag_map(&self.tags))
    }
}

/// The description Amazon MQ gives the revision it generates for a configuration
/// nobody described: the configuration is created from the engine's default
/// settings (`CreateConfiguration`: "Amazon MQ uses the default configuration
/// (the engine type and version)"), so the sentence is built from the stored
/// record — never from a request member this operation does not carry.
///
/// The engine is named in the spelling every answer reports it in
/// ([`reported_engine_type`]), so the sentence and the `engineType` member of the same
/// answer agree.
pub fn default_configuration_description(
    name: &str,
    engine_type: &str,
    engine_version: &str,
) -> String {
    format!(
        "Auto-generated default for {name} on {} {engine_version}",
        reported_engine_type(engine_type)
    )
}

/// Per-service state for com-amazonaws-mq-mq. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMqMqState {
    /// Every broker that exists, keyed by its minted `BrokerId`.
    pub brokers: BTreeMap<String, Broker>,
    /// The strictly increasing sequence the next minted `BrokerId` is derived
    /// from, so two creates in the same instant cannot mint one id.
    pub next_broker_sequence: u64,
    /// Every configuration that exists, keyed by its minted `ConfigurationId`.
    pub configurations: BTreeMap<String, ConfigurationRecord>,
    /// The mint sequence behind the next `ConfigurationId` — see
    /// [`Self::next_broker_sequence`].
    pub next_configuration_sequence: u64,
    /// Every `ConfigurationId` this service issued and has since deleted, oldest
    /// retirement first, bounded by [`MAX_RETIRED_CONFIGURATIONS`].
    ///
    /// A deleted configuration is gone as a RESOURCE — nothing reads back
    /// through it — but the id it was issued under is not forgotten, so a write
    /// against it can be told apart from a write against an id this service
    /// never issued. The ids are held here rather than as deleted records in
    /// [`Self::configurations`] so that every operation reading that map still
    /// reports exactly the configurations that exist.
    pub retired_configuration_ids: VecDeque<String>,
}

impl ComAmazonawsMqMqState {
    /// The broker `broker_id` names, with everything the clock already owes it applied
    /// ([`Broker::apply_due_changes`]) — `None` when no broker carries that id.
    ///
    /// Every operation that resolves a broker goes through this, so a change a restart
    /// was to apply is in force on the first read taken after that restart came due,
    /// whichever operation takes it. The only write it makes is the one the clock had
    /// already made due: the same answer a caller would have been given had nobody
    /// called at all.
    pub fn settled_broker(&mut self, broker_id: &str, now: SystemTime) -> Option<&Broker> {
        self.settled_broker_mut(broker_id, now).map(|broker| &*broker)
    }

    /// [`Self::settled_broker`], for an operation that then changes the broker.
    pub fn settled_broker_mut(
        &mut self,
        broker_id: &str,
        now: SystemTime,
    ) -> Option<&mut Broker> {
        let broker = self.brokers.get_mut(broker_id)?;
        broker.apply_due_changes(now);
        Some(broker)
    }

    /// The broker carrying `name`. Broker names are unique per account, so this
    /// scan over the one map is the name index — a second map could desync from it.
    pub fn broker_by_name(&self, name: &str) -> Option<&Broker> {
        self.brokers.values().find(|b| b.broker_name == name)
    }

    /// The broker an earlier `CreateBroker` with this `@idempotencyToken` created.
    pub fn broker_by_creator_request_id(&self, token: &str) -> Option<&Broker> {
        self.brokers
            .values()
            .find(|b| b.creator_request_id.as_deref() == Some(token))
    }

    /// Mint the `BrokerId` for a broker created at `created`: `b-` followed by
    /// 128 bits in the 8-4-4-4-12 grouping Amazon MQ's broker ids carry.
    ///
    /// The bits are a digest of this store's own mint sequence, the creating
    /// instant and the broker's name, so the id is unique within the store
    /// (the sequence alone guarantees that, and a collision with a stored id
    /// takes the next sequence value), is not guessable from the request alone,
    /// and is MINTED here rather than taken from any caller-supplied value.
    pub fn mint_broker_id(&mut self, created: SystemTime, broker_name: &str) -> String {
        loop {
            let sequence = self.next_broker_sequence;
            self.next_broker_sequence = sequence.wrapping_add(1);
            let candidate = mint_resource_id("b", sequence, created, broker_name);
            if !self.brokers.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The configuration a CALLER created under `name`. Configuration names are
    /// unique per account, so this scan over the one map is the name index — a second
    /// map could desync from it.
    ///
    /// A configuration created FOR a broker carries that broker's name by
    /// construction, and broker names are their own namespace, so it is not part of
    /// the names a `CreateConfiguration` competes for: a caller can still name a
    /// configuration after a broker it created.
    pub fn configuration_by_name(&self, name: &str) -> Option<&ConfigurationRecord> {
        self.configurations
            .values()
            .find(|c| c.name == name && c.created_for_broker.is_none())
    }

    /// Mint the `ConfigurationId` of a configuration created at `created`: `c-`
    /// and the same minted bits a `BrokerId` carries — see
    /// [`Self::mint_broker_id`] for where they come from.
    pub fn mint_configuration_id(&mut self, created: SystemTime, name: &str) -> String {
        loop {
            let sequence = self.next_configuration_sequence;
            self.next_configuration_sequence = sequence.wrapping_add(1);
            let candidate = mint_resource_id("c", sequence, created, name);
            // An id this store ever issued is never issued twice, so a retired
            // one is as taken as a live one.
            if !self.configurations.contains_key(&candidate)
                && !self.is_configuration_retired(&candidate)
            {
                return candidate;
            }
        }
    }

    /// Whether `configuration_id` is an id this service issued and has since
    /// deleted.
    pub fn is_configuration_retired(&self, configuration_id: &str) -> bool {
        self.retired_configuration_ids
            .iter()
            .any(|retired| retired == configuration_id)
    }

    /// Delete the configuration `configuration_id` names and retire the id,
    /// reporting the record that was removed, or `None` when no configuration
    /// is held under that id.
    ///
    /// Removing the record is what makes the configuration gone: every operation
    /// that reads [`Self::configurations`] resolves that id to nothing
    /// afterwards. Retiring the id is what keeps the store from forgetting that
    /// it issued it, so a later write against it is answered as the conflict it
    /// is rather than as an unknown resource. The oldest retirement is dropped
    /// once [`MAX_RETIRED_CONFIGURATIONS`] are remembered.
    pub fn retire_configuration(&mut self, configuration_id: &str) -> Option<ConfigurationRecord> {
        let record = self.configurations.remove(configuration_id)?;
        if !self.is_configuration_retired(configuration_id) {
            self.retired_configuration_ids
                .push_back(configuration_id.to_string());
            while self.retired_configuration_ids.len() > MAX_RETIRED_CONFIGURATIONS {
                self.retired_configuration_ids.pop_front();
            }
        }
        Some(record)
    }

    /// Every configuration in a stable order — the ordering guarantee
    /// [`Self::brokers_in_order`] documents, over the configuration map.
    pub fn configurations_in_order(&self) -> Vec<&ConfigurationRecord> {
        let mut all: Vec<&ConfigurationRecord> = self.configurations.values().collect();
        all.sort_by(|a, b| {
            a.created
                .cmp(&b.created)
                .then_with(|| a.configuration_id.cmp(&b.configuration_id))
        });
        all
    }

    /// The tag map of the resource `resource_arn` names, or `None` when this
    /// service holds no resource with that ARN.
    ///
    /// Every tag operation resolves its `ResourceArn` through this one lookup (or
    /// through [`Self::tags_of_mut`]), so reading, adding and removing tags all
    /// agree on which ARNs name something. The ARN is matched against the one a
    /// resource was minted with, so an ARN naming another account, Region,
    /// resource type or id resolves to nothing. A record type that becomes
    /// taggable later is resolved here too rather than in a handler.
    pub fn tags_of(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(broker) = self
            .brokers
            .values()
            .find(|broker| broker.broker_arn == resource_arn)
        {
            return Some(&broker.tags);
        }
        self.configurations
            .values()
            .find(|configuration| configuration.configuration_arn == resource_arn)
            .map(|configuration| &configuration.tags)
    }

    /// The tag map of the resource `resource_arn` names, for an operation that
    /// changes it. See [`Self::tags_of`] for how an ARN is resolved.
    pub fn tags_of_mut(&mut self, resource_arn: &str) -> Option<&mut BTreeMap<String, String>> {
        if let Some(broker) = self
            .brokers
            .values_mut()
            .find(|broker| broker.broker_arn == resource_arn)
        {
            return Some(&mut broker.tags);
        }
        self.configurations
            .values_mut()
            .find(|configuration| configuration.configuration_arn == resource_arn)
            .map(|configuration| &mut configuration.tags)
    }

    /// Every broker in a stable order: by creation instant, then by id, so two
    /// listings of an unchanged set page through it identically. A broker
    /// created or deleted between two pages shifts the offsets a `NextToken`
    /// names, which is why the ordering is a stability guarantee over an
    /// unchanged set and not a snapshot across a paginating client's calls.
    pub fn brokers_in_order(&self) -> Vec<&Broker> {
        let mut all: Vec<&Broker> = self.brokers.values().collect();
        all.sort_by(|a, b| {
            a.created
                .cmp(&b.created)
                .then_with(|| a.broker_id.cmp(&b.broker_id))
        });
        all
    }
}

/// Mint the id of a resource created at `created`: `prefix-` followed by 128
/// bits in the 8-4-4-4-12 grouping Amazon MQ's ids carry.
///
/// The bits are a digest of the store's own mint sequence, the creating instant
/// and the resource's name, so the id is unique within the store (the sequence
/// alone guarantees that), is not guessable from the request alone, and is
/// MINTED here rather than taken from any caller-supplied value. Both resource
/// kinds mint through this one function, so their ids carry the same shape.
fn mint_resource_id(prefix: &str, sequence: u64, created: SystemTime, name: &str) -> String {
    let nanos = created
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut digest = <::sha2::Sha256 as ::sha2::Digest>::new();
    ::sha2::Digest::update(&mut digest, sequence.to_be_bytes());
    ::sha2::Digest::update(&mut digest, nanos.to_be_bytes());
    ::sha2::Digest::update(&mut digest, name.as_bytes());
    let bytes = ::sha2::Digest::finalize(digest);
    let hex: String = bytes[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{prefix}-{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A list member, or `None` when nothing was stored — restJson1 omits a `None`
/// member rather than emitting an empty array for a value the request never set.
///
/// A free function because both a broker's own lists and a user's `Groups` go
/// through it; it has nothing to do with a particular record.
pub fn optional_list(values: &[String]) -> Option<Vec<String>> {
    if values.is_empty() {
        None
    } else {
        Some(values.to_vec())
    }
}

/// A request this service refuses: the member at fault, and why.
///
/// Each operation turns it into its OWN modelled `BadRequestException` (every
/// operation declares one, with its own generated shape), which is why this
/// carries the parts rather than a built error.
#[derive(Debug, Clone)]
pub struct Invalid {
    /// The member at fault, in its wire (`jsonName`) spelling, for
    /// `BadRequestException$errorAttribute`; `None` when no member of the
    /// request is at fault (a quota, say).
    pub attribute: Option<String>,
    /// What was wrong, naming the member and the constraint it broke.
    pub message: String,
}

impl Invalid {
    pub fn new(attribute: &str, message: impl Into<String>) -> Self {
        Self {
            attribute: Some(attribute.to_string()),
            message: message.into(),
        }
    }
}

/// The idempotency fingerprint of a `CreateBroker` request: every member that
/// defines the broker being asked for, in a canonical form. It is compared,
/// never parsed, so its spelling only has to be stable and collision-free.
///
/// `parts` are joined with the unit separator, which no member's documented
/// character set contains, so two different requests cannot fold to one string.
/// A member the request omitted contributes an empty part, so omitting a member
/// on a replay is itself a mismatch.
pub fn fingerprint_of(parts: &[String]) -> String {
    parts.join("\u{1f}")
}

/// The subject Amazon MQ names when it refuses a value outside an enumerated
/// member's value set.
///
/// MQ states such a refusal as one sentence built from the request and the
/// model: the member's documented subject, the rejected value in brackets, then
/// the values it does accept — for `engineType`, "Broker engine type
/// [<value>] is invalid. Valid values are: [ACTIVEMQ, RABBITMQ]". The subject is
/// the member's own documented description, so it is held per member here rather
/// than derived from the wire name; a member with no row falls back to its wire
/// name, which keeps the sentence well-formed for any member added later.
fn enum_subject(attribute: &str) -> &str {
    match attribute {
        // "Required. The type of broker engine." (`CreateBrokerRequest$EngineType`)
        "engineType" => "Broker engine type",
        // "Required. The broker's deployment mode." (`CreateBrokerRequest$DeploymentMode`)
        "deploymentMode" => "Broker deployment mode",
        // "The broker's storage type." (`CreateBrokerRequest$StorageType`)
        "storageType" => "Broker storage type",
        // "Optional. The authentication strategy used to secure the broker."
        // (`CreateBrokerRequest$AuthenticationStrategy`)
        "authenticationStrategy" => "Broker authentication strategy",
        // "Defines whether this broker is a part of a data replication pair."
        // (`CreateBrokerRequest$DataReplicationMode`)
        "dataReplicationMode" => "Broker data replication mode",
        other => other,
    }
}

/// Refuse a value the model declares as an enum but the request spelled
/// otherwise. `allowed` is the generated `*_VALUES` constant for that enum.
///
/// This is the ONLY check an enum-typed member of this service goes through: the
/// input schemas skip the framework's boundary check for them
/// (`skip_validation`, see `ops::types`), because a boundary refusal is worded
/// through `Refused { code, message }` and so cannot carry
/// `BadRequestException$errorAttribute`.
pub fn validate_enum(attribute: &str, value: &str, allowed: &[&str]) -> Result<(), Invalid> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(Invalid::new(
        attribute,
        format!(
            "{} [{value}] is invalid. Valid values are: [{}]",
            enum_subject(attribute),
            allowed.join(", ")
        ),
    ))
}

/// Refuse an integer outside the `@range` its member declares. As with
/// [`validate_enum`], the handler owns this refusal so that it can name the
/// member in `BadRequestException$errorAttribute`.
pub fn validate_range(attribute: &str, value: i32, min: i32, max: i32) -> Result<(), Invalid> {
    if (min..=max).contains(&value) {
        return Ok(());
    }
    Err(Invalid::new(
        attribute,
        format!("{attribute} must be between {min} and {max}, got {value}"),
    ))
}

/// Refuse a `CreateBroker` that names no broker user, and one that names more
/// than a broker record may hold. `count` is the number of users the request
/// carried — an absent list counts as none, exactly like an empty one.
/// See [`MIN_USERS_PER_BROKER`] for the rule and where it comes from.
///
/// The missing-user sentence is Amazon MQ's own, and names the input rather than
/// any value from the request, so it reads the same for every request in the
/// class.
pub fn validate_users_requested(count: usize) -> Result<(), Invalid> {
    const ATTRIBUTE: &str = "users";
    if count < MIN_USERS_PER_BROKER {
        return Err(Invalid::new(ATTRIBUTE, "Specify at least one user."));
    }
    validate_list_size(
        ATTRIBUTE,
        count,
        MIN_USERS_PER_BROKER,
        MAX_USERS_PER_BROKER,
    )
}

/// Refuse an absent or blank required string member.
pub fn validate_present(attribute: &str, value: &str) -> Result<(), Invalid> {
    if value.trim().is_empty() {
        return Err(Invalid::new(
            attribute,
            format!("{attribute} is required and must not be blank"),
        ));
    }
    Ok(())
}

/// The sentence Amazon MQ answers an operation whose `ResourceArn` names a
/// resource it does not hold with. It is the whole sentence for that failure:
/// nothing from the request is interpolated, so the ARN that resolved to
/// nothing — its Region, account and resource id — is not part of it.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The sentence Amazon MQ answers a `BrokerId` that resolves to no broker
/// with, on every operation that resolves one. Only the requested id varies, so
/// the four handlers that look a broker up share this one sentence rather than
/// each wording the same failure differently.
pub fn broker_not_found_message(broker_id: &str) -> String {
    format!(
        "Cannot find broker ID [{broker_id}]. Verify that your broker exists or check \
         broker permissions."
    )
}

/// The input member a refusal names when the `Username` of the request resolved
/// to no user of the broker it was asked of, for
/// `NotFoundException$errorAttribute` ("The attribute which caused the error").
pub const USERNAME_ATTRIBUTE: &str = "username";

/// The sentence Amazon MQ answers a `Username` that resolves to no user of the
/// broker the request named with, on every operation that resolves one.
///
/// Amazon MQ words a missing USER differently from a missing broker
/// ([`broker_not_found_message`]), so a client can tell the two refusals apart: the
/// fixed text below, with the username and the broker id from the request as its only
/// variable parts. Shared so the handlers that look a user up state the one failure
/// identically.
pub fn user_not_found_message(broker_id: &str, username: &str) -> String {
    format!("Can\u{2019}t find user [{username}] for broker [{broker_id}]. Make sure user exists.")
}

/// The input member a refusal names when the `ConfigurationId` of the request
/// resolved to no configuration, for `NotFoundException$errorAttribute` ("The
/// attribute which caused the error").
pub const CONFIGURATION_ID_ATTRIBUTE: &str = "configuration-id";

/// The sentence Amazon MQ answers a `ConfigurationId` that resolves to no
/// configuration with, on every operation that resolves one — the requested id
/// is interpolated, so the answer names which configuration was not found.
/// Shared for the same reason [`broker_not_found_message`] is.
pub fn configuration_not_found_message(configuration_id: &str) -> String {
    format!("Configuration ID [{configuration_id}] can't be found. Make sure it exists.")
}

/// The sentence Amazon MQ answers a write that would make a new revision of a
/// configuration whose id it issued and has since deleted with — the id it
/// cannot revise is interpolated, and is the only part that varies.
pub fn configuration_revision_conflict_message(configuration_id: &str) -> String {
    format!(
        "Concurrent update to Configuration [{configuration_id}]. Retry to create a new revision."
    )
}

/// The input member a refusal names when the `ConfigurationRevision` of the
/// request resolved to no revision of the configuration it named.
pub const CONFIGURATION_REVISION_ATTRIBUTE: &str = "configuration-revision";

/// The sentence this service answers a `ConfigurationRevision` that resolves to
/// no revision of the configuration with. No recorded response covers this
/// refusal, so it is worded the way this service words the resource-not-found it
/// does have recorded ([`configuration_not_found_message`]), naming the revision
/// asked for and the configuration it was asked of — both from the request.
pub fn configuration_revision_not_found_message(configuration_id: &str, revision: &str) -> String {
    format!(
        "Configuration revision [{revision}] of configuration ID [{configuration_id}] can't be \
         found. Make sure it exists."
    )
}

/// `BrokerName`'s documented constraint: "1-50 characters long, must contain
/// only letters, numbers, dashes, and underscores"
/// (`smithy_query(query="doc", shape="CreateBrokerRequest", member="BrokerName")`).
pub fn validate_broker_name(name: &str) -> Result<(), Invalid> {
    const ATTRIBUTE: &str = "brokerName";
    if name.is_empty() || name.chars().count() > 50 {
        return Err(Invalid::new(
            ATTRIBUTE,
            format!(
                "{ATTRIBUTE} must be 1-50 characters long, got {} characters",
                name.chars().count()
            ),
        ));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '_'))
    {
        return Err(Invalid::new(
            ATTRIBUTE,
            format!(
                "{ATTRIBUTE} must contain only letters, numbers, dashes and underscores, \
                 got the character {bad:?}"
            ),
        ));
    }
    Ok(())
}

/// The character set Amazon MQ documents for a name a caller chooses:
/// "alphanumeric characters, dashes, periods, underscores, and tildes (- . _ ~)"
/// — the same set for a configuration `Name`, a `Username` and a group name, so
/// the three share one predicate and cannot drift apart.
fn is_name_character(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~')
}

/// A configuration `Name`'s constraint, as the model documents it: "This value
/// can contain only alphanumeric characters, dashes, periods, underscores, and
/// tildes (- . _ ~). This value must be 1-150 characters long."
///
/// Amazon MQ states the character-set half with one fixed sentence that
/// interpolates nothing from the request. No recorded response covers the length
/// half, so that one is worded the way this service words every other length
/// refusal (see [`validate_broker_name`]), naming the member and the published
/// bounds.
pub fn validate_configuration_name(name: &str) -> Result<(), Invalid> {
    const ATTRIBUTE: &str = "name";
    let length = name.chars().count();
    if !(MIN_CONFIGURATION_NAME_LEN..=MAX_CONFIGURATION_NAME_LEN).contains(&length) {
        return Err(Invalid::new(
            ATTRIBUTE,
            format!(
                "{ATTRIBUTE} must be {MIN_CONFIGURATION_NAME_LEN}-\
                 {MAX_CONFIGURATION_NAME_LEN} characters long, got {length} characters"
            ),
        ));
    }
    if !name.chars().all(is_name_character) {
        return Err(Invalid::new(
            ATTRIBUTE,
            "Make sure your Configuration name only contains valid characters.",
        ));
    }
    Ok(())
}

/// A user's `Username` constraint, as the model documents it: "can contain only
/// alphanumeric characters, dashes, periods, underscores, and tildes (- . _ ~).
/// This value must be 2-100 characters long."
pub fn validate_username(attribute: &str, username: &str) -> Result<(), Invalid> {
    let length = username.chars().count();
    if !(2..=100).contains(&length) {
        return Err(Invalid::new(
            attribute,
            format!("{attribute} must be 2-100 characters long, got {length} characters"),
        ));
    }
    if let Some(bad) = username.chars().find(|c| !is_name_character(*c)) {
        return Err(Invalid::new(
            attribute,
            format!(
                "{attribute} must contain only alphanumeric characters, dashes, periods, \
                 underscores and tildes (- . _ ~), got the character {bad:?}"
            ),
        ));
    }
    Ok(())
}

/// A user's `Password` constraint, as the model documents it: "must be at least
/// 12 characters long, must contain at least 4 unique characters, and must not
/// contain commas, colons, or equal signs (,:=)."
///
/// A refusal carries Amazon MQ's own sentence for the rule that was broken
/// ([`PASSWORD_LENGTH_MESSAGE`], [`PASSWORD_UNIQUE_CHARACTERS_MESSAGE`],
/// [`PASSWORD_FORBIDDEN_CHARACTERS_MESSAGE`]) rather than a description of the value
/// sent: the sentence states the RULE, so nothing about the rejected secret — not its
/// length, its unique-character count or the character at fault — leaves this function.
pub fn validate_password(attribute: &str, password: &str) -> Result<(), Invalid> {
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        return Err(Invalid::new(attribute, PASSWORD_LENGTH_MESSAGE));
    }
    let unique: std::collections::BTreeSet<char> = password.chars().collect();
    if unique.len() < MIN_UNIQUE_PASSWORD_CHARACTERS {
        return Err(Invalid::new(attribute, PASSWORD_UNIQUE_CHARACTERS_MESSAGE));
    }
    if password.chars().any(|c| matches!(c, ',' | ':' | '=')) {
        return Err(Invalid::new(attribute, PASSWORD_FORBIDDEN_CHARACTERS_MESSAGE));
    }
    Ok(())
}

/// How many characters long a password must be, from `User$Password`'s own
/// documentation ("must be at least 12 characters long") — the bound
/// [`PASSWORD_LENGTH_MESSAGE`] states.
pub const MIN_PASSWORD_LENGTH: usize = 12;

/// How many distinct characters a password must contain, from `User$Password`'s own
/// documentation ("must contain at least 4 unique characters") — the bound
/// [`PASSWORD_UNIQUE_CHARACTERS_MESSAGE`] states.
pub const MIN_UNIQUE_PASSWORD_CHARACTERS: usize = 4;

/// How Amazon MQ states the unique-character rule when it refuses a password. The text
/// is FIXED: the bracketed number is the service's rule, and nothing in the sentence
/// comes from the request — a refused password's own properties (its length, how many
/// unique characters it had) are never quoted back.
pub const PASSWORD_UNIQUE_CHARACTERS_MESSAGE: &str =
    "Password must contain at least [4] unique characters.";

/// How Amazon MQ states the length rule, in the same fixed, bracketed-limit form as
/// [`PASSWORD_UNIQUE_CHARACTERS_MESSAGE`], for the other half of the same documented
/// rule set ("must be at least 12 characters long").
pub const PASSWORD_LENGTH_MESSAGE: &str = "Password must be at least [12] characters long.";

/// How Amazon MQ states the forbidden-character rule, the third rule of that set
/// ("must not contain commas, colons, or equal signs (,:=)"). Fixed text: the character
/// the password carried is not quoted back.
pub const PASSWORD_FORBIDDEN_CHARACTERS_MESSAGE: &str =
    "Password must not contain commas, colons, or equal signs (,:=).";

/// A user's `Groups` constraint, as the model documents it: "The list of groups
/// (20 maximum) to which the ActiveMQ user belongs. This value can contain only
/// alphanumeric characters, dashes, periods, underscores, and tildes (- . _ ~).
/// This value must be 2-100 characters long." — the same value rule as a
/// username, which is why it reuses [`validate_username`].
pub fn validate_groups(attribute: &str, groups: &[String]) -> Result<(), Invalid> {
    validate_list_size(attribute, groups.len(), 0, MAX_GROUPS_PER_USER)?;
    for (position, group) in groups.iter().enumerate() {
        validate_username(attribute, group).map_err(|invalid| Invalid {
            attribute: invalid.attribute,
            message: format!("{attribute}[{position}]: {}", invalid.message),
        })?;
    }
    Ok(())
}

/// Refuse a caller-sized list outside `min..=max` members. Bounding what one
/// record can hold is what keeps a create from growing the store by as much as
/// the request asks for.
pub fn validate_list_size(
    attribute: &str,
    len: usize,
    min: usize,
    max: usize,
) -> Result<(), Invalid> {
    if (min..=max).contains(&len) {
        return Ok(());
    }
    Err(Invalid::new(
        attribute,
        format!("{attribute} must hold between {min} and {max} members, got {len}"),
    ))
}

/// Refuse a `SubnetIds` list whose size the selected deployment mode does not
/// allow. From `SubnetIds`' own documentation: "A SINGLE_INSTANCE deployment
/// requires one subnet (for example, the default subnet). An
/// ACTIVE_STANDBY_MULTI_AZ Amazon MQ for ActiveMQ deployment requires two
/// subnets. A CLUSTER_MULTI_AZ Amazon MQ for RabbitMQ deployment has no subnet
/// requirements when deployed with public accessibility."
///
/// An ABSENT list is not refused: Amazon MQ then places the broker in the
/// account's default subnet, so absence is not a constraint violation. An
/// explicitly EMPTY list is read the same way — "no subnets supplied" — unlike
/// `SecurityGroups`, whose own documentation states a minimum of 1. A list that
/// was sent has to match the mode it was sent with.
pub fn validate_subnet_ids(deployment_mode: &str, subnet_ids: &[String]) -> Result<(), Invalid> {
    const ATTRIBUTE: &str = "subnetIds";
    if subnet_ids.is_empty() {
        return Ok(());
    }
    validate_list_size(ATTRIBUTE, subnet_ids.len(), 1, MAX_SUBNET_IDS)?;
    let required = match deployment_mode {
        DEPLOYMENT_MODE_SINGLE_INSTANCE => Some(1),
        DEPLOYMENT_MODE_ACTIVE_STANDBY_MULTI_AZ => Some(2),
        // CLUSTER_MULTI_AZ documents no exact count.
        _ => None,
    };
    match required {
        Some(count) if subnet_ids.len() != count => Err(Invalid::new(
            ATTRIBUTE,
            format!(
                "a {deployment_mode} deployment requires {count} subnet(s), got {}",
                subnet_ids.len()
            ),
        )),
        _ => Ok(()),
    }
}

/// The page size a listing of this service uses when the request omits
/// `MaxResults`, from the member's own documentation: "The maximum number of
/// brokers that Amazon MQ can return per page (20 by default)", worded the same
/// way for every paginated operation of this service. The modelled `@range`
/// (1..100) bounds an explicit `MaxResults`; it is not the default.
pub const DEFAULT_MAX_RESULTS: usize = 20;

/// `MaxResults`' modelled `@range`, from
/// `smithy_query(query="operation", operation="ListBrokers")`:
/// `MaxResults { range: { min: 1, max: 100 } }` — the same `MaxResults` shape
/// every paginated operation of this service binds. The handler enforces it (see
/// [`validate_range`]) so that the refusal names the member.
pub const MIN_MAX_RESULTS: i32 = 1;

/// The upper end of `MaxResults`' modelled `@range` — see [`MIN_MAX_RESULTS`].
pub const MAX_MAX_RESULTS: i32 = 100;

/// Tags a page token as this listing's own, so a token from somewhere else
/// decodes to `None` rather than to an offset it was never meant to name.
const PAGE_TOKEN_PREFIX: &str = "mqb1";

/// One page of a listing: the slice to serialize and the token that resumes
/// after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub start: usize,
    pub end: usize,
    pub next_token: Option<String>,
}

/// Encode the offset a `NextToken` resumes at: the tag plus the offset in
/// zero-padded hexadecimal, so the token carries no readable number and a client
/// has to round-trip it rather than compute one.
pub fn encode_page_token(offset: usize) -> String {
    format!(
        "{PAGE_TOKEN_PREFIX}{offset:0width$x}",
        width = PAGE_TOKEN_DIGITS
    )
}

/// The number of hexadecimal digits [`encode_page_token`] emits, so a token is
/// recognised by its exact shape rather than by what a radix parser tolerates.
const PAGE_TOKEN_DIGITS: usize = 12;

/// Decode a `NextToken` this service issued. `None` for anything else — the
/// caller answers that with the modelled client-input error.
///
/// The digits are checked explicitly: `from_str_radix` alone would also accept a
/// sign prefix (`+1`) or a shorter run, which this service never emits, so such
/// a token would decode to an offset it was never issued for.
pub fn decode_page_token(token: &str) -> Option<usize> {
    let digits = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    if digits.len() != PAGE_TOKEN_DIGITS
        || !digits.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return None;
    }
    usize::from_str_radix(digits, 16).ok()
}

/// The sentence Amazon MQ answers a `NextToken` it did not issue with. Only the
/// token the request sent varies; it lives beside the codec that decides which
/// tokens are this service's.
pub fn invalid_page_token_message(token: &str) -> String {
    format!("Invalid next token [{token}]. Make sure you copied the token correctly.")
}

/// The page of `total` items `max_results` and `next_token` select.
///
/// A token past the end yields an empty page rather than an error, and the last
/// page carries no `NextToken`, so a paginating client terminates.
///
/// `max_results` is already inside the member's modelled `range` (1..100) by the
/// time a handler calls this — the handler refuses 0, anything negative and
/// anything past 100 itself (`validate_range`), so the `filter`/`unwrap_or`
/// below is a total-function fallback, not a silent clamp of a value a caller
/// could actually send.
pub fn page_of(total: usize, max_results: Option<i32>, offset: usize) -> Page {
    let size = max_results
        .and_then(|m| usize::try_from(m).ok())
        .filter(|m| *m > 0)
        .unwrap_or(DEFAULT_MAX_RESULTS);
    let start = offset.min(total);
    let end = start.saturating_add(size).min(total);
    Page {
        start,
        end,
        next_token: (end < total).then(|| encode_page_token(end)),
    }
}

/// The items a [`Page`] selects out of `items`, through a CHECKED accessor.
///
/// The page's bounds come from an offset a request supplied, so the slice is
/// taken with `get` and falls back to no items: a request can never index past
/// the end of what this call actually has, whatever offset its token carries.
/// Every listing of this service pages through this one function.
pub fn page_slice<'a, T>(items: &'a [T], page: &Page) -> &'a [T] {
    items.get(page.start..page.end).unwrap_or_default()
}

impl Page {
    /// The `NextToken` a listing that always reports one answers with: the token that
    /// resumes AFTER this page, for a page that carries at least one element —
    /// including the page that exhausts the collection, where [`page_of`] leaves the
    /// member out.
    ///
    /// Amazon MQ's revision listing carries the member on its last page too, so a
    /// client that paginates by the member's presence makes one more call; that call
    /// answers the empty page past the end, which carries no element and therefore no
    /// token, so the client still terminates. The token is produced by this module's
    /// own codec, so the one a page reports is one [`decode_page_token`] accepts.
    pub fn token_after(&self) -> Option<String> {
        self.next_token
            .clone()
            .or_else(|| (self.end > self.start).then(|| encode_page_token(self.end)))
    }
}

/// The region to build an ARN in: the one the caller signed for, else
/// [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Availability Zone letters every Region has: AWS documents that every Region has
/// a minimum of three zones, and most Regions' zone names end in these letters.
pub const COMMON_AVAILABILITY_ZONE_LETTERS: &str = "abc";

/// The Regions whose Availability Zone set is not [`COMMON_AVAILABILITY_ZONE_LETTERS`],
/// and the letters their zone names end in.
///
/// Source: AWS's published "Regions and Availability Zones" list (the zones each Region
/// offers). The letters are held rather than a count because a Region's zone letters are
/// not always consecutive. A Region this table does not name has the three zones above.
/// The zone set is a property of the Region, not of this service, so it is kept here
/// rather than in the operation that reports zones.
const AVAILABILITY_ZONE_LETTERS_BY_REGION: &[(&str, &str)] = &[
    ("us-east-1", "abcdef"),
    ("us-west-2", "abcd"),
    ("ap-northeast-1", "acd"),
    ("ap-northeast-2", "abcd"),
];

/// The letters `region`'s Availability Zone names end in.
pub fn availability_zone_letters(region: &str) -> &'static str {
    AVAILABILITY_ZONE_LETTERS_BY_REGION
        .iter()
        .find(|(named, _)| *named == region)
        .map_or(COMMON_AVAILABILITY_ZONE_LETTERS, |(_, letters)| *letters)
}

/// Every Availability Zone name of `region`, in order.
///
/// An AWS zone name is its Region's code followed by a letter
/// (`us-east-1a`), so the names follow from the Region the caller signed for
/// ([`request_region`]) and that Region's zone letters — never from the request
/// body. The list is the Region's FULL zone set: a caller reading it to place a
/// broker sees every zone the Region offers.
pub fn availability_zones_in_region(region: &str) -> Vec<String> {
    availability_zone_letters(region)
        .chars()
        .map(|letter| format!("{region}{letter}"))
        .collect()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMqMqState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMqMqState>)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page a client-supplied offset selects is always INSIDE the collection it
    /// pages, whatever integer the offset carries: a `nextToken` carries an offset a
    /// caller chose, so a page past the end is the empty last page — and the accessor
    /// every listing of this service pages through ([`page_slice`]) answers no items
    /// for it rather than indexing past the end.
    #[test]
    fn a_page_offset_past_the_end_selects_the_empty_last_page() {
        // (case, how many items are stored, the offset the token carried, the items
        // the page selects out of `0..total`)
        let rows: [(&str, usize, usize, &[usize]); 8] = [
            ("an empty store at the start", 0, 0, &[]),
            ("an empty store one past the end", 0, 1, &[]),
            ("an empty store at the largest offset", 0, usize::MAX, &[]),
            ("a populated store at the start", 3, 0, &[0, 1, 2]),
            ("a populated store inside the collection", 3, 1, &[1, 2]),
            ("a populated store exactly at the end", 3, 3, &[]),
            ("a populated store one past the end", 3, 4, &[]),
            ("a populated store at the largest offset", 3, usize::MAX, &[]),
        ];

        for (case, total, offset, expected) in rows {
            let page = page_of(total, Some(MAX_MAX_RESULTS), offset);
            assert!(
                page.start <= page.end && page.end <= total,
                "case {case:?}: expected a range inside 0..={total}, got \
                 {}..{}",
                page.start,
                page.end
            );
            let items: Vec<usize> = (0..total).collect();
            let selected = page_slice(&items, &page);
            assert_eq!(
                selected, expected,
                "case {case:?}: expected the items {expected:?}, got {selected:?}"
            );
            // A page that carries everything left, and one past the end, are both
            // the LAST page: a paginating client that followed this token stops.
            if page.end == total {
                assert!(
                    page.next_token.is_none(),
                    "case {case:?}: expected no nextToken on the last page, got {page:?}"
                );
            }
        }
    }
}
