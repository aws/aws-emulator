// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kafka-kafka service.
//!
//! The provisioned-cluster registry lives here, together with the renderings the
//! reporting operations derive from it: `DescribeCluster` / `ListClusters` build a
//! [`ClusterInfo`] with [`Cluster::to_cluster_info`], `DescribeClusterV2` /
//! `ListClustersV2` build the v2 `Cluster` with [`Cluster::to_cluster_v2`], and
//! `GetBootstrapBrokers` builds its endpoint strings with
//! [`Cluster::broker_endpoints`] plus the access predicates beside it, and
//! `ListNodes` renders one stored broker node with [`Cluster::to_node_info`].
//! Keeping them
//! next to the record is what makes the readers agree: they render the same stored
//! cluster the same way.
//!
//! A cluster comes in the two kinds the model's `ClusterType` names, and both live
//! in that one registry: `CreateCluster` and `CreateClusterV2`'s `Provisioned` body
//! mint a provisioned record with [`Cluster::provisioned`], while
//! `CreateClusterV2`'s `Serverless` body mints one with [`Cluster::serverless`],
//! whose [`ServerlessConfig`] is what the V2 readers derive the `Serverless` shape
//! from. One registry, so a cluster name is unique across both kinds and a reader
//! resolves either through the same lookups.
//!
//! The MSK configuration registry lives here for the same reason:
//! `CreateConfiguration` stores a [`ConfigurationRecord`] and both
//! `ListConfigurations` and `DescribeConfiguration` render it with
//! [`ConfigurationRecord::to_configuration`], so the writer and every reader
//! agree on one record. A configuration's revisions are an append-only history
//! on that record: `UpdateConfiguration` adds the next one with
//! [`ConfigurationRecord::append_revision`] and
//! `DescribeConfigurationRevision` reports a stored one through
//! [`ConfigurationRecord::revision`], so no update rewrites a revision an
//! earlier call already stored.
//!
//! A configuration's deletion is ASYNCHRONOUS, so it is accepted on the record
//! rather than applied to the registry: `DeleteConfiguration` moves it to
//! `DELETING` through [`ComAmazonawsKafkaKafkaState::begin_configuration_deletion`],
//! which records the instant the clock owes its removal, and the configuration
//! stays a resource every reader resolves and lists in that state until then.
//! The lookups ([`ComAmazonawsKafkaKafkaState::configuration_in`] for a read and
//! [`ComAmazonawsKafkaKafkaState::settled_configuration_in_mut`] for a call that
//! writes) are what materialise that removal, so every reader agrees about when
//! a deleted configuration is gone, and a call that CHANGES a configuration is
//! gated on the state it is in
//! ([`CONFIGURATION_STATES_A_CHANGE_IS_ACCEPTED_FROM`]) rather than reporting a
//! resource it still holds as a reference it cannot resolve.
//!
//! The topic registry lives beside it, keyed by (cluster ARN, topic name): a topic
//! belongs to exactly one cluster, so `CreateTopic`, `DescribeTopic`, `ListTopics`,
//! `UpdateTopic` and `DeleteTopic` all resolve the cluster first and then the
//! topic, through
//! [`ComAmazonawsKafkaKafkaState::settled_topic_in`] (or
//! [`ComAmazonawsKafkaKafkaState::topics_in`] for a listing) — the one
//! accessor each, so every caller sees the transitions the clock already owed that
//! topic. A change to a topic comes through
//! [`ComAmazonawsKafkaKafkaState::start_topic_update`], the one write path, which
//! is what serializes two concurrent changes to one topic.
//! A cluster leaving the registry takes its topics with it, so no topic can
//! outlive the cluster that holds it. Those operations serve a provisioned cluster
//! only: each refuses a serverless one on its kind, in the same words, through
//! [`serverless_topic_apis_unsupported`].
//!
//! A cluster reports the topics a running replicator REPLICATES onto it as well as
//! the ones it holds itself, and the topic readers answer from the one derivation
//! that puts the two together:
//! [`ComAmazonawsKafkaKafkaState::observable_topics_in`] for a listing and
//! [`ComAmazonawsKafkaKafkaState::observable_topic_in`] for one name. A replicated
//! topic is DERIVED from the source cluster's own topic rather than stored a second
//! time, so a listing and a describe of one cluster cannot disagree, and the
//! deletion of the source topic or of the replicator withdraws it without anything
//! having to remember to.
//!
//! A cluster's configuration also changes after creation: an update operation
//! (`UpdateConnectivity`, `UpdateStorage`, `UpdateBrokerStorage`,
//! `UpdateClusterKafkaVersion`, `UpdateClusterConfiguration`,
//! `UpdateSecurity`, `UpdateBrokerCount`, `UpdateRebalancing`) records a
//! [`PendingUpdate`] on the cluster, and [`Cluster::settle`] folds it into the
//! record once the clock owes its completion — so one place decides what every
//! reader then reports. The operation itself is kept in the cluster-operation
//! registry, which is what its `ClusterOperationArn` names and what
//! `DescribeClusterOperation` renders with
//! [`ClusterOperation::to_cluster_operation_info`] and
//! `DescribeClusterOperationV2` with
//! [`ClusterOperation::to_cluster_operation_v2`] — one stored record, two
//! renderings, so the V1 and V2 readers of one operation cannot disagree. A
//! cluster's whole history is derived once, by
//! [`ComAmazonawsKafkaKafkaState::cluster_operation_history`], which is what
//! `ListClusterOperations` pages: the listing reports the same records the
//! by-ARN readers resolve, in the one order
//! [`ClusterOperation::history_key`] defines.
//!
//! The channel registry lives beside it, keyed by channel ARN and indexed by
//! (cluster ARN, channel name) — a channel is a sub-resource of the cluster it
//! streams from, so it is only ever resolved through that cluster and disappears
//! with it. `DeleteChannel` accepts a channel's deletion through
//! [`ComAmazonawsKafkaKafkaState::begin_channel_deletion`], which moves it to
//! `DELETING` and records the instant the clock owes its removal, and the
//! channel lookups ([`ComAmazonawsKafkaKafkaState::settled_channel_in`] and the
//! by-ARN pair the tagging operations resolve through) materialise that removal
//! — so every reader agrees about when a deleted channel is gone.
//!
//! The replicator registry sits beside them on the same principle: a [`Replicator`]
//! holds the whole replication setup, and the readers derive their shapes from that
//! one record — [`Replicator::to_replicator_summary`] for `ListReplicators`, and
//! [`Replicator::kafka_cluster_descriptions`] /
//! [`Replicator::replication_info_descriptions`] for `DescribeReplicator`.
//!
//! The service's Apache Kafka version catalog lives beside them in
//! [`kafka_versions`] ([`KAFKA_VERSION_CATALOG`], with
//! [`catalogued_kafka_versions`], [`kafka_version_status`] and
//! [`compatible_target_versions`]), for the same reason: it is the single list
//! every version-aware operation answers from — the listing publishes its rows,
//! and everything else asks it about a version.
//!
//! A cluster's resource-based policy is held ON that cluster record, as a
//! [`ClusterPolicy`]: `PutClusterPolicy` attaches or replaces it through
//! [`Cluster::put_policy`], `GetClusterPolicy` reports the stored document and
//! its version, and `DeleteClusterPolicy` takes it off through
//! [`Cluster::take_policy`] — one stored document, so the writer and both
//! readers cannot disagree, and a cluster that leaves the registry takes its
//! policy with it.
//!
//! A cluster's SASL/SCRAM secrets are held ON the record for the same reason, as
//! the [`Cluster::scram_secret_arns`] set: `BatchAssociateScramSecret` adds to it
//! through [`Cluster::associate_scram_secrets`],
//! `BatchDisassociateScramSecret` removes from it through
//! [`Cluster::disassociate_scram_secrets`] and `ListScramSecrets` pages that one
//! set, so the two writers and the reader cannot disagree about which secrets a
//! cluster authenticates its clients with, and the secrets leave with the
//! cluster.
//!
//! The multi-VPC (PrivateLink) connection registry lives here too:
//! `CreateVpcConnection` mints a [`VpcConnection`] with
//! [`VpcConnection::requested`] and `DescribeVpcConnection` reports that same
//! stored record, while `ListVpcConnections` pages the region's connections and
//! renders each one with [`VpcConnection::to_vpc_connection`] — so a listed
//! connection and a described one are two views of one record.
//! `ListClientVpcConnections` pages the SAME registry narrowed to one target
//! cluster, rendering each record with
//! [`VpcConnection::to_client_vpc_connection`], so a cluster's client
//! connections are that same one record seen from the cluster's side.
//! The cluster owner's `RejectClientVpcConnection` writes to that same record
//! through [`VpcConnection::reject`], which starts the `REJECTING` ->
//! `REJECTED` transition [`VpcConnection::settle`] then materialises, so every
//! reader of a rejected connection reports the one state it holds.

/// The account's EC2 subnets, which the creates that place a resource in a client
/// VPC resolve what they are given against. They are not this service's resources,
/// so they are not held here — they are resolved, which is why that module sits
/// beside this one rather than in it.
pub mod network;

/// The authorization decision a cluster-scoped operation takes on the ARN its
/// request names, BEFORE the resolution below runs. It is decided from the request
/// alone, which is why it sits beside this module rather than in it — nothing it
/// reads is held here.
pub mod authorization;

/// What the CALLING PRINCIPAL's own identity-based policies say about the action a
/// request asks for, decided before anything the request names is resolved. It
/// reads the IAM and STS stores for the principal the credential speaks for, which
/// is held by those services rather than here, so it sits beside this module too.
pub mod identity_authorization;

/// The preconditions Amazon MSK checks on the TARGET CLUSTER before it manages a
/// VPC connection to it: the cluster's state, and the client authentication the
/// connection asks for. Each is a function of one cluster record and the request,
/// so it sits beside this module rather than in it.
pub mod vpc_connection_admission;

/// Whether ONE secret a SCRAM-secret batch names can serve as a cluster's SASL/SCRAM
/// credential. The secrets are AWS Secrets Manager's resources, not this service's, so
/// they are not held here — they are resolved, which is why that module sits beside
/// this one rather than in it.
pub mod scram_secret_admission;

/// Whether a submitted `server.properties` body is a document this service can
/// store as a revision. A function of the submitted bytes alone, so it sits
/// beside this module rather than in it.
pub mod server_properties;

/// The state precondition a call that CHANGES a resource applies, and the one
/// sentence the service refuses it with when the resource is in some other state.
/// It is a function of the accepted-state set and nothing this module holds, which
/// is why it sits beside this one.
pub mod state_gate;

/// The registry's own operations — every lookup, insertion and removal over the
/// maps the state struct below holds. Its own file so this module keeps to the
/// RECORDS and the service's fixed values.
mod registry;

/// What one cluster record computes about itself: its states, its endpoints, its
/// client authentication, and the changes a completed operation applies to it.
mod cluster;

/// The same for one replicator record.
mod replicator;

/// The same for one channel record: how it is minted, how its status settles, and
/// the destination change an update applies to it.
mod channel;
/// The service's published Apache Kafka version catalog: the rows the listing
/// pages, and every version-aware answer derived from them.
mod kafka_versions;
pub use kafka_versions::*;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use regex::RegexSet;

use crate::ops::types::{
    BadRequestException, BrokerCountUpdateInfo, BrokerEBSVolumeInfo, BrokerNodeGroupInfo,
    BrokerNodeInfo,
    BrokerSoftwareInfo,
    ChannelInfo, ChannelLoggingInfo, ClientAuthentication, ClientVpcConnection, ClusterInfo,
    ClusterOperationInfo,
    ClusterOperationV2,
    ClusterOperationV2Provisioned,
    ClusterOperationV2Summary,
    ConflictException, Configuration,
    ConfigurationInfo, ConfigurationRevision, ConnectivityInfo, ConsumerGroupReplicationUpdate,
    CreateChannelInput,
    CreateClusterInput, CreateConfigurationInput, CreateReplicatorInput, CreateVpcConnectionInput,
    DescribeChannelOutput, EBSStorageInfo, EncryptionAtRest, EncryptionConfiguration,
    EncryptionInTransit, EncryptionInfo, IcebergDestinationConfiguration, JmxExporter,
    JmxExporterInfo, KafkaCluster, KafkaClusterDescription, KafkaClusterSummary, LogDelivery,
    LoggingInfo, MutableClusterInfo, NodeExporter, NodeExporterInfo, NodeInfo, OpenMonitoring,
    OpenMonitoringInfo, Prometheus, PrometheusInfo, Provisioned, ProvisionedRequest,
    ProvisionedThroughput, Rebalancing, ReplicationInfo, ReplicationInfoDescription,
    ReplicationInfoSummary, ReplicationStartingPosition, ReplicationStateInfo,
    ReplicationTopicNameConfiguration, ReplicatorSummary, S3DestinationConfiguration, Serverless,
    ServerlessClientAuthentication, ServerlessConnectivityInfo, ServerlessRequest, StorageInfo,
    TopicConfiguration, TopicInfo, TopicReplication, TopicReplicationUpdate, VpcConfig,
    ZookeeperAccess,
};

/// How long after a cluster create a new cluster becomes `ACTIVE`.
///
/// CHOSEN, not documented: the real service takes many minutes to provision
/// brokers. The project fixes an emulated clock-owed transition at 250–750 ms, so
/// a caller that describes the cluster immediately still observes `CREATING` (and
/// the state errors that go with it) while a caller that waits sees `ACTIVE`.
pub const CLUSTER_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The `KafkaBrokerNodeId` a broker-storage update must name, per
/// `BrokerEBSVolumeInfo`'s model documentation: "The broker identifier must be set
/// to the keyword ALL. This means the changes apply to all the brokers in the
/// cluster."
pub const ALL_BROKERS: &str = "ALL";

/// The smallest EBS data-volume size a broker may be given, from the `range` on
/// `EBSStorageInfo$VolumeSize` (`__integerMin1Max16384`, 1..16384) in the model.
pub const MIN_VOLUME_SIZE_GB: i32 = 1;

/// The largest EBS data-volume size a broker may be given, from the same `range`.
///
/// `UpdateStorageRequest$VolumeSizeGB` is a plain `__integer`, so the boundary
/// validator lets any value through: the range belongs to the stored
/// `EBSStorageInfo$VolumeSize` the update writes, and the handler is what
/// enforces it.
pub const MAX_VOLUME_SIZE_GB: i32 = 16_384;

/// How many cluster operations the registry keeps for one cluster, newest first.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: every update call adds
/// one operation record, so an unbounded history would grow the store with the
/// number of calls. Sized well past any realistic client script's update count;
/// a caller holding an ARN evicted past it reads the same `NotFoundException` as
/// for an operation that never existed.
pub const MAX_OPERATIONS_PER_CLUSTER: usize = 64;

/// The `OperationType` an `UpdateBrokerType` call records, in the same form.
pub const OPERATION_TYPE_UPDATE_BROKER_TYPE: &str = "UPDATE_BROKER_TYPE";

/// The `OperationType` an `UpdateBrokerCount` call records, in the same form.
pub const OPERATION_TYPE_UPDATE_BROKER_COUNT: &str = "UPDATE_BROKER_COUNT";

/// The `OperationType` an `UpdateStorage` call records, which is the API name in
/// the SCREAMING_SNAKE_CASE form MSK's cluster-operation types take.
pub const OPERATION_TYPE_UPDATE_STORAGE: &str = "UPDATE_STORAGE";

/// The `OperationType` an `UpdateBrokerStorage` call records, in the same form.
pub const OPERATION_TYPE_UPDATE_BROKER_STORAGE: &str = "UPDATE_BROKER_STORAGE";

/// The `OperationType` an `UpdateConnectivity` call records, in the same form.
pub const OPERATION_TYPE_UPDATE_CONNECTIVITY: &str = "UPDATE_CONNECTIVITY";

/// The `OperationType` an `UpdateClusterKafkaVersion` call records, in the same
/// form.
pub const OPERATION_TYPE_UPDATE_CLUSTER_KAFKA_VERSION: &str = "UPDATE_CLUSTER_KAFKA_VERSION";

/// The `OperationType` an `UpdateClusterConfiguration` call records, in the same
/// form.
pub const OPERATION_TYPE_UPDATE_CLUSTER_CONFIGURATION: &str = "UPDATE_CLUSTER_CONFIGURATION";

/// The `OperationType` an `UpdateSecurity` call records, in the same form.
pub const OPERATION_TYPE_UPDATE_SECURITY: &str = "UPDATE_SECURITY";

/// The `OperationType` an `UpdateMonitoring` call records, in the same form.
pub const OPERATION_TYPE_UPDATE_MONITORING: &str = "UPDATE_MONITORING";

/// The `OperationType` an `UpdateRebalancing` call records, in the same form:
/// the API's own name in SCREAMING_SNAKE_CASE, which is how every update above
/// names the operation it files.
pub const OPERATION_TYPE_UPDATE_REBALANCING: &str = "UPDATE_REBALANCING";

/// The `OperationType` a `RebootBroker` call records.
///
/// A reboot acts on a NODE of the cluster rather than on one of its settings —
/// `RebootBrokerRequest`'s own documentation is "Reboots a node." — which is the
/// operation type Amazon MSK files such a call under, in the same
/// SCREAMING_SNAKE_CASE form as the update types above.
pub const OPERATION_TYPE_REBOOT_NODE: &str = "REBOOT_NODE";

/// The `OperationType` the cluster operation behind a `CreateChannel` records,
/// in the same SCREAMING_SNAKE_CASE form as the update types above: a channel
/// lifecycle call is carried out by a cluster operation of its own, which is
/// what the `ClusterOperationArn` each of those calls answers names.
pub const OPERATION_TYPE_CREATE_CHANNEL: &str = "CREATE_CHANNEL";

/// The `OperationType` the cluster operation behind an `UpdateChannel` records.
pub const OPERATION_TYPE_UPDATE_CHANNEL: &str = "UPDATE_CHANNEL";

/// The `OperationType` the cluster operation behind a `DeleteChannel` records.
pub const OPERATION_TYPE_DELETE_CHANNEL: &str = "DELETE_CHANNEL";

/// The state MSK reports for a cluster operation that is still applying a
/// configuration change.
pub const OPERATION_STATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";

/// The state MSK reports for such an operation once it has finished.
pub const OPERATION_STATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// The state MSK reports for a cluster operation that is restarting a broker
/// node, and the state it reports once that node is back.
///
/// A cluster operation's state names the WORK it does rather than the fact that
/// something is in flight, so a node reboot is reported in its own pair of states
/// — which is what a client polling `DescribeClusterOperation` after a
/// `RebootBroker` waits for.
pub const OPERATION_STATE_REBOOT_IN_PROGRESS: &str = "REBOOT_IN_PROGRESS";
/// See [`OPERATION_STATE_REBOOT_IN_PROGRESS`].
pub const OPERATION_STATE_REBOOT_COMPLETE: &str = "REBOOT_COMPLETE";

/// The pair of states — (still running, finished) — a cluster operation of
/// `operation_type` is reported in.
///
/// Derived from the operation's own type, in ONE place, so every reader of a
/// cluster operation (`DescribeClusterOperation`, `DescribeClusterOperationV2`
/// and the two listings) reports one state for one record: a reboot of a node
/// walks [`OPERATION_STATE_REBOOT_IN_PROGRESS`] -> [`OPERATION_STATE_REBOOT_COMPLETE`],
/// and every other type this service files is a change to the cluster's
/// configuration, which walks the `UPDATE_*` pair.
pub fn operation_states_of(operation_type: &str) -> (&'static str, &'static str) {
    if operation_type == OPERATION_TYPE_REBOOT_NODE {
        (
            OPERATION_STATE_REBOOT_IN_PROGRESS,
            OPERATION_STATE_REBOOT_COMPLETE,
        )
    } else {
        (OPERATION_STATE_IN_PROGRESS, OPERATION_STATE_COMPLETE)
    }
}

/// The `ClusterState` of a cluster whose brokers are serving and which accepts an
/// update. One of the model's `ClusterState` values
/// (`crate::ops::types::CLUSTER_STATE_VALUES`), as are the two below.
pub const CLUSTER_STATE_ACTIVE: &str = "ACTIVE";

/// The `ClusterState` of a cluster that is being provisioned.
pub const CLUSTER_STATE_CREATING: &str = "CREATING";

/// The `ClusterState` of a cluster an accepted update is still being applied to.
pub const CLUSTER_STATE_UPDATING: &str = "UPDATING";

/// The `ClusterState` of a cluster whose provisioning or repair did not succeed.
pub const CLUSTER_STATE_FAILED: &str = "FAILED";

/// The `ClusterState` of a cluster the service is repairing a broker of.
pub const CLUSTER_STATE_HEALING: &str = "HEALING";

/// The `ClusterState` of a cluster the service is applying maintenance to.
pub const CLUSTER_STATE_MAINTENANCE: &str = "MAINTENANCE";

/// The `ClusterState` of a cluster whose brokers are being rebooted.
pub const CLUSTER_STATE_REBOOTING_BROKER: &str = "REBOOTING_BROKER";

/// The `ClusterState` values a cluster delete is accepted from.
///
/// These are the two values of the model's `ClusterState` enum
/// (`crate::ops::types::CLUSTER_STATE_VALUES`) a cluster STAYS in until a caller
/// asks for something else; every other value (`CREATING`, `UPDATING`,
/// `DELETING`, `HEALING`, `MAINTENANCE`, `REBOOTING_BROKER`) is one the cluster
/// is still moving out of under the service's own control, and a destructive
/// call made while it is in flight is refused rather than applied. A cluster
/// whose provisioning failed is still deletable — otherwise nothing could ever
/// clear it.
pub const CLUSTER_STATES_DELETE_IS_ACCEPTED_FROM: [&str; 2] =
    [CLUSTER_STATE_ACTIVE, CLUSTER_STATE_FAILED];

/// Whether a cluster reported in `state` may be deleted, per
/// [`CLUSTER_STATES_DELETE_IS_ACCEPTED_FROM`]. One predicate, so every caller
/// that destroys a cluster decides it the same way.
pub fn is_deletable_cluster_state(state: &str) -> bool {
    CLUSTER_STATES_DELETE_IS_ACCEPTED_FROM.contains(&state)
}

/// Whether a cluster reported in `state` has its broker nodes yet.
///
/// A cluster the service is still provisioning has none — that is what
/// `CREATING` means — so an operation whose answer IS the brokers (their
/// endpoints, their node records) or which acts on one of them has nothing to
/// serve from until the cluster leaves that state. Every later state has the
/// brokers, including the ones a cluster passes through while an update or a
/// repair is applied to them.
pub fn cluster_has_broker_nodes(state: &str) -> bool {
    state != CLUSTER_STATE_CREATING
}

/// How long a cluster update (`UpdateConnectivity`, `UpdateStorage`,
/// `UpdateBrokerStorage` and the
/// sibling update operations) stays in flight before the cluster leaves
/// `UPDATING` — and equally how long a `RebootBroker` stays in flight before the
/// cluster leaves `REBOOTING_BROKER`, since both are one cluster operation of the
/// same registry.
///
/// CHOSEN, not documented: the real service reconfigures or restarts brokers over
/// minutes.
/// The project fixes an emulated clock-owed transition at 250–750 ms, so a caller
/// that describes the cluster immediately still observes `UPDATING`, its active
/// operation ARN and the pre-update configuration, while a caller that waits sees
/// the new configuration and the new cluster version.
pub const CLUSTER_UPDATE_DELAY: Duration = Duration::from_millis(500);

/// How long a `RebootBroker` stays in flight before the cluster leaves
/// `REBOOTING_BROKER` and its operation reports
/// [`OPERATION_STATE_REBOOT_COMPLETE`].
///
/// CHOSEN, not documented: the real service rolls a broker out of the cluster and
/// waits for it to rejoin, which takes minutes. It is its own span rather than
/// [`CLUSTER_UPDATE_DELAY`] because what a reboot has to be observable AS is a
/// rolling restart: the cluster keeps serving throughout it, so a client that
/// drains or rebalances around the restart — and a reader that checks the broker
/// inventory never shrinks — must be able to sample the in-flight operation
/// several times in a row at a few seconds' cadence. Sized for that, and still far
/// inside the interval a client polling at an AWS-sized cadence waits, so no
/// poller pays an extra interval for it.
pub const BROKER_REBOOT_DELAY: Duration = Duration::from_secs(5);

/// Shortest and longest a broker instance type may be, from the `length` trait on
/// `BrokerNodeGroupInfo$InstanceType` and `MutableClusterInfo$InstanceType`
/// (`__stringMin5Max32`, 5..32) in the model.
///
/// `UpdateBrokerTypeRequest$TargetInstanceType` is a plain `__string`, so the
/// boundary validator lets any value through: the constraint belongs to the
/// member the update WRITES, and [`is_broker_instance_type`] is what enforces it.
pub const INSTANCE_TYPE_MIN_LEN: usize = 5;
/// See [`INSTANCE_TYPE_MIN_LEN`].
pub const INSTANCE_TYPE_MAX_LEN: usize = 32;

/// The largest number of broker nodes one provisioned cluster may hold, from the
/// `range` on `CreateClusterRequest$NumberOfBrokerNodes` (`__integerMin1Max15`,
/// 1..15) in the model. Enforced by the handler AND clamped in
/// [`Cluster::provisioned`], so no caller of this module can make it allocate a
/// broker list sized by an unchecked request value.
pub const MAX_BROKER_NODES_PER_CLUSTER: i32 = 15;

/// Clusters one account may hold in one region, from the Amazon MSK service
/// quota "Number of clusters per account" (90, per Region). A create past it is
/// refused with the modelled `TooManyRequestsException`, so the registry's
/// cardinality is bounded rather than growing with the number of calls.
pub const MAX_CLUSTERS_PER_REGION: usize = 90;

/// Absolute ceiling on the registry across every region a caller signs for.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: the signing region
/// comes from the request, so the per-region quota alone would still let a caller
/// grow the store by naming new regions. Sized at the per-region quota times more
/// regions than AWS publishes, so it can only ever be reached by a caller trying
/// to exhaust the emulator, never by a realistic client script.
pub const MAX_CLUSTERS_IN_REGISTRY: usize = MAX_CLUSTERS_PER_REGION * 64;

/// How many entries one list of a serverless cluster's network configuration may
/// carry: `ServerlessRequest$VpcConfigs`, and each `VpcConfig`'s subnets and
/// security groups.
///
/// CHOSEN as a process-safety backstop, not a published AWS quota (the model
/// declares no `length` on any of those lists): the lists come from the request
/// and are STORED on the cluster, so without a ceiling one create would size
/// every later read of that cluster. Sized well above the handful of VPCs and
/// subnets a serverless cluster is really reached through.
pub const MAX_SERVERLESS_VPC_ENTRIES: usize = 16;

/// How long after `CreateVpcConnection` a new VPC connection becomes
/// `AVAILABLE`.
///
/// CHOSEN, not documented: the real service takes minutes to place the
/// PrivateLink endpoint. The project fixes an emulated clock-owed transition at
/// 250–750 ms, so a caller that describes the connection immediately still
/// observes `CREATING` while a caller that waits sees `AVAILABLE`.
pub const VPC_CONNECTION_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The `VpcConnectionState` of a connection whose PrivateLink endpoint is still
/// being placed. One of the model's `VpcConnectionState` values
/// (`crate::ops::types::VPC_CONNECTION_STATE_VALUES`), as are the three below.
pub const VPC_CONNECTION_STATE_CREATING: &str = "CREATING";

/// The `VpcConnectionState` of a connection that is in service.
pub const VPC_CONNECTION_STATE_AVAILABLE: &str = "AVAILABLE";

/// The `VpcConnectionState` of a connection the target cluster's owner has
/// rejected and whose rejection is still being applied.
pub const VPC_CONNECTION_STATE_REJECTING: &str = "REJECTING";

/// The `VpcConnectionState` of a connection whose rejection by the target
/// cluster's owner has been applied: the connection stays a resource every
/// reader resolves, in the state that says it carries no traffic.
pub const VPC_CONNECTION_STATE_REJECTED: &str = "REJECTED";

/// How long after `RejectClientVpcConnection` a rejected connection reaches
/// `REJECTED`.
///
/// CHOSEN, not documented, for the same reason as
/// [`VPC_CONNECTION_ACTIVATION_DELAY`] and sized the same: the real service
/// withdraws the PrivateLink endpoint's acceptance, and the project fixes an
/// emulated clock-owed transition at 250–750 ms, so a caller that reads the
/// connection immediately still observes `REJECTING` while a caller that waits
/// sees `REJECTED`.
pub const VPC_CONNECTION_REJECTION_DELAY: Duration = Duration::from_millis(500);

/// Absolute ceiling on the VPC-connection registry.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: the registry must not
/// grow without bound with the number of calls a client makes. Sized far above
/// anything a realistic client script creates, so it can only be reached by a
/// caller trying to exhaust the emulator.
pub const MAX_VPC_CONNECTIONS_IN_REGISTRY: usize = 1024;

/// The authentication types an MSK VPC connection may use, from Amazon MSK's
/// multi-VPC private connectivity documentation: a connection authenticates with
/// SASL/IAM, SASL/SCRAM or TLS. `CreateVpcConnectionRequest$Authentication` is a
/// plain `__string` in the model, so the handler is what refuses anything else.
///
/// Spelled from the values the admission gate decides each scheme by
/// ([`vpc_connection_admission`]), so the published set and the gate cannot drift
/// apart.
pub const VPC_CONNECTION_AUTHENTICATION_TYPES: [&str; 3] = [
    vpc_connection_admission::AUTHENTICATION_SASL_IAM,
    vpc_connection_admission::AUTHENTICATION_SASL_SCRAM,
    vpc_connection_admission::AUTHENTICATION_TLS,
];

/// How long after `CreateTopic` a new topic becomes `ACTIVE`.
///
/// CHOSEN, not documented: the real service creates the topic on the cluster's
/// brokers, which takes as long as it takes. The project fixes an emulated
/// clock-owed transition at 250–750 ms, so a caller that describes the topic
/// immediately still observes `CREATING` while one that waits sees `ACTIVE`.
pub const TOPIC_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long after `DeleteTopic` a topic marked `DELETING` is actually gone.
///
/// CHOSEN, not documented, for the same reason: `DeleteTopic` answers the
/// non-terminal `DELETING` status, so the record must remain readable for a span
/// before it leaves the registry.
pub const TOPIC_DELETION_DELAY: Duration = Duration::from_millis(500);

/// The longest legal Apache Kafka topic name: 249 characters
/// (`org.apache.kafka.common.internals.Topic.MAX_NAME_LENGTH`, which reserves
/// room for the `-<partition>` suffix a log directory name adds).
pub const MAX_TOPIC_NAME_LENGTH: usize = 249;

/// Topics one cluster may hold in this emulator.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: Amazon MSK bounds a
/// cluster by partitions per broker rather than by topic count, and the topic
/// name comes from the request, so without a ceiling a caller could grow the
/// registry one topic at a time. Sized well above any realistic client script.
pub const MAX_TOPICS_PER_CLUSTER: usize = 1_000;

/// Absolute ceiling on the topic registry across every cluster it holds.
///
/// CHOSEN as a process-safety backstop, not an AWS quota, for the reason
/// [`MAX_CLUSTERS_IN_REGISTRY`] exists: the cluster a topic is filed under comes
/// from the request, so the per-cluster ceiling alone would bound each cluster's
/// topics without bounding the registry. Sized at the per-cluster ceiling times
/// more clusters than one client script drives, so it can only ever be reached
/// by a caller trying to exhaust the emulator.
pub const MAX_TOPICS_IN_REGISTRY: usize = MAX_TOPICS_PER_CLUSTER * 16;

/// The `TopicState` values this emulator reports, spelled as the model's enum
/// declares them. Named here so every topic operation reports the same
/// spellings.
pub const TOPIC_STATE_CREATING: &str = "CREATING";
/// The state of a topic the cluster has finished creating.
pub const TOPIC_STATE_ACTIVE: &str = "ACTIVE";
/// The state of a topic a change the service has accepted is being applied to —
/// what `UpdateTopic` answers with and leaves the topic in until the clock owes
/// the change its completion.
pub const TOPIC_STATE_UPDATING: &str = "UPDATING";
/// The state of a topic whose deletion has been accepted but not completed.
pub const TOPIC_STATE_DELETING: &str = "DELETING";

/// How long after `CreateChannel` a new channel becomes `ACTIVE`.
///
/// CHOSEN, not documented: the real service takes a while to stand a channel's
/// delivery pipeline up. The project fixes an emulated clock-owed transition at
/// 250–750 ms, so a caller that describes the channel immediately still observes
/// `CREATING` while a caller that waits sees `ACTIVE`.
pub const CHANNEL_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long after an accepted `UpdateChannel` the channel leaves `UPDATING`.
///
/// CHOSEN, not documented, and set to the SAME span as
/// [`CHANNEL_ACTIVATION_DELAY`] and [`CHANNEL_DELETION_DELAY`]: the destination
/// change is applied asynchronously, so the channel reports the non-terminal
/// `UPDATING` for a span and a caller that waits sees it `ACTIVE` again, and the
/// project fixes an emulated clock-owed transition at 250–750 ms. Kept at the
/// bottom of that band deliberately: a client polling a channel at an
/// AWS-appropriate cadence pays its poll interval, not this span, for each
/// transition, so the shorter this is the less of its budget the emulator's own
/// asynchrony can consume.
pub const CHANNEL_UPDATE_DELAY: Duration = Duration::from_millis(500);

/// How long after `DeleteChannel` a channel marked `DELETING` is actually gone.
///
/// CHOSEN, not documented, for the same reason: `DeleteChannel` is documented to
/// move the channel through the non-terminal `DELETING` status and remove it once
/// the asynchronous delete completes, so the record must stay readable for a span
/// before it leaves the registry.
pub const CHANNEL_DELETION_DELAY: Duration = Duration::from_millis(500);

/// The `ChannelStatus` values this emulator reports, spelled as the model's enum
/// declares them. Named here so every channel operation reports the same
/// spellings. (`FAILED`, `SUSPENDING` and `SUSPENDED`, the model's other values,
/// belong to failures and to a suspend surface this emulator does not serve.)
pub const CHANNEL_STATUS_CREATING: &str = "CREATING";
/// The status of a channel whose delivery pipeline is up.
pub const CHANNEL_STATUS_ACTIVE: &str = "ACTIVE";
/// The status of a channel an accepted destination change is still being applied
/// to.
pub const CHANNEL_STATUS_UPDATING: &str = "UPDATING";
/// The status of a channel whose deletion has been accepted but not completed.
pub const CHANNEL_STATUS_DELETING: &str = "DELETING";

/// Topic configurations one channel carries: `CreateChannelRequest$TopicConfigurationList`
/// documents "Currently exactly one topic must be specified", so a list of any
/// other length is refused with the modelled `BadRequestException`.
pub const TOPIC_CONFIGURATIONS_PER_CHANNEL: usize = 1;

/// Channels one cluster may hold.
///
/// CHOSEN as a process-safety backstop, not a published AWS quota: a channel
/// record is created by a request, so without a ceiling the registry would grow
/// with the number of calls. Sized well above what any realistic client script
/// creates against one cluster.
pub const MAX_CHANNELS_PER_CLUSTER: usize = 100;

/// Absolute ceiling on the channel registry across every cluster it holds.
///
/// CHOSEN as a process-safety backstop, not a published AWS quota, for the same
/// reason [`MAX_TOPICS_IN_REGISTRY`] exists: the cluster a channel is filed
/// under comes from the request, so the per-cluster ceiling alone leaves the
/// registry itself unbounded. Sized at the per-cluster ceiling times more
/// clusters than one client script drives.
pub const MAX_CHANNELS_IN_REGISTRY: usize = MAX_CHANNELS_PER_CLUSTER * 64;

/// How long after `CreateReplicator` a new replicator becomes `RUNNING`.
///
/// CHOSEN, not documented: the real service takes minutes to stand a MirrorMaker
/// fleet up. The project fixes an emulated clock-owed transition at 250–750 ms, so
/// a caller that describes the replicator immediately still observes `CREATING`
/// while a caller that waits sees `RUNNING` — the same span, and the same
/// reasoning, as [`CLUSTER_ACTIVATION_DELAY`].
pub const REPLICATOR_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// Absolute ceiling on the replicator registry.
///
/// CHOSEN as a process-safety backstop, not an AWS quota (AWS publishes no
/// per-region replicator limit this emulator could cite): the signing region comes
/// from the request, so without a ceiling a caller could grow the store by naming
/// new regions. Sized well past anything a realistic client script creates, so it
/// can only be reached by a caller trying to exhaust the emulator.
pub const MAX_REPLICATORS_IN_REGISTRY: usize = 1024;

/// The most `KafkaClusters` entries one replicator RECORD may hold.
///
/// CHOSEN, not an AWS quota (the model declares `__listOfKafkaCluster` with no
/// `length` trait): a replicator's cluster list and its replication list are both
/// caller-sized, and every reader resolves each replication against the cluster
/// list, so without ceilings on BOTH one create would size the work of every
/// later read. `CreateReplicator` admits exactly
/// [`KAFKA_CLUSTERS_PER_REPLICATOR`] entries, so this is the backstop the
/// record-building path keeps rather than a bound a caller meets, and the product
/// with [`MAX_FLOWS_PER_REPLICATOR`] stays small.
pub const MAX_CLUSTERS_PER_REPLICATOR: usize = 16;

/// The most `ReplicationInfoList` entries one replicator may hold. CHOSEN for the
/// same reason, and on the same model evidence, as
/// [`MAX_CLUSTERS_PER_REPLICATOR`]: a replicator cannot hold more distinct
/// ordered cluster pairs than this anyway.
pub const MAX_FLOWS_PER_REPLICATOR: usize = 32;

/// How many Kafka clusters ONE replicator replicates between.
///
/// A replicator carries one source and one target — `KafkaClusters` is documented
/// as the "Kafka Clusters to use in setting up sources / targets for replication"
/// and every `ReplicationInfo` names one of each — so the list names exactly that
/// many, and a create naming any other number is refused with
/// [`KAFKA_CLUSTER_COUNT_MESSAGE`] before anything is minted. The model declares no
/// `length` on `__listOfKafkaCluster`, so the service is what states the count; the
/// behavioural record for a list of one is that refusal rather than a replicator.
pub const KAFKA_CLUSTERS_PER_REPLICATOR: usize = 2;

/// What MSK answers a `KafkaClusters` list that does not name
/// [`KAFKA_CLUSTERS_PER_REPLICATOR`] clusters: fixed text end to end, carrying
/// neither the submitted count nor the required one.
pub const KAFKA_CLUSTER_COUNT_MESSAGE: &str = "Too many or too few kafka clusters specified.";

/// What MSK answers a `KafkaClusters` list that names one Kafka cluster more
/// than once: the list is a SET of clusters — a replicator replicates between
/// distinct clusters — so a repeated cluster ARN is refused with this fixed
/// text, which names neither the repeated ARN nor its position.
pub const DUPLICATE_KAFKA_CLUSTERS_MESSAGE: &str = "Kafka cluster list contains duplicate cluster \
     ARNs. Please ensure all the cluster ARNs are unique.";

/// The fewest subnets ONE Kafka cluster's client VPC configuration may name
/// (`KafkaClusterClientVpcConfig$SubnetIds`, "The list of subnets in the client VPC
/// to connect to").
///
/// Amazon MSK creates the elastic network interfaces a replicator reaches a Kafka
/// cluster through one per Availability Zone, and a client VPC configuration spans
/// two or three of them — the same two-or-three-Availability-Zone placement MSK
/// requires of a cluster's own client subnets. The model declares no `length` on the
/// list, so the count is the service's rule: a list outside this range describes a
/// placement MSK does not admit and is refused with [`SUBNET_COUNT_MESSAGE`], which
/// is what the behavioural record shows for a list of one.
pub const MIN_CLIENT_VPC_SUBNETS: usize = 2;

/// The most that configuration may name, the other end of the same range.
pub const MAX_CLIENT_VPC_SUBNETS: usize = 3;

/// What MSK answers a client VPC configuration naming a number of subnets outside
/// [`MIN_CLIENT_VPC_SUBNETS`]`..=`[`MAX_CLIENT_VPC_SUBNETS`]: fixed text end to end,
/// carrying neither the submitted count, nor the subnet ids, nor the bounds.
pub const SUBNET_COUNT_MESSAGE: &str = "Too many or too few subnet ids specified.";

/// How long after a replication flow and a source topic it covers are both in
/// place the clock owes that topic's appearance on the flow's target cluster.
///
/// CHOSEN, not documented: AWS documents that a matching source topic becomes
/// visible on the target cluster but no propagation latency. The project fixes an
/// emulated clock-owed transition at 250–750 ms, so a caller that lists the target
/// cluster's topics immediately still sees the topic absent while one that waits
/// sees it — the same span, and the same reasoning, as
/// [`TOPIC_ACTIVATION_DELAY`].
pub const TOPIC_REPLICATION_DELAY: Duration = Duration::from_millis(500);

/// The most `TopicsToReplicate` / `TopicsToExclude` patterns one replication flow
/// may carry.
///
/// CHOSEN as a process-safety backstop, not an AWS quota (the model declares both
/// members as `__listOf__stringMax249`, with no `length` trait): the patterns are
/// caller-supplied and every listing of a target cluster matches that cluster's
/// replicated topics against them, so without a ceiling one create would size the
/// work of every later read. Sized far above the handful of patterns a real
/// replication configuration names.
pub const MAX_TOPIC_PATTERNS_PER_FLOW: usize = 100;

/// The longest one topic-selection pattern may be: the `length` the model puts on
/// the member of `TopicsToReplicate` / `TopicsToExclude`
/// (`__listOf__stringMax249`, whose member is `__stringMax249`, 0..249).
///
/// The framework's boundary validator refuses a longer value off the wire, with
/// the service's own constraint wording; this module checks it as well, so no
/// caller of it can hand a pattern of unbounded length to the regex compiler.
pub const MAX_TOPIC_PATTERN_LENGTH: usize = 249;

/// The most memory the matcher compiled from ONE of a flow's pattern lists may
/// take.
///
/// CHOSEN as a process-safety bound, not an AWS quota: the patterns come from a
/// request and the matcher compiled from them is kept on the stored flow, so this
/// is what bounds the memory one create can leave behind. A list is at most
/// [`MAX_TOPIC_PATTERNS_PER_FLOW`] patterns of [`MAX_TOPIC_PATTERN_LENGTH`]
/// characters — about 25 KiB of pattern text — and a matcher that would not fit
/// here makes the request a `BadRequestException` instead of a large allocation.
/// A tenth of the regex crate's own default, and far above what a real
/// replication configuration's handful of patterns compiles to.
pub const TOPIC_MATCHER_SIZE_LIMIT: usize = 1024 * 1024;

/// The most replication flows INTO one cluster a single reader of that cluster's
/// topics derives from.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: a caller can register
/// [`MAX_REPLICATORS_IN_REGISTRY`] replicators of
/// [`MAX_FLOWS_PER_REPLICATOR`] flows each pointing at one cluster, so without a
/// cap on the PRODUCT one small listing would walk every one of them. Sized at the
/// flows a single replicator may carry, which is already far above the one or two
/// a real target cluster is fed by.
pub const MAX_REPLICATION_FLOWS_PER_TARGET: usize = MAX_FLOWS_PER_REPLICATOR;

/// The `ReplicationTopicNameConfigurationType` members, spelled as the model's
/// enum declares them. `PREFIXED_WITH_SOURCE_CLUSTER_ALIAS` names a replicated
/// topic after its source topic prefixed with the source cluster's alias, which is
/// what `KafkaClusterDescription$KafkaClusterAlias` ("Used to prefix names of
/// replicated topics") describes.
pub const TOPIC_NAME_PREFIXED_WITH_SOURCE_CLUSTER_ALIAS: &str =
    "PREFIXED_WITH_SOURCE_CLUSTER_ALIAS";
/// The other member: a replicated topic carries the same name as its source
/// topic, so a flow that asks for it mints no prefix.
pub const TOPIC_NAME_IDENTICAL: &str = "IDENTICAL";

/// What separates the source cluster's alias from the source topic's name in a
/// replicated topic's name, when the flow prefixes names with the alias:
/// `<source-cluster-alias>.<source-topic>`, the form Amazon MSK's replicated
/// topics carry.
pub const REPLICATED_TOPIC_SEPARATOR: char = '.';

/// The most tags one replicator may carry: the general per-resource limit, which
/// [`MAX_TAGS_PER_RESOURCE`] states and every tag write is admitted against. Kept
/// as its own name because `Replicator::created` clamps with it, so the clamp and
/// the admission check cannot drift apart.
pub const MAX_TAGS_PER_REPLICATOR: usize = MAX_TAGS_PER_RESOURCE;

/// Amazon MSK provisions a three-node Apache ZooKeeper ensemble alongside a
/// provisioned cluster, which is what `ZookeeperConnectString` enumerates.
const ZOOKEEPER_NODE_COUNT: i32 = 3;

// Port numbers below are the ones Amazon MSK documents in "Port information for
// Amazon MSK" — the access path a client uses picks the port, which is why
// `GetBootstrapBrokers` returns one string per enabled path.
/// Plaintext client-broker access.
pub const PORT_PLAINTEXT: u16 = 9092;
/// TLS client-broker access.
pub const PORT_TLS: u16 = 9094;
/// SASL/SCRAM client-broker access.
pub const PORT_SASL_SCRAM: u16 = 9096;
/// SASL/IAM client-broker access.
pub const PORT_SASL_IAM: u16 = 9098;
/// TLS access over public (service-provided EIP) connectivity.
pub const PORT_PUBLIC_TLS: u16 = 9194;
/// SASL/SCRAM access over public connectivity.
pub const PORT_PUBLIC_SASL_SCRAM: u16 = 9196;
/// SASL/IAM access over public connectivity.
pub const PORT_PUBLIC_SASL_IAM: u16 = 9198;
/// TLS access over multi-VPC (PrivateLink) connectivity.
pub const PORT_VPC_TLS: u16 = 14094;
/// SASL/SCRAM access over multi-VPC connectivity.
pub const PORT_VPC_SASL_SCRAM: u16 = 14096;
/// SASL/IAM access over multi-VPC connectivity.
pub const PORT_VPC_SASL_IAM: u16 = 14098;
/// Plaintext Apache ZooKeeper access.
const PORT_ZOOKEEPER: u16 = 2181;
/// TLS Apache ZooKeeper access.
const PORT_ZOOKEEPER_TLS: u16 = 2182;

/// The message Amazon MSK answers a parameter it cannot parse with: its
/// `BadRequestException` carries one fixed, request-independent sentence rather
/// than a description of the offending value. The generic one of the service's
/// fixed sentences; a few surfaces answer a more specific one for a cluster ARN
/// ([`CLUSTER_ARN_NOT_VALID_MESSAGE`]).
pub const BAD_REQUEST_MESSAGE: &str = "One or more of the parameters are not valid.";

/// The default for `EncryptionInTransit$ClientBroker`, per that member's model
/// documentation ("The default value is TLS_PLAINTEXT").
pub const DEFAULT_CLIENT_BROKER: &str = "TLS_PLAINTEXT";
/// The default for `EnhancedMonitoring`: the baseline level the enum names.
pub const DEFAULT_ENHANCED_MONITORING: &str = "DEFAULT";
/// The default for `StorageMode`: local broker storage, i.e. no tiered storage.
pub const DEFAULT_STORAGE_MODE: &str = "LOCAL";
/// The default for `ConnectivityInfo$NetworkType`, per the `NetworkType` model
/// documentation ("By default, a cluster uses the IPv4 network type").
pub const DEFAULT_NETWORK_TYPE: &str = "IPV4";

/// The message Amazon MSK answers a request naming a resource it does not hold.
///
/// A CONSTANT of the service, not built from the request: every operation that
/// models `NotFoundException` reports this one wording, which is what the
/// behavioural corpus records the real service answering (the apostrophe is
/// U+2019, as the service writes it). The parameter at fault is named by
/// `NotFoundException$InvalidParameter`, not by the message, so a reader that
/// knows which member it was still says the same thing about the resource.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The requested resource doesn\u{2019}t exist.";

/// The message Amazon MSK answers an update whose `CurrentVersion` is not the
/// version the cluster is actually at.
///
/// A CONSTANT of the service, not built from the request: it carries neither the
/// stored version nor the submitted one, and tells the caller to read the
/// current version back instead (the apostrophe is U+2019, as the service writes
/// it). Shared by every operation of this service that quotes a cluster's
/// version, so one condition gets one wording; the member at fault travels in
/// `BadRequestException$InvalidParameter`.
pub const CLUSTER_VERSION_NOT_CURRENT_MESSAGE: &str =
    "The version of the cluster isn\u{2019}t current. Check the current version and try again.";

/// The member a stale-version refusal names, as the request spells it on the
/// wire (`CurrentVersion`'s `jsonName`).
pub const CURRENT_VERSION_MEMBER: &str = "currentVersion";

/// The wire error code that refusal carries, and the status the model gives the
/// `NotFoundException` shape (`@httpError 404`).
///
/// Named here because an operation whose model declares NO `NotFoundException`
/// still answers one for a reference it cannot resolve — the service answers the
/// code whatever its published model lists — so such an operation carries it
/// through the unmodelled channel
/// ([`crate::scaffolded::Scaffolded::unmodeled`]) with these two values.
pub const NOT_FOUND_CODE: &str = "NotFoundException";
/// The status that refusal carries.
pub const NOT_FOUND_STATUS: u16 = 404;

/// The same refusal, as the replicator APIs phrase it: the input member that
/// failed to resolve, then [`RESOURCE_NOT_FOUND_MESSAGE`] and nothing else.
///
/// `member` is the member's name as the wire carries it (`replicatorArn`), so the
/// text names what the CALLER sent rather than anything about the serving
/// endpoint — which is the form the behavioural corpus records MSK's replicator
/// APIs answering, where the cluster APIs answer the bare sentence. Built here,
/// from one place, so every operation that resolves a replicator by ARN answers
/// one form instead of describing its own state.
pub fn parameter_not_found_message(member: &str) -> String {
    format!("Invalid parameter {member}: {RESOURCE_NOT_FOUND_MESSAGE}")
}

/// The message Amazon MSK answers a request whose value for `member` is not one
/// that member can carry at all — a value it could not have minted and will not
/// read.
///
/// The same `Invalid parameter <member>` framing
/// [`parameter_not_found_message`] puts a detail after, with nothing after it:
/// the member is named, and no part of the offending value is echoed back.
pub fn invalid_parameter_message(member: &str) -> String {
    format!("Invalid parameter {member}.")
}

/// The message Amazon MSK answers a cluster call it refuses because the cluster
/// is not in a state that call is accepted from.
///
/// One fixed sentence per call, in the wording the service uses for that call —
/// they are not one template — with one variable part each: the state the cluster
/// is actually in, read from the stored record, so the caller is told why the
/// call was refused and when to retry it.
pub fn cluster_not_deletable_message(state: &str) -> String {
    format!("You can't delete cluster in {state} state.")
}

/// The same refusal, for the reader of a cluster's bootstrap endpoints: there
/// are no brokers to bootstrap against until the cluster has them. Fixed text
/// apart from the state, in the form the service words this family of refusals
/// in ("You can't … in <STATE> state.", as the delete above).
pub fn bootstrap_brokers_unavailable_message(state: &str) -> String {
    format!("You can't get bootstrap broker nodes for a cluster in {state} state.")
}

/// The same refusal, for the reader of a cluster's broker nodes. The subject is
/// this reader's own and so is its wording — it names "the nodes" and spells the
/// state out as "that is in the <STATE> state" — with the state the cluster is
/// actually in as its one variable part.
pub fn nodes_unavailable_message(state: &str) -> String {
    format!("You can't list the nodes for a cluster that is in the {state} state.")
}

/// The input member every replicator operation resolves its target through, named
/// as the wire carries it — what the refusals of those operations report as the
/// parameter at fault.
pub const REPLICATOR_ARN_MEMBER: &str = "replicatorArn";

/// The `ClusterType` of a cluster whose brokers this emulator provisions — one of
/// the two values the model's `ClusterType` enum names. It is STORED on the record
/// (see [`Cluster::cluster_type`]) rather than assumed by a reader, so a listing's
/// `ClusterTypeFilter` tests what each cluster IS.
pub const CLUSTER_TYPE_PROVISIONED: &str = "PROVISIONED";

/// The `ClusterType` of a cluster that has no customer broker nodes — the model's
/// other `ClusterType` value, which [`Cluster::serverless`] stores on the record
/// together with the [`ServerlessConfig`] the create submitted.
pub const CLUSTER_TYPE_SERVERLESS: &str = "SERVERLESS";

/// What an operation whose subject is the broker nodes answers for a cluster that
/// has none: the broker-node views (`ClusterInfo`, the bootstrap strings' access
/// paths) and the broker-reconfiguring updates all describe a provisioned
/// cluster, so each refuses a serverless one — and they refuse it in the same
/// words, so the service says one thing about one condition.
pub const PROVISIONED_ONLY_MESSAGE: &str =
    "The cluster is a serverless cluster; this operation applies to provisioned clusters only.";

/// The wire error code Amazon MSK answers a Topic Management call against a cluster
/// whose `ClusterType` is [`CLUSTER_TYPE_SERVERLESS`] with. The surface is not
/// offered for that cluster kind at all — topics on such a cluster are created and
/// read over the native Apache Kafka protocol — so the refusal is about the kind of
/// cluster and nothing else about the request.
///
/// This service's model declares no shape for the code, so it leaves through
/// [`crate::scaffolded::Scaffolded::unmodeled`] rather than as a modelled variant: a
/// client branches on the code, so the code the service answers is the code that
/// goes on the wire.
pub const SERVERLESS_TOPIC_APIS_CODE: &str = "ServerlessUnsupportedException";

/// The HTTP status that refusal carries. The call is refused because of what the
/// named cluster IS, which no retry changes and the caller fixes by addressing a
/// provisioned cluster (or the native protocol) instead — a client error, and 4xx
/// so no SDK retry classifier treats it as retryable.
pub const SERVERLESS_TOPIC_APIS_STATUS: u16 = 400;

/// The whole message of that refusal: fixed text end to end, carrying no value from
/// the request or from state.
pub const SERVERLESS_TOPIC_APIS_MESSAGE: &str = "Topic APIs are not supported on serverless clusters.";

/// The refusal every Topic Management operation answers for a serverless cluster.
///
/// Answered immediately after the cluster is resolved and BEFORE any further
/// state-dependent check (a broker-count bound, a topic lookup) and before anything
/// is registered, so the answer depends on the cluster's kind alone and a refused
/// call leaves the registry untouched. One helper, so the service says one thing
/// about one condition on every one of those operations.
pub fn serverless_topic_apis_unsupported<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::unmodeled(
        SERVERLESS_TOPIC_APIS_CODE,
        SERVERLESS_TOPIC_APIS_STATUS,
        SERVERLESS_TOPIC_APIS_MESSAGE,
    )
}

/// The one `NodeType` the model names: `ListNodes` reports a cluster's broker
/// nodes, so every node it lists is of this type.
pub const NODE_TYPE_BROKER: &str = "BROKER";

/// The largest page a listing may report, from the `range` on the `MaxResults`
/// shape (1..100) that this service's paginated listings bind. Also the page
/// size used when a request names none.
pub const MAX_RESULTS_LIMIT: i32 = 100;

/// Marker that prefixes a cluster page token's plaintext, so a token minted for
/// some other list cannot be mistaken for one of these.
const CLUSTER_PAGE_TOKEN_PREFIX: &str = "kafka:clusters:";

/// One broker node of a provisioned cluster: the DNS name clients reach it on,
/// and the placement the service gave it when it joined the cluster.
/// `GetBootstrapBrokers` renders every bootstrap string from these hosts, and
/// `ListNodes` reports each node through [`Cluster::to_node_info`].
#[derive(Debug, Clone)]
pub struct BrokerNode {
    /// 1-based broker id, as it appears in the `b-<id>` DNS label.
    pub id: i32,
    /// `b-<id>.<cluster>.<suffix>.kafka.<region>.amazonaws.com` — the port is
    /// appended per access path.
    pub endpoint_host: String,
    /// The one subnet of `BrokerNodeGroupInfo$ClientSubnets` this broker's
    /// network interface sits in. Stored rather than recomputed per read, so the
    /// placement a node was given does not move when the cluster's
    /// configuration is read again.
    pub client_subnet: String,
    /// The id of the elastic network interface the service created for this
    /// broker in [`Self::client_subnet`] — `ClientSubnets`' documentation is
    /// what says one is created per node.
    pub attached_eni_id: String,
    /// The private IPv4 address that interface answers on inside the client VPC.
    pub client_vpc_ip_address: String,
    /// When the node joined the cluster: the creation instant for a broker the
    /// create laid out, so a node added later carries its own.
    pub added_to_cluster_time: SystemTime,
}

impl BrokerNode {
    /// Lay out broker `id` of a cluster whose endpoints sit under `dns_suffix`:
    /// its endpoint host, the client subnet it is placed in, the network
    /// interface created for it there, and the instant it joined.
    ///
    /// `client_subnets` is the cluster's own `BrokerNodeGroupInfo$ClientSubnets`
    /// and the brokers are spread over it in order, which is what makes an
    /// `MSK` cluster's nodes sit across the Availability Zones those subnets
    /// name. The ids are pure functions of the node's endpoint host — unique per
    /// cluster and per broker — so a node keeps them for as long as the record
    /// holds it.
    fn placed(
        region: &str,
        name: &str,
        dns_suffix: &str,
        id: i32,
        client_subnets: &[String],
        now: SystemTime,
    ) -> Self {
        let endpoint_host = format!("b-{id}.{name}.{dns_suffix}.kafka.{region}.amazonaws.com");
        // The brokers are placed round-robin over the submitted subnets, so
        // broker 1 lands in the first. A cluster whose stored group names none
        // (which the model's `required` `ClientSubnets` does not allow a create
        // to submit) places the node nowhere rather than inventing a subnet.
        let client_subnet = client_subnets
            .get(usize::try_from(id - 1).unwrap_or(0) % client_subnets.len().max(1))
            .cloned()
            .unwrap_or_default();
        Self {
            attached_eni_id: ec2_long_id("eni", &format!("{endpoint_host}#eni")),
            client_vpc_ip_address: private_ipv4(&format!("{endpoint_host}#ip")),
            client_subnet,
            endpoint_host,
            id,
            added_to_cluster_time: now,
        }
    }
}

/// A provisioned MSK cluster, holding the whole submitted configuration so every
/// reader can be computed from it rather than echoed from a request.
#[derive(Debug, Clone)]
pub struct Cluster {
    pub arn: String,
    pub name: String,
    /// The region the cluster lives in: the signing region of the request that
    /// created it, and the region segment of its ARN and endpoints. A cluster name
    /// is unique per account and region, so this is half of its registry key.
    pub region: String,
    /// A `ClusterType` value — which of the model's two cluster kinds this record
    /// is, so a listing's `ClusterTypeFilter` and the `Cluster` shape's
    /// `clusterType` both read it off the record instead of assuming one.
    pub cluster_type: String,
    pub kafka_version: String,
    /// The `CurrentVersion` token an update call must quote, derived from
    /// [`Self::revision`] so it changes exactly when the cluster does.
    pub current_version: String,
    pub creation_time: SystemTime,
    /// The instant the clock owes this cluster its `CREATING` -> `ACTIVE`
    /// transition. Stored, never slept on.
    pub active_due: SystemTime,
    /// A `ClusterState` value.
    pub state: String,
    pub number_of_broker_nodes: i32,
    pub broker_node_group_info: BrokerNodeGroupInfo,
    pub client_authentication: Option<ClientAuthentication>,
    /// Always stored with its defaults resolved, so every reader reports the
    /// effective settings the cluster actually runs with.
    pub encryption_info: EncryptionInfo,
    pub enhanced_monitoring: String,
    pub open_monitoring: OpenMonitoringInfo,
    pub logging_info: Option<LoggingInfo>,
    pub storage_mode: String,
    pub configuration_info: Option<ConfigurationInfo>,
    pub rebalancing: Option<Rebalancing>,
    /// Ordered so `DescribeCluster` renders a stable map.
    pub tags: BTreeMap<String, String>,
    pub zookeeper_connect_string: String,
    pub zookeeper_connect_string_tls: String,
    /// The opaque cluster id plus cell every endpoint of this cluster sits
    /// under, as [`cluster_identity`] derived it from the cluster's ordinal:
    /// `b-1.<name>.<dns suffix>.kafka.<region>.amazonaws.com`. STORED, because
    /// a broker node added to the cluster after its creation has to be given an
    /// endpoint under the same suffix as the nodes already there.
    pub dns_suffix: String,
    pub brokers: Vec<BrokerNode>,
    /// The ARNs of this cluster's recorded operations, oldest first. It is the
    /// cluster's own index into the operation registry, which is what keeps the
    /// history bound a walk of ONE cluster's operations rather than of every
    /// operation the registry holds.
    pub operation_arns: Vec<String>,
    /// Whether Apache ZooKeeper access is turned on, as
    /// `UpdateConnectivity`'s `ZookeeperAccess` leaves it. A cluster is
    /// provisioned with access on. The model reports this setting through
    /// `MutableClusterInfo$ZookeeperAccess`, i.e. through the cluster-operation
    /// readers, so it is stored here rather than rendered by `ClusterInfo`,
    /// which models no member for it.
    pub zookeeper_access_enabled: bool,
    /// Which revision of its configuration the cluster is at.
    /// [`Cluster::current_version`] is this number's token, so the version
    /// changes exactly when a completed update changes the cluster.
    pub revision: u64,
    /// The update this cluster has in flight, if any: the one cluster operation
    /// acting on it, and the change the clock owes it. `Some` is exactly what
    /// makes the cluster `UPDATING` with an active operation ARN.
    pub pending_update: Option<PendingUpdate>,
    /// How many reboot operations have been filed on this cluster.
    ///
    /// A reboot generates no new cluster version
    /// ([`ClusterChange::generates_new_version`]), so [`Self::revision`] cannot
    /// identify the operation one files: this counter is what a reboot operation's
    /// ARN is minted from, so two reboots of one cluster name two operations and
    /// neither can collide with an update's.
    pub reboots: u64,
    /// The serverless configuration, for a record whose [`Self::cluster_type`] is
    /// [`CLUSTER_TYPE_SERVERLESS`]; `None` for a provisioned cluster. It is what
    /// the `Cluster` shape's `Serverless` member is derived from, so the per-kind
    /// detail a reader reports comes off the record rather than from the kind's
    /// name.
    pub serverless: Option<ServerlessConfig>,
    /// The resource-based policy attached to this cluster, or `None` while it
    /// has none. A policy is a sub-resource of exactly one cluster — the model
    /// addresses it as `/v1/clusters/{ClusterArn}/policy` — so it is stored on
    /// the cluster record and disappears when the cluster does.
    pub policy: Option<ClusterPolicy>,
    /// The AWS Secrets Manager secrets this cluster authenticates its SASL/SCRAM
    /// clients with, as `BatchAssociateScramSecret` associated them.
    ///
    /// A SET, so associating a secret the cluster already holds changes nothing
    /// and `ListScramSecrets` reports each secret exactly once; `BTreeSet` so
    /// that listing has one total order to page through, the way the cluster
    /// registry gives `ListClusters` one. Addressed as
    /// `/v1/clusters/{ClusterArn}/scram-secrets`, hence stored on the cluster
    /// record: the secrets leave with the cluster they authenticate clients to.
    pub scram_secret_arns: BTreeSet<String>,
}

/// The resource-based policy a cluster carries: the document a caller attached
/// and which revision of the policy it is, so the `CurrentVersion` every reader
/// reports is derived from the stored policy rather than echoed from a request.
#[derive(Debug, Clone)]
pub struct ClusterPolicy {
    /// The policy document exactly as submitted — the service stores a policy,
    /// it does not rewrite one.
    pub document: String,
    /// Which revision of this cluster's policy the stored document is: 1 for the
    /// first one attached, the next for each one that replaces it.
    pub revision: u64,
    /// The `CurrentVersion` token of [`Self::revision`], minted the same way a
    /// cluster's version token is but over the policy's own address, so a policy
    /// version and a cluster version are never the same token and the policy's
    /// changes exactly when the document does.
    pub current_version: String,
}

/// The configuration of a serverless cluster: the VPCs its clients reach it
/// through, the client authentication it accepts, and the one endpoint they
/// bootstrap on. Held beside the cluster record for the same reason the
/// provisioned configuration is: every reader derives its answer from this one
/// stored copy.
#[derive(Debug, Clone)]
pub struct ServerlessConfig {
    /// The `VpcConfigs` the create submitted, in the order it listed them.
    pub vpc_configs: Vec<VpcConfig>,
    /// The `ServerlessClientAuthentication` the create submitted, if any.
    pub client_authentication: Option<ServerlessClientAuthentication>,
    /// The single host a client bootstraps this cluster through:
    /// `boot-<id>.kafka-serverless.<region>.amazonaws.com`, the form Amazon MSK
    /// Serverless publishes. Reached on [`PORT_SASL_IAM`], since IAM access
    /// control is the only client authentication MSK Serverless offers, which is
    /// why the serverless arm of `GetBootstrapBrokers` reports one string.
    pub bootstrap_host: String,
}

impl ServerlessConfig {
    /// The configuration as the `Serverless` shape the V2 readers report.
    pub fn to_serverless(&self) -> Serverless {
        Serverless {
            vpc_configs: self.vpc_configs.clone(),
            client_authentication: self.client_authentication.clone(),
            // `ServerlessRequest` models no connectivity member, so a serverless
            // cluster runs the documented default network type and the reader
            // reports that effective setting rather than leaving the caller to
            // guess which default applied.
            connectivity_info: Some(ServerlessConnectivityInfo {
                network_type: Some(DEFAULT_NETWORK_TYPE.to_string()),
            }),
        }
    }
}

/// A cluster operation in flight: the change a caller asked for, the ARN of the
/// operation that will apply it, and the instant the clock owes its completion.
/// Stored, never slept on — [`Cluster::settle`] materialises it.
#[derive(Debug, Clone)]
pub struct PendingUpdate {
    /// The active operation's ARN while the operation runs — reported as
    /// `ClusterInfo$ActiveOperationArn` and as `Cluster$ActiveOperationArn` —
    /// and the `ClusterOperationArn` the update call answered with.
    pub operation_arn: String,
    /// When the operation is due to finish.
    pub due: SystemTime,
    /// What it changes.
    pub change: ClusterChange,
}

/// The change one cluster operation applies. Each update operation this service
/// implements contributes the variant describing what it changes, so a cluster
/// holds at most one in-flight change whatever produced it.
#[derive(Debug, Clone)]
pub enum ClusterChange {
    /// `UpdateConnectivity`: the broker access configuration, the ZooKeeper
    /// access setting, or both. `None` means the request left that part of the
    /// cluster alone.
    Connectivity {
        connectivity_info: Option<ConnectivityInfo>,
        zookeeper_access: Option<ZookeeperAccess>,
    },
    /// `UpdateStorage` and `UpdateBrokerStorage`: the brokers' EBS volume size,
    /// their provisioned throughput, the cluster's storage mode, or any
    /// combination the request named.
    Storage(StorageChange),
    /// `UpdateBrokerType`: the EC2 instance type every broker of the cluster
    /// runs on.
    BrokerType { instance_type: String },
    /// `UpdateBrokerCount`: how many broker nodes the cluster is to hold once
    /// the operation completes. The request always names one, so there is
    /// nothing optional about this change.
    BrokerCount { number_of_broker_nodes: i32 },
    /// `UpdateClusterKafkaVersion`: the Apache Kafka version the brokers run,
    /// and the custom configuration to apply on that new version when the
    /// request named one — `None` leaves the cluster's configuration as it is.
    KafkaVersion {
        kafka_version: String,
        configuration_info: Option<ConfigurationInfo>,
    },
    /// `UpdateClusterConfiguration`: the MSK configuration revision the brokers
    /// run their Apache Kafka version with. The request always names one, so
    /// there is nothing optional about this change.
    Configuration { configuration_info: ConfigurationInfo },
    /// `UpdateSecurity`: the client authentication the cluster accepts, the
    /// encryption it runs with, or both. `None` means the request left that
    /// setting alone.
    Security {
        client_authentication: Option<ClientAuthentication>,
        encryption_info: Option<EncryptionInfo>,
    },
    /// `UpdateMonitoring`: the CloudWatch metrics level the cluster gathers,
    /// the open-monitoring (Prometheus) exporters it runs, the broker log
    /// delivery it performs, or any combination the request named. `None` means
    /// the request left that setting alone.
    Monitoring {
        enhanced_monitoring: Option<String>,
        open_monitoring: Option<OpenMonitoringInfo>,
        logging_info: Option<LoggingInfo>,
    },
    /// `UpdateRebalancing`: the intelligent-rebalancing status the cluster runs
    /// its partition balancing under. The request always names one, so there is
    /// nothing optional about this change.
    Rebalancing { rebalancing: Rebalancing },
    /// `RebootBroker`: the broker nodes the request named are restarted. It
    /// carries no attribute, because a reboot changes none: the nodes come back
    /// under the same placement, endpoints and software, and this API reports no
    /// per-node status for a reader to see one of them down — so what the reboot
    /// leaves behind is the cluster's state while it runs
    /// ([`Self::in_flight_state`]) and the operation record filed for it.
    BrokerReboot,
}

impl ClusterChange {
    /// The `ClusterState` a cluster is in WHILE this change is applied: a cluster
    /// whose brokers are being rebooted is `REBOOTING_BROKER`, and one an accepted
    /// update is being applied to is `UPDATING`.
    ///
    /// One function of the change itself, so the state a cluster operation puts its
    /// cluster into and the state its completion is materialised from
    /// ([`Cluster::settle`]) cannot drift apart. Matched variant by variant rather
    /// than through a catch-all, so a change added later has to say which state it
    /// puts its cluster in instead of inheriting `UPDATING` silently.
    pub fn in_flight_state(&self) -> &'static str {
        match self {
            Self::BrokerReboot => CLUSTER_STATE_REBOOTING_BROKER,
            Self::Connectivity { .. }
            | Self::Storage(_)
            | Self::BrokerType { .. }
            | Self::BrokerCount { .. }
            | Self::KafkaVersion { .. }
            | Self::Configuration { .. }
            | Self::Security { .. }
            | Self::Rebalancing { .. }
            | Self::Monitoring { .. } => CLUSTER_STATE_UPDATING,
        }
    }

    /// How long a cluster operation carrying this change stays in flight: the
    /// instant the clock owes its completion is this far after the call that
    /// started it.
    ///
    /// One function of the change itself, so the operation record and the
    /// cluster's own pending change cannot disagree about when the work finished.
    /// Matched variant by variant for the reason [`Self::in_flight_state`] is: a
    /// restart of a broker node is a different piece of work from a
    /// reconfiguration and is observable for its own span
    /// ([`BROKER_REBOOT_DELAY`]), and a change added later has to say which span
    /// it takes.
    pub fn settling_delay(&self) -> Duration {
        match self {
            Self::BrokerReboot => BROKER_REBOOT_DELAY,
            Self::Connectivity { .. }
            | Self::Storage(_)
            | Self::BrokerType { .. }
            | Self::BrokerCount { .. }
            | Self::KafkaVersion { .. }
            | Self::Configuration { .. }
            | Self::Security { .. }
            | Self::Rebalancing { .. }
            | Self::Monitoring { .. } => CLUSTER_UPDATE_DELAY,
        }
    }

    /// Whether completing this change generates a new cluster version.
    ///
    /// "When this update operation is successful, it generates a new cluster
    /// version" is what every update request's `CurrentVersion` documents, and each
    /// of those operations quotes the version it updates from. `RebootBroker`
    /// quotes none — its request carries no `CurrentVersion` member at all — and it
    /// changes no cluster attribute, so a rebooted cluster is still at the version
    /// it was.
    ///
    /// Decided the same way, variant by variant, for the same reason.
    pub fn generates_new_version(&self) -> bool {
        match self {
            Self::BrokerReboot => false,
            Self::Connectivity { .. }
            | Self::Storage(_)
            | Self::BrokerType { .. }
            | Self::BrokerCount { .. }
            | Self::KafkaVersion { .. }
            | Self::Configuration { .. }
            | Self::Security { .. }
            | Self::Rebalancing { .. }
            | Self::Monitoring { .. } => true,
        }
    }
}

/// The storage attributes one update call asked to change. A member left `None`
/// is one the request did not name, which the update leaves as it is.
#[derive(Debug, Clone, Default)]
pub struct StorageChange {
    /// The EBS data-volume size each broker is to be given, in GiB.
    pub volume_size_gb: Option<i32>,
    /// The EBS provisioned-throughput setting the volumes are to run with.
    pub provisioned_throughput: Option<ProvisionedThroughput>,
    /// The `StorageMode` the cluster is to run in.
    pub storage_mode: Option<String>,
}

impl StorageChange {
    /// Whether the request named no storage attribute at all, so there is
    /// nothing for an update to do.
    pub fn is_empty(&self) -> bool {
        self.volume_size_gb.is_none()
            && self.provisioned_throughput.is_none()
            && self.storage_mode.is_none()
    }
}

/// One cluster operation: the record an update call leaves behind, which is what
/// its `ClusterOperationArn` names.
#[derive(Debug, Clone)]
pub struct ClusterOperation {
    pub arn: String,
    pub cluster_arn: String,
    /// The region the operation ran in — the signing region of the call that
    /// started it, and the region segment of its ARN.
    pub region: String,
    /// A `ClusterType` value: which kind of cluster the operation acted on,
    /// taken off that cluster's record when the operation was filed. The V2
    /// reader reports it as `ClusterOperationV2$ClusterType` and derives the
    /// per-kind detail arm from it, so the kind is stored rather than assumed
    /// from what filed the operation.
    pub cluster_type: String,
    /// A `ClusterOperationInfo$OperationType` value.
    pub operation_type: String,
    pub creation_time: SystemTime,
    /// The instant the clock owes this operation its completion. STORED, never
    /// slept on: the reported state is a function of it and the reading instant.
    pub end_due: SystemTime,
    /// The attributes this operation changes, as they were before it ran.
    pub source_cluster_info: MutableClusterInfo,
    /// The same attributes, as the operation sets them.
    pub target_cluster_info: MutableClusterInfo,
}

impl ClusterOperation {
    /// The operation's state at `now`: the pair of states its own type is
    /// reported in ([`operation_states_of`]), chosen by whether the clock has
    /// reached the instant this operation was due.
    pub fn operation_state(&self, now: SystemTime) -> &'static str {
        let (in_progress, complete) = operation_states_of(&self.operation_type);
        if now >= self.end_due {
            complete
        } else {
            in_progress
        }
    }

    /// The instant this operation was filed, in whole milliseconds since the
    /// Unix epoch — the granularity a listing's pagination token can carry, so
    /// it is also the granularity [`Self::history_key`] orders by.
    pub fn creation_millis(&self) -> u128 {
        self.creation_time
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis())
    }

    /// The key a cluster's operation history is ordered by: the instant the
    /// operation was filed, latest first, with its own ARN breaking a tie so
    /// the order is TOTAL.
    ///
    /// A listing resumes by comparing this key against the one its page token
    /// carries, so the order the page is built in and the order the token
    /// resumes in are the same key rather than two that can drift apart.
    pub fn history_key(&self) -> (std::cmp::Reverse<u128>, &str) {
        (std::cmp::Reverse(self.creation_millis()), self.arn.as_str())
    }

    /// The operation as `ClusterOperationInfo`, the shape
    /// `DescribeClusterOperation` reports.
    ///
    /// `EndTime` is reported only once the operation has finished, since an
    /// operation still running has not ended.
    pub fn to_cluster_operation_info(&self, now: SystemTime) -> ClusterOperationInfo {
        ClusterOperationInfo {
            // The id of the API request that triggered the operation is the
            // caller's, and this emulator mints none.
            client_request_id: None,
            cluster_arn: Some(self.cluster_arn.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            end_time: (now >= self.end_due)
                .then(|| ::aws_smithy_types::DateTime::from(self.end_due)),
            // Nothing fails an emulated update, so there is no error to describe.
            error_info: None,
            operation_arn: Some(self.arn.clone()),
            operation_state: Some(self.operation_state(now).to_string()),
            // No step list: this emulator runs no broker-by-broker steps, and
            // reporting invented ones would describe work nothing did.
            operation_steps: None,
            operation_type: Some(self.operation_type.clone()),
            source_cluster_info: Some(self.source_cluster_info.clone()),
            target_cluster_info: Some(self.target_cluster_info.clone()),
            // A VPC connection is described only by the operations that manage
            // one.
            vpc_connection_info: None,
        }
    }

    /// The same stored operation as `ClusterOperationV2`, the shape
    /// `DescribeClusterOperationV2` reports: the members every cluster kind
    /// shares, plus the one per-kind detail arm this operation's
    /// [`Self::cluster_type`] names.
    ///
    /// Derived from the SAME record [`Self::to_cluster_operation_info`] renders
    /// and at the same reading instant, so the V1 and V2 views of one operation
    /// cannot disagree about when it started, what it changes or how far along
    /// it is.
    pub fn to_cluster_operation_v2(&self, now: SystemTime) -> ClusterOperationV2 {
        ClusterOperationV2 {
            cluster_arn: Some(self.cluster_arn.clone()),
            cluster_type: Some(self.cluster_type.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            end_time: (now >= self.end_due)
                .then(|| ::aws_smithy_types::DateTime::from(self.end_due)),
            // Nothing fails an emulated update, so there is no error to describe.
            error_info: None,
            operation_arn: Some(self.arn.clone()),
            operation_state: Some(self.operation_state(now).to_string()),
            operation_type: Some(self.operation_type.clone()),
            provisioned: (self.cluster_type == CLUSTER_TYPE_PROVISIONED).then(|| {
                ClusterOperationV2Provisioned {
                    // No step list, for the reason the V1 view reports none:
                    // this emulator runs no broker-by-broker steps.
                    operation_steps: None,
                    source_cluster_info: Some(self.source_cluster_info.clone()),
                    target_cluster_info: Some(self.target_cluster_info.clone()),
                    // A VPC connection is described only by the operations that
                    // manage one.
                    vpc_connection_info: None,
                }
            }),
            // The serverless arm reports a serverless cluster's connectivity
            // before and after the operation, and every operation this registry
            // holds was filed by an update that a serverless cluster is refused
            // on, so no stored operation has that detail to report.
            serverless: None,
        }
    }

    /// The same stored operation as `ClusterOperationV2Summary`, the shape
    /// `ListClusterOperationsV2` pages.
    ///
    /// The summary is the V2 view WITHOUT its per-kind detail arm: it carries
    /// exactly the members every cluster kind shares, so it is derived from the
    /// full V2 rendering at the same reading instant rather than computed a
    /// second way, and a listed operation cannot disagree with the by-ARN
    /// reader of the same ARN about any member both report.
    pub fn to_cluster_operation_v2_summary(&self, now: SystemTime) -> ClusterOperationV2Summary {
        let full = self.to_cluster_operation_v2(now);
        ClusterOperationV2Summary {
            cluster_arn: full.cluster_arn,
            cluster_type: full.cluster_type,
            start_time: full.start_time,
            end_time: full.end_time,
            operation_arn: full.operation_arn,
            operation_state: full.operation_state,
            operation_type: full.operation_type,
        }
    }
}

/// A stored MSK configuration (a `server.properties` bundle a cluster can be
/// created with), holding every property its readers report so each one is
/// computed from the record rather than echoed from a request.
///
/// A configuration is revisioned: `CreateConfiguration` stores revision 1 and an
/// update appends the next one, so the record keeps the revisions rather than a
/// single body.
#[derive(Debug, Clone)]
pub struct ConfigurationRecord {
    pub arn: String,
    pub name: String,
    /// The region the configuration lives in: the signing region of the request
    /// that created it, and the region segment of its ARN. A configuration name
    /// is unique per account and region, so this is half of its registry key.
    pub region: String,
    /// The description this configuration has, or `None` when it has none — the
    /// request either omitted `Description` or carried an empty one, which is
    /// the same thing ([`stored_description`]). Every reader whose own
    /// `Description` member is optional reports no description by OMITTING the
    /// member.
    pub description: Option<String>,
    /// The Apache Kafka versions the configuration may be used with, as
    /// submitted; empty when the request named none.
    pub kafka_versions: Vec<String>,
    pub creation_time: SystemTime,
    /// A `ConfigurationState` value. A configuration AWS accepts is usable at
    /// once, so it is created `ACTIVE`; a delete moves it to `DELETING` through
    /// [`Self::begin_deletion`].
    pub state: String,
    /// Set by `DeleteConfiguration`: the instant the clock owes this
    /// configuration's removal from the registry. `None` for a configuration
    /// nobody has deleted. Stored, never slept on.
    pub removal_due: Option<SystemTime>,
    /// Every revision in revision order, so the first is revision 1 and the last
    /// is the latest. Never empty: a configuration exists only once its first
    /// revision has been stored.
    pub revisions: Vec<ConfigurationRevisionRecord>,
}

/// One revision of a stored configuration: the `server.properties` body plus the
/// description and instant that went with it.
#[derive(Debug, Clone)]
pub struct ConfigurationRevisionRecord {
    /// 1-based revision number, as `ConfigurationRevision$Revision` reports it.
    pub revision: i64,
    pub creation_time: SystemTime,
    /// The description the call that stored this revision gave it, or `None`
    /// when it gave none ([`stored_description`]) —
    /// `ConfigurationRevision$Description` is optional, so a revision with no
    /// description is reported by omitting the member.
    pub description: Option<String>,
    /// The `server.properties` contents exactly as submitted, kept as bytes
    /// because the member is a blob and may hold any byte string.
    pub server_properties: Vec<u8>,
}

/// The state a configuration this emulator accepts is in, from the
/// `ConfigurationState` enum (`ACTIVE | DELETING | DELETE_FAILED`): AWS returns a
/// freshly created configuration ready to use, so it starts `ACTIVE` and the
/// clock owes it no transition.
pub const CONFIGURATION_STATE_ACTIVE: &str = "ACTIVE";

/// The `ConfigurationState` a configuration a delete accepted is reported in,
/// the same way a deleted cluster is reported `DELETING`. The delete is
/// asynchronous, so this state is STORED on the record
/// ([`ConfigurationRecord::begin_deletion`]) and reported by every reader until
/// the clock owes the removal: a configuration on its way out is still a
/// resource of this endpoint, not an unresolvable reference.
pub const CONFIGURATION_STATE_DELETING: &str = "DELETING";

/// How long after `DeleteConfiguration` a configuration marked `DELETING` is
/// actually gone from the registry.
///
/// CHOSEN, not documented: Amazon MSK answers the delete with the non-terminal
/// `DELETING` state and completes the removal on its own, which takes as long as
/// it takes. The span is never zero, so a caller that retries its update right
/// after the delete still meets the configuration in `DELETING` — and short
/// enough that a caller polling the deletion to completion sees it leave the
/// registry.
pub const CONFIGURATION_DELETION_DELAY: Duration = Duration::from_secs(3);

/// The description a configuration or one of its revisions stores for the
/// `Description` a request submitted: the value where the request carried one,
/// and NONE where it omitted the member or carried an empty string.
///
/// An empty description is no description: Amazon MSK describes a configuration
/// created with `Description: ""` back with the member ABSENT, exactly as it
/// describes one created without the member at all, so the two requests store
/// the same thing and no reader has to decide between them. Applied on the two
/// paths that store a description (a create and an append), so the record holds
/// one normalised value whichever call wrote it.
pub fn stored_description(submitted: Option<String>) -> Option<String> {
    submitted.filter(|description| !description.is_empty())
}

/// The `ConfigurationState` values a call that CHANGES a configuration is
/// accepted from: a configuration is only ever changed while it is `ACTIVE`,
/// since the other two states (`DELETING`, `DELETE_FAILED`) are a configuration
/// the service is taking out. Named here so the update and the delete gate on
/// one set and report the one refusal
/// ([`configuration_not_in_accepted_state`]) built from it.
pub const CONFIGURATION_STATES_A_CHANGE_IS_ACCEPTED_FROM: [&str; 1] =
    [CONFIGURATION_STATE_ACTIVE];

/// How Amazon MSK refuses a call against a resource whose current state that
/// call is not accepted from: one fixed sentence that names the set of states it
/// IS accepted from, which is what tells such a refusal apart from a reference
/// the service cannot resolve at all
/// ([`CONFIGURATION_ARN_NOT_FOUND_MESSAGE`]).
///
/// `accepted` is that set, in the spelling the `ConfigurationState` enum
/// declares — so the sentence is built from the gate the handler applied rather
/// than written out beside it, and a gate that admits more than one state says
/// so.
pub fn configuration_not_in_accepted_state(accepted: &[&str]) -> BadRequestException {
    BadRequestException {
        // The configuration the `Arn` names is what the call was refused on, as
        // the cluster refusals name the cluster's own ARN member.
        invalid_parameter: Some("Arn".to_string()),
        // The one sentence the service words this condition in, whichever
        // resource family met it ([`state_gate`]).
        message: Some(state_gate::not_in_accepted_state_message(accepted)),
    }
}

/// What Amazon MSK answers any configuration operation whose configuration
/// reference — its `Arn`, or that `Arn` together with the `Revision` of it being
/// named — resolves to nothing it holds.
///
/// A CONSTANT of the service, not built from the request: every operation of the
/// configuration family reports a reference it cannot resolve as a bad request
/// with this one fixed sentence — not as the `NotFoundException` the other
/// resource families answer — which is what the behavioural corpus records the
/// real service answering, for a configuration that was never minted or already
/// deleted and for a revision number of an existing configuration that was never
/// issued alike. The parameter at fault is named by
/// `BadRequestException$InvalidParameter`, not by the message.
pub const CONFIGURATION_ARN_NOT_FOUND_MESSAGE: &str = "Configuration ARN does not exist.";

/// The refusal every configuration operation answers a configuration reference
/// that resolves to nothing this endpoint's region holds with: a
/// `BadRequestException` (HTTP 400) carrying
/// [`CONFIGURATION_ARN_NOT_FOUND_MESSAGE`] and naming `arn` as the parameter at
/// fault. The reference is the `Arn` alone where the operation names only a
/// configuration, and the `Arn` plus the `Revision` where it names one revision
/// of one, since a revision number the configuration never issued resolves to a
/// revision just as little as an absent configuration resolves to a
/// configuration.
///
/// Built HERE rather than in each handler so the readers, the update, the delete
/// and the cluster updates that apply a configuration cannot drift apart on how
/// an unresolvable configuration reference is reported: one condition, one
/// envelope, whichever operation met it.
pub fn configuration_arn_not_found() -> BadRequestException {
    BadRequestException {
        // `InvalidParameter` reports the parameter as the API names it on the
        // wire — the configuration operations bind their `Arn` label as `arn` —
        // not as the Smithy structure member is spelled.
        invalid_parameter: Some("arn".to_string()),
        message: Some(CONFIGURATION_ARN_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// What Amazon MSK answers a create whose name is already taken: ONE fixed
/// sentence for every resource the service names by name, disclosing neither the
/// name submitted nor the resource already holding it.
///
/// A CONSTANT of the service, not built from the request: the conflict is about
/// the name being taken, and the caller already knows which name it sent, so
/// nothing of the request or of the stored resource reaches the text. The
/// request member at fault is named by `ConflictException$InvalidParameter`
/// instead.
pub const NAME_ALREADY_EXISTS_MESSAGE: &str = "A resource with this name already exists.";

/// The refusal a create answers a name another resource of its kind already
/// holds with: a `ConflictException` (HTTP 409) carrying
/// [`NAME_ALREADY_EXISTS_MESSAGE`] and naming, in `invalidParameter`, the
/// request member that carried the name — by its wire name, which is what the
/// API documents and what the caller's own request holds.
///
/// Built HERE rather than in each create handler because the sentence is the
/// service's, not the resource type's: two creates refusing a taken name must
/// not answer differently.
pub fn name_already_exists(parameter: &str) -> ConflictException {
    ConflictException {
        invalid_parameter: Some(parameter.to_string()),
        message: Some(NAME_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// How this emulator refuses a delete of a configuration a cluster still uses:
/// one fixed sentence about the condition, in the same request-independent form
/// as [`RESOURCE_NOT_FOUND_MESSAGE`] (the apostrophe is U+2019, as the service
/// writes it). UNVERIFIED wording — no recording of this path was available, so
/// unlike [`CONFIGURATION_ARN_NOT_FOUND_MESSAGE`] this is the condition stated
/// in the service's style rather than text observed from Amazon MSK, and it is
/// held here, in one place, to be reconciled against the real sentence once a
/// recording of it exists. Which clusters hold the configuration is a listing
/// the caller gets from the cluster readers, so the refusal describes the
/// condition rather than this endpoint's registry.
pub const CONFIGURATION_IN_USE_MESSAGE: &str =
    "The configuration is in use by one or more clusters and can\u{2019}t be deleted.";

/// Configurations one account may hold in one region, from the Amazon MSK
/// service quota "Number of configurations per account" (100, per Region). A
/// create past it is refused with the modelled `TooManyRequestsException`, so the
/// registry's cardinality is bounded rather than growing with the number of
/// calls.
pub const MAX_CONFIGURATIONS_PER_REGION: usize = 100;

/// Absolute ceiling on the configuration registry across every region a caller
/// signs for, for the same reason [`MAX_CLUSTERS_IN_REGISTRY`] exists: the
/// signing region comes from the request, so the per-region quota alone would
/// still let a caller grow the store by naming new regions.
pub const MAX_CONFIGURATIONS_IN_REGISTRY: usize = MAX_CONFIGURATIONS_PER_REGION * 64;

/// How many revisions this emulator keeps for one configuration.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: every update call
/// appends a revision holding the body the request submitted, so an unbounded
/// history would grow the store with the number of calls. Sized well past any
/// realistic client script's update count, and enforced by refusing the append
/// rather than evicting a revision, because a stored revision is immutable: a
/// reader of an earlier revision must still see the body it was stored with.
pub const MAX_REVISIONS_PER_CONFIGURATION: usize = 100;

/// How many bytes of `server.properties` one stored configuration revision may
/// hold.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: the modelled member is
/// a blob with no length constraint, so without a ceiling a single call could
/// store an arbitrarily large body. Sized far past any realistic
/// `server.properties` file, which holds at most a few hundred short properties.
pub const MAX_SERVER_PROPERTIES_BYTES: usize = 256 * 1024;

/// Whether a submitted `server.properties` body is longer than this emulator
/// stores for one revision. The one test both record paths that store a body
/// apply — [`ConfigurationRecord::created`] for revision 1 and
/// [`ConfigurationRecord::append_revision`] for every later one — so a stored body
/// is bounded whichever call stored it, and a handler cannot bypass the ceiling by
/// minting a record instead of appending to one.
pub fn server_properties_too_large(body: &[u8]) -> bool {
    body.len() > MAX_SERVER_PROPERTIES_BYTES
}

/// How many bytes of policy document one cluster's resource-based policy may
/// hold.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: the modelled member is
/// a string with no length constraint, so without a ceiling one call could store
/// an arbitrarily large document. Sized far past any realistic resource-based
/// policy, which holds at most a few dozen short statements.
pub const MAX_CLUSTER_POLICY_BYTES: usize = 256 * 1024;

/// How many SASL/SCRAM secrets one cluster may hold in this emulator.
///
/// CHOSEN as a process-safety backstop, not an AWS quota: the secret ARNs come
/// from the request and the modelled list carries no length constraint, so
/// without a ceiling a caller could grow one cluster's secret set one call at a
/// time. Sized well above any realistic client script.
pub const MAX_SCRAM_SECRETS_PER_CLUSTER: usize = 1_000;

/// The message Amazon MSK answers a SCRAM-secret operation against a cluster
/// whose client authentication does not turn SASL/SCRAM on.
///
/// A cluster holds SCRAM secrets only to authenticate SASL/SCRAM clients with,
/// so the condition is about the cluster's configuration and nothing else about
/// the request: the whole sentence is fixed text, with no cluster ARN, secret
/// ARN or other request value in it. The parameter at fault is reported through
/// `BadRequestException$InvalidParameter`.
pub const SASL_SCRAM_NOT_ENABLED_MESSAGE: &str =
    "SASL/SCRAM client authentication must be enabled for the cluster.";

/// The `ClusterState` values in which a cluster HAS brokers that are not being
/// taken away from it: every value of the model's `ClusterState` enum
/// (`crate::ops::types::CLUSTER_STATE_VALUES`) except the one in which the cluster
/// has no brokers yet (`CREATING`) and the ones in which it is losing or has lost
/// them (`DELETING`, `FAILED`).
///
/// The ONE set behind every surface whose operation needs the cluster's brokers to
/// be there — its SCRAM secrets
/// ([`CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM`]) and its VPC connections
/// ([`vpc_connection_admission::CLUSTER_STATES_VPC_CONNECTIONS_ARE_MANAGED_FROM`])
/// — so two surfaces cannot drift apart about which states those are. Each surface
/// keeps its own name for the set and its own refusal sentence, because Amazon MSK
/// words them differently. Spelled in the order those sentences list them.
pub const CLUSTER_STATES_WITH_BROKERS_IN_SERVICE: [&str; 5] = [
    CLUSTER_STATE_ACTIVE,
    CLUSTER_STATE_UPDATING,
    CLUSTER_STATE_MAINTENANCE,
    CLUSTER_STATE_HEALING,
    CLUSTER_STATE_REBOOTING_BROKER,
];

/// The `ClusterState` values a SCRAM-secret operation acts on a cluster's secret
/// set from.
///
/// A cluster's SCRAM secrets are credentials its BROKERS authenticate clients
/// with, so an operation that reaches that set is served only from a state in
/// which the cluster has brokers that are not being taken away from it —
/// [`CLUSTER_STATES_WITH_BROKERS_IN_SERVICE`], which is also the order
/// [`invalid_cluster_state_for_scram_message`] builds its sentence from.
pub const CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM: [&str; 5] =
    CLUSTER_STATES_WITH_BROKERS_IN_SERVICE;

/// Whether a cluster reported in `state` serves a SCRAM-secret operation, per
/// [`CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM`]. One predicate, so every
/// operation that reaches a cluster's secret set decides it the same way.
pub fn scram_secrets_served_from_cluster_state(state: &str) -> bool {
    CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM.contains(&state)
}

/// What a SCRAM-secret operation DOES to the secret set, as the state refusal's
/// sentence names it: the association's verb phrase, and the listing's.
pub const SCRAM_SECRET_ACTION_ASSOCIATE: &str = "associate secrets";
/// See [`SCRAM_SECRET_ACTION_ASSOCIATE`].
pub const SCRAM_SECRET_ACTION_LIST: &str = "list secrets";

/// The message a SCRAM-secret operation named by `action` (one of
/// [`SCRAM_SECRET_ACTION_ASSOCIATE`], [`SCRAM_SECRET_ACTION_LIST`]) is refused
/// with on a cluster whose state does not serve it.
///
/// Fixed text: the states come from
/// [`CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM`] and the verb phrase from the
/// operation, so nothing of the request — not the cluster ARN, not a secret ARN,
/// not the cluster's own state — is interpolated. Built here rather than written
/// out per operation, so the sentence and the set it names cannot drift apart.
pub fn invalid_cluster_state_for_scram_message(action: &str) -> String {
    format!(
        "Invalid cluster state. Cluster must be in the {} state to {action}.",
        comma_or_list(&CLUSTER_STATES_SCRAM_SECRETS_ARE_SERVED_FROM)
    )
}

/// `values` as English lists them in a sentence: separated by commas, with `or`
/// before the last.
fn comma_or_list(values: &[&str]) -> String {
    match values.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_string(),
        Some((last, rest)) => format!("{}, or {last}", rest.join(", ")),
    }
}

/// Why a batch of SASL/SCRAM secrets was not associated with a cluster. Decided
/// on the one association path ([`Cluster::associate_scram_secrets`]) rather
/// than by each caller, and leaving the stored set exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScramSecretsRejected {
    /// How many secrets the cluster already holds.
    pub held: usize,
    /// How many of the request's secrets the cluster does not hold yet.
    pub added: usize,
    /// What the two would give it — more than
    /// [`MAX_SCRAM_SECRETS_PER_CLUSTER`], which is why the batch was refused.
    pub resulting: usize,
}

/// Why an append to a configuration's revision history was refused. Decided on
/// the one append path ([`ConfigurationRecord::append_revision`]) rather than by
/// each caller, and leaving the history exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionRejected {
    /// The configuration already holds [`MAX_REVISIONS_PER_CONFIGURATION`]
    /// revisions.
    HistoryFull,
    /// The submitted body is longer than [`MAX_SERVER_PROPERTIES_BYTES`].
    BodyTooLarge,
}

/// One MSK VPC connection: a client VPC's private (PrivateLink) connection to a
/// cluster, as `CreateVpcConnection` requested it. The whole submitted
/// configuration is held here so every reader computes its answer from the
/// record rather than echoing a request.
#[derive(Debug, Clone)]
pub struct VpcConnection {
    pub arn: String,
    /// The region the connection lives in: the signing region of the request
    /// that created it, and the region segment of its ARN. A regional endpoint
    /// serves only its own region's connections.
    pub region: String,
    pub target_cluster_arn: String,
    /// The account that requested the connection, as the account segment of its
    /// own ARN names it. Reported as `ClientVpcConnection$Owner` by the cluster's
    /// listing of its client connections.
    pub owner: String,
    /// One of [`VPC_CONNECTION_AUTHENTICATION_TYPES`].
    pub authentication: String,
    pub vpc_id: String,
    /// The client subnets the connection reaches the cluster from, in the order
    /// the request listed them. `DescribeVpcConnection` reports them as
    /// `Subnets`, `CreateVpcConnection` as `ClientSubnets`.
    pub client_subnets: Vec<String>,
    pub security_groups: Vec<String>,
    pub creation_time: SystemTime,
    /// The instant the clock owes this connection its `CREATING` ->
    /// `AVAILABLE` transition. Stored, never slept on.
    pub available_due: SystemTime,
    /// The instant the clock owes this connection its `REJECTING` ->
    /// `REJECTED` transition, or `None` while no rejection is in flight.
    /// Stored, never slept on, like [`Self::available_due`].
    pub rejected_due: Option<SystemTime>,
    /// A `VpcConnectionState` value.
    pub state: String,
    /// Ordered so every reader renders a stable map.
    pub tags: BTreeMap<String, String>,
}

/// A channel streaming one cluster topic to Amazon S3 or Apache Iceberg, holding
/// the whole submitted configuration so every reader is computed from the record
/// rather than echoed from a request.
#[derive(Debug, Clone)]
pub struct Channel {
    pub arn: String,
    pub name: String,
    /// The ARN of the cluster the channel reads from. A channel name is unique
    /// WITHIN a cluster, so this is half of its registry key, and a channel is
    /// only ever reported under the cluster that holds it.
    pub cluster_arn: String,
    /// The region the channel lives in: the region of the cluster that holds it,
    /// and the region segment of its own ARN.
    pub region: String,
    pub creation_time: SystemTime,
    /// The instant the clock owes this channel its `CREATING` -> `ACTIVE`
    /// transition. Stored, never slept on.
    pub active_due: SystemTime,
    /// A `ChannelStatus` value.
    pub status: String,
    /// Set by `DeleteChannel`: the instant the clock owes this channel's removal
    /// from the registry. `None` for a channel nobody has deleted.
    pub removal_due: Option<SystemTime>,
    /// A `ChannelDestinationType` value, DERIVED from which destination
    /// configuration the create carried rather than taken from the request.
    pub destination_type: String,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub iceberg_destination_configuration: Option<IcebergDestinationConfiguration>,
    pub s3_destination_configuration: Option<S3DestinationConfiguration>,
    pub topic_configuration_list: Vec<TopicConfiguration>,
    pub logging_info: Option<ChannelLoggingInfo>,
    /// Ordered so `DescribeChannel` renders a stable map.
    pub tags: BTreeMap<String, String>,
    /// The cluster operation that created the channel, as `CreateChannel` and
    /// `DescribeChannel` both report it: minted once with the channel and stored,
    /// so the two readers never disagree about it.
    pub cluster_operation_arn: String,
    /// The cluster operation carrying out the destination change the most recent
    /// accepted `UpdateChannel` asked for. `None` until one is accepted, so a
    /// channel nobody has updated reports its create as its in-flight operation.
    pub update_operation_arn: Option<String>,
    /// How many destination changes this channel has accepted. Seeds the id of
    /// each update's own cluster operation, so two updates of one channel are
    /// two distinct operations rather than one ARN handed out twice.
    pub updates_accepted: u64,
    /// The cluster operation carrying out this channel's delete, minted once when
    /// the delete is accepted. `None` until then, so `DeleteChannel` and a reader
    /// inside the `DELETING` window report the same in-flight operation however
    /// often the idempotent delete is repeated.
    pub deletion_operation_arn: Option<String>,
}

/// The `ChannelDestinationType` value for a channel whose records land in an
/// Apache Iceberg table.
pub const DESTINATION_TYPE_ICEBERG: &str = "ICEBERG";
/// The `ChannelDestinationType` value for a channel whose records land in an
/// Amazon S3 bucket.
pub const DESTINATION_TYPE_S3: &str = "S3";

/// The smallest `DataFreshnessInSeconds` a channel destination may buffer for,
/// from that member's own documentation on BOTH destination configurations
/// ("The maximum time, in seconds, that records buffer in MSK before being
/// flushed to the destination. Allowed range: 300 to 900. Default: 600.":
/// `smithy_query(query="shape",shape="S3DestinationConfiguration")`). The bound
/// belongs to the member, not to one destination kind, so it is named once here
/// and applied wherever a channel destination carries the member.
pub const MIN_DATA_FRESHNESS_SECONDS: i32 = 300;

/// The largest `DataFreshnessInSeconds` a channel destination may buffer for,
/// from the same documented range as [`MIN_DATA_FRESHNESS_SECONDS`].
pub const MAX_DATA_FRESHNESS_SECONDS: i32 = 900;

/// Whether `seconds` is a `DataFreshnessInSeconds` a channel destination may run
/// with: inside the documented range, inclusive of both published bounds.
pub fn is_data_freshness_in_range(seconds: i32) -> bool {
    (MIN_DATA_FRESHNESS_SECONDS..=MAX_DATA_FRESHNESS_SECONDS).contains(&seconds)
}

/// What MSK answers a channel destination whose `DataFreshnessInSeconds` falls
/// outside the documented range: one sentence naming the two published bounds,
/// with neither the submitted value nor the destination kind in it, since the
/// rule is the member's and reads the same whichever destination carried it.
///
/// Built FROM [`MIN_DATA_FRESHNESS_SECONDS`] and
/// [`MAX_DATA_FRESHNESS_SECONDS`], so the sentence and the check that produces
/// it cannot disagree about the range.
pub fn data_freshness_range_message() -> String {
    format!(
        "Data freshness must be between {MIN_DATA_FRESHNESS_SECONDS} and \
         {MAX_DATA_FRESHNESS_SECONDS} seconds."
    )
}

/// The refusal a channel admission answers an out-of-range
/// `DataFreshnessInSeconds` with: a `BadRequestException` (HTTP 400) carrying
/// [`data_freshness_range_message`] and naming, in `invalidParameter`, the
/// member at fault by the wire name the request itself carries.
///
/// Built HERE rather than in each handler so the create and a later update
/// cannot drift apart on how the one member-level rule is refused.
pub fn data_freshness_out_of_range() -> BadRequestException {
    BadRequestException {
        invalid_parameter: Some("dataFreshnessInSeconds".to_string()),
        message: Some(data_freshness_range_message()),
    }
}

/// What MSK answers a channel create whose `ChannelName` a channel of the target
/// cluster already holds: a `BadRequestException` (HTTP 400) — a channel name is
/// part of the request MSK validates, so the loser of a race for one name is
/// refused as a bad request rather than as a generic conflict — whose sentence
/// names the REQUESTED channel name and nothing else, neither the cluster nor
/// the ARN of the channel already holding the name.
///
/// Built from the request's own name, which is the template's one variable part.
pub fn channel_name_already_exists(name: &str) -> BadRequestException {
    BadRequestException {
        // The member at fault, by the wire name `CreateChannelRequest` carries.
        invalid_parameter: Some("channelName".to_string()),
        message: Some(format!(
            "A channel with name '{name}' already exists for this cluster."
        )),
    }
}

/// What MSK answers a channel-scoped call whose `ChannelArn` the cluster in the
/// request path holds no channel for: a `BadRequestException` (HTTP 400) whose
/// sentence is fixed end to end, carrying nothing from the request — once the
/// cluster resolves, a channel ARN it does not hold is a bad identifier in the
/// caller's own request rather than a resource that went missing.
///
/// The sentence is MSK's own, from the RECORDED answer to a channel-scoped call
/// carrying a well-formed channel ARN of an existing cluster that holds no such
/// channel (MSK names a channel a "link" internally, which is the wording it
/// refuses the identifier in); the model declares no shape for it, so it is written
/// here, in one place, rather than per handler.
pub fn invalid_channel_arn() -> BadRequestException {
    BadRequestException {
        // The member at fault, by the name the request carries it under — an
        // `@httpLabel`, so the wire name and the member name are the same.
        invalid_parameter: Some("ChannelArn".to_string()),
        message: Some(
            "The specified link ARN isn\u{2019}t valid. Specify a valid link ARN, and try again."
                .to_string(),
        ),
    }
}

/// What MSK answers an `UpdateChannel` on a channel that is not `ACTIVE`: a
/// `BadRequestException` (HTTP 400) naming the status the channel is in now and
/// the one it has to be in to be updated.
///
/// The sentence is MSK's own, from the RECORDED refusal of an `UpdateChannel`
/// against a channel whose status was not `ACTIVE`: "Channel cannot be updated in
/// state " and ". Channel must be in ACTIVE state to be updated." are its fixed
/// text, and the status token between them is the channel's CURRENT status, read
/// from its own record at the instant the call is served — never a status from any
/// one run.
pub fn channel_not_updatable(status: &str) -> BadRequestException {
    BadRequestException {
        invalid_parameter: Some("ChannelArn".to_string()),
        message: Some(format!(
            "Channel cannot be updated in state {status}. Channel must be in \
             {CHANNEL_STATUS_ACTIVE} state to be updated."
        )),
    }
}

/// Why [`ComAmazonawsKafkaKafkaState::register_channel`] refused to register a
/// channel. Every variant leaves the registry exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRejected {
    /// The cluster already holds [`MAX_CHANNELS_PER_CLUSTER`] channels.
    ClusterQuota,
    /// The registry already holds [`MAX_CHANNELS_IN_REGISTRY`] channels across
    /// every cluster, whatever each cluster holds on its own.
    RegistryCeiling,
    /// The registry holds no cluster for the ARN the channel is filed under, so
    /// nothing would ever reclaim the record. Every caller resolves the cluster
    /// first, which makes this unreachable; it is refused rather than filed.
    NoSuchCluster,
    /// A channel with this ARN is already registered. The registry's monotonic
    /// ordinal makes this unreachable; it is refused rather than overwriting an
    /// existing channel.
    DuplicateArn,
}

/// Which clusters a cluster listing keeps, on top of the endpoint's own region.
/// A `None` member filters nothing, which is how an absent request member behaves.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClusterFilter<'a> {
    /// Keep the clusters whose name starts with this prefix
    /// (`ClusterNameFilter`).
    pub name_prefix: Option<&'a str>,
    /// Keep the clusters whose `ClusterType` equals this (`ClusterTypeFilter`).
    pub cluster_type: Option<&'a str>,
}

impl ClusterFilter<'_> {
    /// Whether `cluster` passes every member this filter carries.
    fn keeps(&self, cluster: &Cluster) -> bool {
        self.name_prefix
            .is_none_or(|prefix| cluster.name.starts_with(prefix))
            && self
                .cluster_type
                .is_none_or(|kind| cluster.cluster_type == kind)
    }
}

/// One page of a cluster listing: the registry keys it reports, and the token the
/// next page resumes from.
#[derive(Debug, Default)]
pub struct ClusterPage {
    /// The ARNs this page reports, in the registry's own order.
    pub keys: Vec<String>,
    /// The token a caller passes back to get the next page, absent on the last
    /// page — minted from the registry KEY the resume comparison uses, so a full
    /// page can never be mistaken for the last one.
    pub next_token: Option<String>,
}

/// The `ReplicatorState` value a replicator holds until the clock owes it
/// [`REPLICATOR_STATE_RUNNING`]; both are members of the model's
/// `ReplicatorState` enum.
pub const REPLICATOR_STATE_CREATING: &str = "CREATING";
/// The `ReplicatorState` value a settled replicator reports.
pub const REPLICATOR_STATE_RUNNING: &str = "RUNNING";
/// The `ReplicatorState` value a deleted replicator is reported in — the state
/// `DeleteReplicator` answers with, another member of the same model enum.
pub const REPLICATOR_STATE_DELETING: &str = "DELETING";
/// The `ReplicatorState` value a replicator holds while a configuration change
/// it accepted is being rolled out, until the clock owes it
/// [`REPLICATOR_STATE_RUNNING`] again — another member of the same model enum.
pub const REPLICATOR_STATE_UPDATING: &str = "UPDATING";
/// The `ReplicatorState` value a replicator whose provisioning failed converges
/// to — the terminal member of the same model enum. A replicator that reaches it
/// reports WHY in `StateInfo` ([`Replicator::state_info`]).
pub const REPLICATOR_STATE_FAILED: &str = "FAILED";

/// The `ReplicatorState` values a call that CHANGES a replicator's replication
/// configuration is accepted from: a replicator replicates while it is
/// `RUNNING`, and every other state is one it is being set up in, rolled out in
/// or taken out in — a configuration change is not applied to any of those.
///
/// Named here so the gate and the refusal built from it
/// ([`replicator_not_in_accepted_state`]) cannot drift apart.
pub const REPLICATOR_STATES_AN_UPDATE_IS_ACCEPTED_FROM: [&str; 1] = [REPLICATOR_STATE_RUNNING];

/// How Amazon MSK refuses a call against a replicator whose current state that
/// call is not accepted from: a `BadRequestException` (HTTP 400) whose message
/// names the input member the replicator was addressed by and the set of states
/// the call IS accepted from — `accepted`, in the spelling the model's
/// `ReplicatorState` enum declares, so the sentence is built from the gate the
/// handler applied rather than written out beside it.
///
/// The member travels in the MESSAGE here, in the service's `Invalid parameter
/// <member>` framing, and the refusal names no `invalidParameter` of its own:
/// the request is well formed and it is the resource's state that refused it.
pub fn replicator_not_in_accepted_state(accepted: &[&str]) -> BadRequestException {
    BadRequestException {
        invalid_parameter: None,
        message: Some(format!(
            "Invalid parameter {REPLICATOR_ARN_MEMBER}: This operation is only valid for \
             resources that are in one of the following states: [{}]",
            accepted.join(", ")
        )),
    }
}

/// The `ReplicationStateInfo$Code` Amazon MSK reports for a replicator it could
/// not provision because an Amazon MSK cluster its setup names does not exist.
/// A fixed token, not built from the request.
pub const REPLICATION_CODE_INVALID_KAFKA_CLUSTER: &str = "InvalidInput.InvalidKafkaCluster";

/// The `ReplicationStateInfo$Message` that goes with
/// [`REPLICATION_CODE_INVALID_KAFKA_CLUSTER`]: one fixed sentence, whose one
/// variable part is the cluster ARN the request named.
pub fn invalid_kafka_cluster_message(cluster_arn: &str) -> String {
    format!("The specified Amazon MSK Cluster does not exist. Cluster ARN: {cluster_arn}")
}

/// One Kafka cluster a replicator replicates between, under the alias the
/// replicator knows it by.
///
/// The whole submitted `KafkaCluster` is kept, so every reader can be COMPUTED
/// from the record: `KafkaClusterSummary` carries only the cluster details and
/// the alias, while `DescribeReplicator`'s richer shape needs the VPC,
/// authentication and encryption settings the same record already holds.
#[derive(Debug, Clone)]
pub struct ReplicatorCluster {
    /// The alias replicated topic names are prefixed with — `KafkaClusterSummary$
    /// KafkaClusterAlias`, and what a `ReplicationInfo` is reported under.
    pub alias: String,
    /// The cluster as the replicator's creator submitted it.
    pub cluster: KafkaCluster,
}

impl ReplicatorCluster {
    /// Whether this is the cluster a `ReplicationInfo` endpoint names.
    ///
    /// A replication endpoint names its cluster by ARN (an Amazon MSK cluster) or
    /// by id (an Apache Kafka cluster), which is exactly the two ways a
    /// `KafkaCluster` can be given — so one comparison resolves either form
    /// against the stored setup.
    pub fn names(&self, arn: Option<&str>, id: Option<&str>) -> bool {
        let msk_matches = match (arn, self.cluster.amazon_msk_cluster.as_ref()) {
            (Some(arn), Some(msk)) => msk.msk_cluster_arn == arn,
            _ => false,
        };
        let apache_matches = match (id, self.cluster.apache_kafka_cluster.as_ref()) {
            (Some(id), Some(apache)) => apache.apache_kafka_cluster_id == id,
            _ => false,
        };
        msk_matches || apache_matches
    }

    /// The cluster as the `KafkaClusterSummary` a replicator reader reports: the
    /// alias plus whichever of the two cluster kinds the record holds. The
    /// summary shape models no VPC, authentication or encryption member, so those
    /// stay in the record rather than being reported here.
    pub fn to_summary(&self) -> KafkaClusterSummary {
        KafkaClusterSummary {
            amazon_msk_cluster: self.cluster.amazon_msk_cluster.clone(),
            apache_kafka_cluster: self.cluster.apache_kafka_cluster.clone(),
            kafka_cluster_alias: Some(self.alias.clone()),
        }
    }

    /// The cluster as the richer `KafkaClusterDescription` `DescribeReplicator`
    /// reports: the alias plus every member of the submitted `KafkaCluster`.
    pub fn to_description(&self) -> KafkaClusterDescription {
        KafkaClusterDescription {
            amazon_msk_cluster: self.cluster.amazon_msk_cluster.clone(),
            apache_kafka_cluster: self.cluster.apache_kafka_cluster.clone(),
            kafka_cluster_alias: Some(self.alias.clone()),
            vpc_config: self.cluster.vpc_config.clone(),
            client_authentication: self.cluster.client_authentication.clone(),
            encryption_in_transit: self.cluster.encryption_in_transit.clone(),
        }
    }
}

/// Which of a source cluster's topics one replication flow replicates: the
/// compiled form of that flow's `TopicsToReplicate` and `TopicsToExclude`.
///
/// Both members are documented as a "List of regular expression patterns", and a
/// pattern describes a WHOLE topic name rather than a part of one, so each is
/// anchored before it is compiled. A topic is replicated when some
/// `TopicsToReplicate` pattern describes it and no `TopicsToExclude` pattern does
/// — one rule, so the flow that decides a listing and the flow that decides a
/// single read cannot disagree.
///
/// Compiled ONCE, when the configuration it comes from is stored, and kept on the
/// replicator record beside that configuration: the patterns come from a request,
/// so compiling them per read would let one create size the work of every later
/// listing of the target cluster.
#[derive(Debug, Clone, Default)]
pub struct TopicFilter {
    replicate: RegexSet,
    exclude: RegexSet,
}

impl TopicFilter {
    /// The filter a stored `TopicReplication` describes.
    pub fn of(topic_replication: &TopicReplication) -> Self {
        Self {
            replicate: compile_topic_patterns(&topic_replication.topics_to_replicate),
            exclude: compile_topic_patterns(
                topic_replication.topics_to_exclude.as_deref().unwrap_or(&[]),
            ),
        }
    }

    /// Whether the flow replicates the topic `name`.
    pub fn covers(&self, name: &str) -> bool {
        self.replicate.is_match(name) && !self.exclude.is_match(name)
    }
}

/// `pattern` as the anchored expression a topic-selection pattern means: the
/// pattern describes a topic's whole name, and the group keeps an alternation
/// inside it from reaching past the anchors.
fn anchored_topic_pattern(pattern: &str) -> String {
    format!("^(?:{pattern})$")
}

/// Why `pattern` cannot be used to select topics, or `None` when it compiles.
///
/// The one rule a flow's patterns are admitted by, so `CreateReplicator` and
/// `UpdateReplicationInfo` refuse the same values — and a stored configuration
/// holds only patterns the readers can actually match topics against, rather than
/// quietly replicating nothing. A pattern longer than
/// [`MAX_TOPIC_PATTERN_LENGTH`] is refused before it is compiled, so no
/// caller-sized string reaches the regex compiler — a backstop behind the
/// boundary validator, which refuses that length off the wire.
pub fn topic_pattern_error(pattern: &str) -> Option<String> {
    let length = pattern.chars().count();
    if length > MAX_TOPIC_PATTERN_LENGTH {
        return Some(format!(
            "A topic pattern is at most {MAX_TOPIC_PATTERN_LENGTH} characters long, but this one \
             is {length}."
        ));
    }
    ::regex::Regex::new(&anchored_topic_pattern(pattern))
        .err()
        .map(|_| format!("'{pattern}' is not a valid regular expression."))
}

/// Why a replication flow's topic-selection pattern lists cannot be stored:
/// WHICH `TopicReplication` member is at fault and why.
///
/// Named so `CreateReplicator` and `UpdateReplicationInfo` admit the same values
/// and name the same member, each under its own request path — a create reports
/// the member inside the `ReplicationInfoList` entry it came in, an update
/// reports it under the update shape.
#[derive(Debug, Clone)]
pub struct TopicPatternsViolation {
    /// The member at fault, spelled as the model names it.
    pub member: &'static str,
    /// What is wrong with it.
    pub message: String,
}

/// Check both pattern lists of one replication flow: no more patterns than
/// [`MAX_TOPIC_PATTERNS_PER_FLOW`], none longer than
/// [`MAX_TOPIC_PATTERN_LENGTH`], every one of them a regular expression topics
/// can be selected by, and the list usable — within
/// [`TOPIC_MATCHER_SIZE_LIMIT`] — as the matcher a reader will build from it.
/// `None` when the lists are admissible.
///
/// Together those bound the work and the memory a stored flow can cost: the
/// patterns of one list are at most `MAX_TOPIC_PATTERNS_PER_FLOW` strings of
/// `MAX_TOPIC_PATTERN_LENGTH` characters, and the matcher compiled from them at
/// most `TOPIC_MATCHER_SIZE_LIMIT` bytes, whatever the request asked for.
pub fn topic_patterns_violation(
    topics_to_replicate: &[String],
    topics_to_exclude: &[String],
) -> Option<TopicPatternsViolation> {
    for (member, patterns) in [
        ("TopicsToReplicate", topics_to_replicate),
        ("TopicsToExclude", topics_to_exclude),
    ] {
        if patterns.len() > MAX_TOPIC_PATTERNS_PER_FLOW {
            return Some(TopicPatternsViolation {
                member,
                message: format!(
                    "{member} may hold at most {MAX_TOPIC_PATTERNS_PER_FLOW} patterns, not {}.",
                    patterns.len()
                ),
            });
        }
        if let Some(message) = patterns.iter().find_map(|pattern| topic_pattern_error(pattern)) {
            return Some(TopicPatternsViolation { member, message });
        }
        // Admitted through the SAME construction the readers match topics with, so
        // a list this service could not turn into a matcher — including one whose
        // compiled program would pass `TOPIC_MATCHER_SIZE_LIMIT` — is refused here
        // rather than stored and selecting nothing.
        if topic_matcher(patterns).is_none() {
            return Some(TopicPatternsViolation {
                member,
                message: format!(
                    "{member} holds patterns too large to be used together as a topic filter."
                ),
            });
        }
    }
    None
}

/// The matcher the patterns `patterns` describe, or `None` when they cannot be
/// compiled into one together.
///
/// The ONE construction a flow's patterns go through: the admission check builds
/// it to decide whether to accept them, and the store builds it to keep, so a
/// stored configuration can never hold patterns the readers would match nothing
/// with. Bounded by [`MAX_TOPIC_PATTERNS_PER_FLOW`] for the reason that constant
/// gives, and every pattern anchored because it describes a whole topic name.
fn topic_matcher(patterns: &[String]) -> Option<RegexSet> {
    ::regex::RegexSetBuilder::new(
        patterns
            .iter()
            .take(MAX_TOPIC_PATTERNS_PER_FLOW)
            .map(|pattern| anchored_topic_pattern(pattern)),
    )
    // The compiled program's own ceiling, set here rather than left to the regex
    // crate's default: the patterns come from a request, and a stored flow keeps
    // the matcher compiled from them, so the memory one create can leave behind
    // is bounded by this rather than by what the caller wrote.
    .size_limit(TOPIC_MATCHER_SIZE_LIMIT)
    .build()
    .ok()
}

/// The patterns `patterns` holds, compiled into the matcher a reader selects
/// topics with.
///
/// Called where a flow's configuration is STORED, never on a read path. An
/// unbuildable list cannot reach here — `CreateReplicator` and
/// `UpdateReplicationInfo` refuse one through [`topic_patterns_violation`], which
/// builds the same matcher — so the empty fallback is an unreachable backstop
/// rather than a filter anybody can be given.
fn compile_topic_patterns(patterns: &[String]) -> RegexSet {
    topic_matcher(patterns).unwrap_or_else(RegexSet::empty)
}

/// The name a replication flow gives the source topic `source_topic` on its
/// target cluster: the source cluster's alias and the topic's own name when the
/// flow prefixes replicated names, and the source topic's name itself when it
/// asks for identical ones.
pub fn replicated_topic_name(source_alias: &str, source_topic: &str, prefixed: bool) -> String {
    if prefixed {
        format!("{source_alias}{REPLICATED_TOPIC_SEPARATOR}{source_topic}")
    } else {
        source_topic.to_string()
    }
}

/// A replicator, holding the whole replication setup so every reader is computed
/// from it rather than echoed from a request.
#[derive(Debug, Clone)]
pub struct Replicator {
    /// `ReplicatorArn` — the ARN in the region this endpoint serves it from.
    pub arn: String,
    /// `ReplicatorResourceArn` — the ARN of the replicator resource in the region
    /// the replicator was CREATED in. The same ARN for a replicator created here,
    /// and a different one for a replicator reference.
    pub resource_arn: String,
    pub name: String,
    /// The region this replicator lives in: the signing region of the request
    /// that created it, and the region segment of its ARN. `ListReplicators` and
    /// `DescribeReplicator` are regional, so this is what scopes it to one
    /// endpoint. A replicator name is unique per account and region, so this is
    /// also half of its registry key.
    pub region: String,
    pub current_version: String,
    /// Which revision of its configuration the replicator is at.
    /// [`Replicator::current_version`] is this number's token, so the version
    /// changes exactly when an accepted update changes the replicator.
    pub revision: u64,
    /// `ReplicatorDescription` — the summary description the request carried, if
    /// any.
    pub description: Option<String>,
    /// The IAM role the replicator uses to reach the source and target clusters.
    pub service_execution_role_arn: String,
    pub creation_time: SystemTime,
    /// The instant the clock owes this replicator its transition into
    /// `RUNNING`: from `CREATING` after the create, and from `UPDATING` after an
    /// accepted configuration change. Stored, never slept on.
    pub running_due: SystemTime,
    /// A `ReplicatorState` value.
    pub state: String,
    /// Why provisioning this replicator failed, as the `ReplicationStateInfo` a
    /// reader reports, or `None` when every cluster it names resolved.
    ///
    /// Decided when the create is ACCEPTED and kept here, because MSK resolves a
    /// replicator's clusters while it provisions it rather than while it admits
    /// the request: the clock then owes the replicator `FAILED` instead of
    /// `RUNNING`, and this is the detail that says why.
    pub failure: Option<ReplicationStateInfo>,
    /// Whether this record is a reference to a replicator created in another
    /// region (`IsReplicatorReference`).
    pub is_replicator_reference: bool,
    /// The clusters replication runs between, in submission order.
    pub kafka_clusters: Vec<ReplicatorCluster>,
    /// The replications configured between those clusters, in submission order.
    pub replication_info: Vec<ReplicationInfo>,
    /// The compiled topic filter of each of those replications, at the same
    /// index: the one DERIVED thing on this record, built from
    /// [`Self::replication_info`] whenever that is set so no reader has to compile
    /// a caller's patterns. Set by [`Self::provisioned`] and refreshed by
    /// [`Self::apply_replication_update`], the only two places the configuration
    /// changes.
    pub topic_filters: Vec<TopicFilter>,
    /// Where the replicator's own logs are delivered, as submitted.
    pub log_delivery: Option<LogDelivery>,
    /// Ordered so a reader renders a stable map.
    pub tags: BTreeMap<String, String>,
}

/// The alias a replication endpoint resolves to through an index
/// [`Replicator::alias_index`] built, or `None` when the setup holds no cluster
/// the endpoint names. The ARN is tried first and the id second, exactly as
/// [`ReplicatorCluster::names`] matches them.
fn indexed_alias(
    aliases: &HashMap<(bool, &str), &str>,
    arn: Option<&str>,
    id: Option<&str>,
) -> Option<String> {
    if let Some(arn) = arn {
        if let Some(alias) = aliases.get(&(true, arn)) {
            return Some((*alias).to_string());
        }
    }
    aliases
        .get(&(false, id?))
        .map(|alias| (*alias).to_string())
}

/// Why [`ComAmazonawsKafkaKafkaState::register_cluster`],
/// [`ComAmazonawsKafkaKafkaState::register_configuration`] or
/// [`ComAmazonawsKafkaKafkaState::register_replicator`] refused to register a
/// resource. Every variant leaves the registry exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterRejected {
    /// The region is at its per-region quota for this kind of resource
    /// ([`MAX_CLUSTERS_PER_REGION`], [`MAX_CONFIGURATIONS_PER_REGION`]).
    RegionQuota,
    /// The registry already holds its ceiling of resources of this kind
    /// ([`MAX_CLUSTERS_IN_REGISTRY`], [`MAX_CONFIGURATIONS_IN_REGISTRY`],
    /// [`MAX_REPLICATORS_IN_REGISTRY`]).
    RegistryCeiling,
    /// A resource with this ARN is already registered. The registry's monotonic
    /// ordinal makes this unreachable; it is refused rather than overwriting an
    /// existing resource.
    DuplicateArn,
}

/// Why [`ComAmazonawsKafkaKafkaState::register_topic`] refused to register a
/// topic. Every variant leaves the registry exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicRejected {
    /// The cluster already holds a topic with this name — including one that is
    /// `DELETING` but still in the registry.
    DuplicateName,
    /// The cluster already holds [`MAX_TOPICS_PER_CLUSTER`] topics.
    ClusterCeiling,
    /// The registry already holds [`MAX_TOPICS_IN_REGISTRY`] topics across every
    /// cluster, whatever each cluster holds on its own.
    RegistryCeiling,
    /// The registry holds no cluster for the ARN the topic is filed under, so
    /// nothing would ever reclaim the record. Every caller resolves the cluster
    /// first, which makes this unreachable; it is refused rather than filed.
    NoSuchCluster,
}

/// One topic of a provisioned cluster, holding everything a reader reports so
/// `DescribeTopic` computes its answer from the record rather than echoing a
/// request.
#[derive(Debug, Clone)]
pub struct Topic {
    /// The topic's own name, unique within its cluster.
    pub name: String,
    /// The ARN of the cluster that holds the topic — half of the registry key.
    pub cluster_arn: String,
    /// `arn:<partition>:kafka:<region>:<account>:topic/<cluster>/<cluster-id>/<topic>`,
    /// the form Amazon MSK uses for a topic in an IAM policy.
    pub arn: String,
    pub partition_count: i32,
    pub replication_factor: i32,
    /// The topic-level Apache Kafka configuration the request supplied, stored
    /// verbatim because the service hands it back the same way.
    pub configs: Option<String>,
    /// A `TopicState` value.
    pub state: String,
    /// The instant this topic was created on its cluster. Not reported by any
    /// topic shape; it is what decides whether a replication flow that does not
    /// detect new topics covers this one, since such a flow replicates the topics
    /// its source cluster already held when the replicator was created.
    pub created_at: SystemTime,
    /// The instant the clock owes this topic the `ACTIVE` state: the end of its
    /// `CREATING` span, and then of the `UPDATING` span an accepted change leaves
    /// it in. Stored, never slept on.
    pub active_due: SystemTime,
    /// Set by `DeleteTopic`: the instant the clock owes this topic's removal from
    /// the registry. `None` for a topic nobody has deleted.
    pub removal_due: Option<SystemTime>,
}

impl Topic {
    /// Mint a topic of `cluster` from an accepted `CreateTopic` request. The ARN
    /// is derived from the cluster's identity, so a topic's ARN belongs to the
    /// cluster that holds it and to that cluster's region.
    pub fn created(
        cluster: &Cluster,
        name: String,
        partition_count: i32,
        replication_factor: i32,
        configs: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            arn: cluster.topic_arn(&name),
            name,
            cluster_arn: cluster.arn.clone(),
            partition_count,
            replication_factor,
            configs,
            state: TOPIC_STATE_CREATING.to_string(),
            created_at: now,
            active_due: now + TOPIC_ACTIVATION_DELAY,
            removal_due: None,
        }
    }

    /// The topic a replication flow carries onto its TARGET cluster, as that
    /// cluster's readers report it: `target` is the cluster the flow replicates
    /// INTO, `name` the name the flow gives the topic there, `source` the source
    /// cluster's topic it is carried from, `configs` the topic-level
    /// configuration the flow copies over (if any), and `visible_at` the instant
    /// the clock owed its appearance on the target.
    ///
    /// Every member is DERIVED from that source topic and that target cluster: a
    /// replicated topic carries the source topic's partitions, and its replication
    /// factor is the source topic's bounded by the brokers the TARGET cluster has
    /// to place replicas on — the same bound `CreateTopic` enforces for a topic of
    /// that cluster, since Apache Kafka cannot place more replicas of a partition
    /// than there are brokers.
    pub fn replicated(
        target: &Cluster,
        name: String,
        source: &Topic,
        configs: Option<String>,
        visible_at: SystemTime,
    ) -> Self {
        Self {
            arn: target.topic_arn(&name),
            name,
            cluster_arn: target.arn.clone(),
            partition_count: source.partition_count,
            replication_factor: source
                .replication_factor
                .clamp(1, target.number_of_broker_nodes.max(1)),
            configs,
            // The flow has finished carrying the topic over by the instant the
            // target reports it at all, so the target reports the created topic it
            // is: before that instant the target holds no such topic.
            state: TOPIC_STATE_ACTIVE.to_string(),
            created_at: visible_at,
            active_due: visible_at,
            removal_due: None,
        }
    }

    /// Whether a reader at `now` still sees this topic in its cluster: a topic
    /// whose removal the clock already owes is gone, whether or not anything has
    /// pruned the record yet.
    pub fn is_present(&self, now: SystemTime) -> bool {
        self.removal_due.is_none_or(|due| now < due)
    }

    /// Apply the transition to `ACTIVE` once `now` has reached
    /// [`Self::active_due`] — out of `CREATING` for a topic the cluster is still
    /// creating, and out of `UPDATING` for one an accepted change is still being
    /// applied to. Idempotent, and a no-op for every other state: only a
    /// transition the clock already owed is materialised here. A topic that is
    /// `DELETING` never goes back to `ACTIVE`.
    pub fn settle(&mut self, now: SystemTime) {
        if matches!(
            self.state.as_str(),
            TOPIC_STATE_CREATING | TOPIC_STATE_UPDATING
        ) && now >= self.active_due
        {
            self.state = TOPIC_STATE_ACTIVE.to_string();
        }
    }

    /// The topic as the `TopicInfo` `ListTopics` reports: its identity and the
    /// counts it was created with.
    ///
    /// `OutOfSyncReplicaCount` is 0 because every replica of an emulated topic is
    /// in sync: nothing here can put a broker behind, so the count is computed
    /// from that fact rather than invented.
    pub fn to_topic_info(&self) -> TopicInfo {
        TopicInfo {
            topic_arn: Some(self.arn.clone()),
            topic_name: Some(self.name.clone()),
            replication_factor: Some(self.replication_factor),
            partition_count: Some(self.partition_count),
            out_of_sync_replica_count: Some(0),
        }
    }
}

/// Per-service state for com-amazonaws-kafka-kafka. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKafkaKafkaState {
    /// Every live cluster, keyed by ARN. `BTreeMap` so `ListClusters` has one
    /// total order to page through.
    pub clusters: BTreeMap<String, Cluster>,
    /// (region, cluster name) -> ARN. A cluster name is unique within an account
    /// and region — the same name in two regions is two clusters — so the key
    /// carries the region as well as the name.
    pub cluster_arn_by_name: HashMap<(String, String), String>,
    /// region -> how many clusters it holds. Maintained on insert and remove by
    /// [`Self::register_cluster`] / [`Self::unregister_cluster`], so enforcing the
    /// per-region quota costs a lookup rather than a walk of the registry.
    pub clusters_per_region: HashMap<String, usize>,
    /// Monotonic counter feeding the ARN and DNS suffixes, so each cluster gets a
    /// distinct identity and a rebuilt emulator reproduces the same ones.
    pub next_cluster_suffix: u64,
    /// Every stored configuration, keyed by ARN. `BTreeMap` for the same reason
    /// the clusters are: `ListConfigurations` needs one total order to page
    /// through.
    pub configurations: BTreeMap<String, ConfigurationRecord>,
    /// (region, configuration name) -> ARN. A configuration name is unique
    /// within an account and region, so the key carries the region too.
    pub configuration_arn_by_name: HashMap<(String, String), String>,
    /// region -> how many configurations it holds, maintained by
    /// [`Self::register_configuration`] so the per-region quota costs a lookup
    /// rather than a walk of the registry.
    pub configurations_per_region: HashMap<String, usize>,
    /// Monotonic counter feeding the configuration ARNs, so each configuration
    /// gets a distinct identity.
    pub next_configuration_suffix: u64,
    /// Every cluster operation an update call recorded, keyed by its own ARN.
    /// `BTreeMap` so a reader walking the history has one total order.
    pub cluster_operations: BTreeMap<String, ClusterOperation>,
    /// Every live topic, keyed by (cluster ARN, topic name). A topic name is
    /// unique within its cluster, and the `BTreeMap` groups one cluster's topics
    /// contiguously in name order — the order `ListTopics` pages through.
    pub topics: BTreeMap<(String, String), Topic>,
    /// Every live channel, keyed by ARN. `BTreeMap` so `ListChannels` has one
    /// total order to page through.
    pub channels: BTreeMap<String, Channel>,
    /// (cluster ARN, channel name) -> channel ARN. A channel name is unique
    /// within a cluster — the same name under two clusters is two channels — so
    /// the key carries the cluster as well as the name.
    pub channel_arn_by_name: HashMap<(String, String), String>,
    /// cluster ARN -> the ARNs of the channels it holds, so the per-cluster
    /// quota costs a lookup and deleting a cluster drops exactly its own
    /// channels rather than walking the whole registry.
    pub channel_arns_by_cluster: HashMap<String, BTreeSet<String>>,
    /// cluster ARN -> the ARNs of the channels this registry HELD under that
    /// cluster and no longer holds, oldest first.
    ///
    /// What makes a channel that is GONE distinguishable from one that never
    /// existed: the first is a resource that went missing, the second a reference
    /// the caller's own request got wrong, and the service answers the two
    /// differently. Kept only for as many channels as a cluster may hold at once
    /// ([`MAX_CHANNELS_PER_CLUSTER`]) and dropped with the cluster, so the record
    /// of removals cannot grow without bound; past that the oldest removal is
    /// forgotten, which is the only thing a bound can do.
    pub removed_channel_arns: HashMap<String, VecDeque<String>>,
    /// Monotonic counter feeding each channel's ARN and its create operation's
    /// ARN, so every channel gets a distinct identity.
    pub next_channel_suffix: u64,
    /// Every registered replicator, keyed by ARN. `BTreeMap` so
    /// `ListReplicators` has one total order to page through, the same way the
    /// cluster registry gives `ListClusters` one.
    pub replicators: BTreeMap<String, Replicator>,
    /// (region, replicator name) -> ARN. A replicator name is unique within an
    /// account and region, which is what makes a repeated `CreateReplicator`
    /// under one name a conflict.
    pub replicator_arn_by_name: HashMap<(String, String), String>,
    /// Target Amazon MSK cluster ARN -> the ARNs of the replicators that run a
    /// replication flow INTO that cluster. Maintained on insert and remove by
    /// [`Self::register_replicator`] / [`Self::unregister_replicator`], so
    /// deriving one cluster's replicated topics costs a lookup rather than a walk
    /// of the whole replicator registry. The endpoints of a stored flow never
    /// change — `UpdateReplicationInfo` changes a replication's topic and
    /// consumer-group settings, not which clusters it runs between — so the index
    /// cannot drift from the records it points at.
    pub replicator_arns_by_target_cluster: HashMap<String, BTreeSet<String>>,
    /// Monotonic counter feeding replicator ARNs, so each replicator gets a
    /// distinct identity.
    pub next_replicator_suffix: u64,
    /// Every live VPC connection, keyed by ARN. `BTreeMap` so the readers have
    /// one total order to walk.
    pub vpc_connections: BTreeMap<String, VpcConnection>,
    /// Monotonic counter feeding each VPC connection's ARN, so each one gets a
    /// distinct identity.
    pub next_vpc_connection_suffix: u64,
}

impl VpcConnection {
    /// Mint a VPC connection to the cluster `cluster_arn`/`cluster_name` names
    /// from the submitted request.
    ///
    /// `region` is the caller's signing region, `suffix` the registry's
    /// monotonic counter (used as the 1-based ordinal `suffix + 1`, so one value
    /// seeds the whole identity) and `now` the creation instant. The connection
    /// starts in `CREATING`, which is the state the real service reports a
    /// just-requested connection in.
    pub fn requested(
        region: &str,
        suffix: u64,
        now: SystemTime,
        cluster_arn: &str,
        cluster_name: &str,
        input: CreateVpcConnectionInput,
    ) -> Self {
        let ordinal = suffix + 1;
        let account = ::engine::emulator_account_id();
        let digest = stable_digest(&format!(
            "{account}:{region}:{cluster_arn}:{}:{ordinal}",
            input.vpc_id
        ));
        // An MSK VPC-connection ARN names the cluster owner's account and the
        // cluster alongside the connection's own id:
        // `vpc-connection/<cluster-owner-account>/<cluster-name>/<uuid>-<n>`.
        let arn = ::engine::emulator_arn(
            "kafka",
            region,
            &format!(
                "vpc-connection/{account}/{cluster_name}/{}-{ordinal}",
                uuid_v4_shaped_from_digest(&digest)
            ),
        );
        Self {
            arn,
            region: region.to_string(),
            target_cluster_arn: cluster_arn.to_string(),
            owner: account.to_string(),
            authentication: input.authentication,
            vpc_id: input.vpc_id,
            client_subnets: input.client_subnets,
            security_groups: input.security_groups,
            creation_time: now,
            available_due: now + VPC_CONNECTION_ACTIVATION_DELAY,
            rejected_due: None,
            state: VPC_CONNECTION_STATE_CREATING.to_string(),
            tags: input.tags.unwrap_or_default().into_iter().collect(),
        }
    }

    /// Apply the transitions the clock already owed this connection: the
    /// `CREATING` -> `AVAILABLE` placement once `now` has reached
    /// [`Self::available_due`], and the `REJECTING` -> `REJECTED` rejection once
    /// it has reached [`Self::rejected_due`]. Idempotent, and a no-op for every
    /// other state: only a transition the clock already owed is materialised
    /// here.
    pub fn settle(&mut self, now: SystemTime) {
        if self.state == VPC_CONNECTION_STATE_CREATING && now >= self.available_due {
            self.state = VPC_CONNECTION_STATE_AVAILABLE.to_string();
        }
        if self.state == VPC_CONNECTION_STATE_REJECTING
            && self.rejected_due.is_some_and(|due| now >= due)
        {
            self.state = VPC_CONNECTION_STATE_REJECTED.to_string();
        }
    }

    /// Whether the target cluster's owner has already rejected this connection —
    /// the rejection still being applied (`REJECTING`) or applied
    /// (`REJECTED`).
    pub fn is_rejected(&self) -> bool {
        self.state == VPC_CONNECTION_STATE_REJECTING || self.state == VPC_CONNECTION_STATE_REJECTED
    }

    /// Reject the connection on the target cluster owner's behalf: it stops
    /// carrying traffic at once (`REJECTING`) and the clock owes it `REJECTED`
    /// at [`VPC_CONNECTION_REJECTION_DELAY`]. The record is kept — a rejected
    /// connection is still a resource every reader resolves, which is what
    /// leaves its own account able to delete it.
    ///
    /// A connection already rejected keeps the rejection it carries, so asking
    /// again for a state it already holds changes nothing about it.
    pub fn reject(&mut self, now: SystemTime) {
        if self.is_rejected() {
            return;
        }
        self.state = VPC_CONNECTION_STATE_REJECTING.to_string();
        self.rejected_due = Some(now + VPC_CONNECTION_REJECTION_DELAY);
    }

    /// The connection's tags as the map every reader reports.
    pub fn tag_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// The connection as the `VpcConnection` shape `ListVpcConnections`
    /// reports: the subset of this record that listing carries. Every member is
    /// read off the stored record, which is what keeps a listed connection and
    /// a described one from ever disagreeing.
    pub fn to_vpc_connection(&self) -> crate::ops::types::VpcConnection {
        crate::ops::types::VpcConnection {
            vpc_connection_arn: self.arn.clone(),
            target_cluster_arn: self.target_cluster_arn.clone(),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            authentication: Some(self.authentication.clone()),
            vpc_id: Some(self.vpc_id.clone()),
            state: Some(self.state.clone()),
        }
    }

    /// The connection as the `ClientVpcConnection` shape
    /// `ListClientVpcConnections` reports: the view a CLUSTER has of one client
    /// connection to it, which names the account that owns the connection
    /// instead of the cluster it targets — the caller named that cluster to get
    /// this listing. Every member is read off the same stored record
    /// [`Self::to_vpc_connection`] renders, so the two listings and
    /// `DescribeVpcConnection` cannot disagree about a connection.
    pub fn to_client_vpc_connection(&self) -> ClientVpcConnection {
        ClientVpcConnection {
            vpc_connection_arn: self.arn.clone(),
            authentication: Some(self.authentication.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            state: Some(self.state.clone()),
            owner: Some(self.owner.clone()),
        }
    }
}

/// Why a tagging operation could not resolve its `ResourceArn` to a resource.
///
/// The two variants are the two failures the real service distinguishes: a value
/// it cannot parse as one of its own resource ARNs, and a well-formed one that
/// names nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagTargetRejected {
    /// Not an ARN of an MSK resource at all — a client `BadRequestException`.
    NotAnArn,
    /// A well-formed MSK resource ARN this endpoint's region holds no resource
    /// for: absent, deleted, or belonging to another region. A `NotFoundException`.
    NoSuchResource,
}

/// The resource-type segment of an MSK cluster ARN.
pub const CLUSTER_RESOURCE_TYPE: &str = "cluster";

/// The resource-type segment of an MSK channel ARN.
pub const CHANNEL_RESOURCE_TYPE: &str = "channel";

/// The resource-type segment of an MSK replicator ARN.
pub const REPLICATOR_RESOURCE_TYPE: &str = "replicator";

/// The message AWS's resource-tagging plumbing answers a missing resource with.
///
/// RECORDED behaviour, not modelled: the Smithy model gives `NotFoundException`
/// only a free-form `message`, and the recorded responses for the MSK tagging
/// operations carry this fixed text (the same text the tagging APIs of other
/// services answer with) rather than one built from the request. The requirement
/// here is the 404 and the `NotFoundException` code; this constant keeps the
/// three tagging operations answering with one text instead of three.
///
/// It is NOT the text the resource readers use for a resource they cannot find:
/// those build a message from the request (which region looked, for which ARN).
/// Reach for this one only from the tagging operations.
pub const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The longest a tag key may be, from the model's own documentation of
/// `UntagResourceRequest$TagKeys`: "Tag keys must be between 1 and 128 Unicode
/// characters in length."
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The tag-key prefix the model reserves for AWS itself: "You can't start a tag
/// key with aws: because this prefix is reserved for use by AWS. AWS creates tags
/// that begin with this prefix on your behalf, but you can't edit or delete them."
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The sentence Amazon MSK answers a tag key that starts with the reserved
/// prefix with: fixed text end to end — the offending key is NOT named — and
/// stating that the test is applied without regard to case, which is how the
/// service applies it ([`starts_with_reserved_tag_prefix`]).
pub const RESERVED_TAG_KEY_MESSAGE: &str =
    "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)";

/// Whether `key` starts with the reserved prefix, compared WITHOUT regard to
/// case: `AWS:`, `Aws:` and `aws:` are one prefix, which is what the refusal
/// says ([`RESERVED_TAG_KEY_MESSAGE`]) and what keeps a caller from writing a
/// key the service reserves by spelling it differently.
pub fn starts_with_reserved_tag_prefix(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// Why `key` is not a tag key a caller may set or remove, phrased as the message
/// a `BadRequestException` carries — or `None` when the key is acceptable.
///
/// The length bound and the reserved prefix are stated by the model's own
/// `UntagResourceRequest$TagKeys` documentation: 1..128 Unicode characters and no
/// `aws:` prefix. The character set is AWS's own tagging rule (AWS General
/// Reference, "Tagging AWS resources"): Unicode letters, digits, white space and
/// the characters `+ - = . _ : / @` — the colon included, which is what makes a
/// namespaced key (`<namespace>:<name>`) a legal key everywhere tags are written.
///
/// The reserved-prefix clause is the service's own fixed sentence
/// ([`RESERVED_TAG_KEY_MESSAGE`]), which names neither the key nor the prefix;
/// the other two state the constraint and the offending key, for which no
/// wording of the real service has been observed.
pub fn tag_key_violation(key: &str) -> Option<String> {
    let length = key.chars().count();
    if !(1..=MAX_TAG_KEY_LENGTH).contains(&length) {
        return Some(format!(
            "Tag key '{key}' is {length} Unicode characters long; a tag key must be between 1 and {MAX_TAG_KEY_LENGTH} Unicode characters in length."
        ));
    }
    if starts_with_reserved_tag_prefix(key) {
        return Some(RESERVED_TAG_KEY_MESSAGE.to_string());
    }
    if let Some(offending) = key.chars().find(|c| !is_tag_key_character(*c)) {
        return Some(format!(
            "Tag key '{key}' contains the unsupported character '{offending}'; a tag key must consist of Unicode letters, digits, white space and the special characters + - = . _ : / @."
        ));
    }
    None
}

/// Whether `c` is one of the characters AWS's tagging rule admits in a tag key:
/// a Unicode letter, a digit, white space, or one of `+ - = . _ : / @`.
fn is_tag_key_character(c: char) -> bool {
    c.is_alphabetic()
        || c.is_numeric()
        || c.is_whitespace()
        || matches!(c, '+' | '-' | '=' | '.' | '_' | ':' | '/' | '@')
}

/// The most tags one resource may carry, from the Amazon MSK tag restrictions
/// ("Maximum number of tags per resource: 50") — the same limit, from the same
/// source, that [`MAX_TAGS_PER_REPLICATOR`] enforces on the tags a replicator is
/// created with.
///
/// It is also what bounds this store: a tag key comes from the request, so
/// without a ceiling on the resulting count a caller could grow one resource's
/// tag map with every call it makes. The ceiling is checked BEFORE the first
/// insert, so a refused request writes nothing.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest a tag value may be, from the same restrictions ("Maximum value
/// length: 256 Unicode characters"). An empty value is allowed.
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// Why `submitted` may not be written onto a resource that already carries
/// `stored`, phrased as the message a `BadRequestException` takes — or `None`
/// when the write is acceptable.
///
/// The ONE admission check for a tag write, shared by `TagResource` and by the
/// `Tags` a create carries, so the two cannot disagree about which tag sets a
/// resource may hold. It checks every key and value against the documented
/// limits and then the resulting entry count: only keys the resource does not
/// already carry add to it, so re-tagging an existing key at the ceiling is
/// still allowed. The submitted pairs are walked in key order, so one request
/// always earns the same message.
pub fn tag_write_violation<'a, I>(stored: &BTreeMap<String, String>, submitted: I) -> Option<String>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let submitted: BTreeMap<&str, &str> = submitted.into_iter().collect();
    for (key, value) in &submitted {
        if let Some(reason) = tag_key_violation(key) {
            return Some(reason);
        }
        let length = value.chars().count();
        if length > MAX_TAG_VALUE_LENGTH {
            return Some(format!(
                "The value of tag key '{key}' is {length} Unicode characters long; a tag value must be at most {MAX_TAG_VALUE_LENGTH} Unicode characters in length."
            ));
        }
    }
    let added = submitted
        .keys()
        .filter(|key| !stored.contains_key(**key))
        .count();
    let resulting = stored.len() + added;
    if resulting > MAX_TAGS_PER_RESOURCE {
        return Some(format!(
            "The resource carries {} tag(s) and the request adds {added} more, which would give it {resulting}; a resource may carry at most {MAX_TAGS_PER_RESOURCE} tags.",
            stored.len()
        ));
    }
    None
}

/// Encode the ARN a page stopped at as a token, through the one page-token
/// codec of this service ([`encode_page_token`]) under the cluster listings' own
/// marker. The token is ENCODED, not authenticated, so a caller could decode it
/// or mint one. That is harmless here — the ARN inside is only used as a `>`
/// comparison against the keys of the caller's own registry, so a forged resume
/// point can only skip or repeat entries the caller may already list.
pub fn encode_cluster_page_token(last_arn: &str) -> String {
    encode_page_token(CLUSTER_PAGE_TOKEN_PREFIX, last_arn)
}

/// The ARN a token resumes after, or `None` when the token is not one a cluster
/// listing minted — which every listing refuses as a bad request rather than
/// serving as a first page.
pub fn decode_cluster_page_token(token: &str) -> Option<String> {
    decode_page_token(CLUSTER_PAGE_TOKEN_PREFIX, token)
}

impl ConfigurationRecord {
    /// Mint a configuration from the submitted request: derive its identity from
    /// `suffix` (the registry's monotonic counter) and store the submitted body
    /// as revision 1.
    ///
    /// `region` is the caller's signing region, `now` the creation instant. The
    /// ARN takes the form the real service mints,
    /// `arn:<partition>:kafka:<region>:<account>:configuration/<name>/<id>-<ordinal>`.
    ///
    /// Refuses a body longer than [`MAX_SERVER_PROPERTIES_BYTES`], exactly as
    /// [`Self::append_revision`] refuses one: revision 1 is a stored body like any
    /// other, so the ceiling lives on BOTH paths that store one and the bound
    /// cannot be bypassed by minting a record instead of appending to it.
    pub fn created(
        region: &str,
        suffix: u64,
        now: SystemTime,
        input: CreateConfigurationInput,
    ) -> Result<Self, RevisionRejected> {
        if server_properties_too_large(&input.server_properties) {
            return Err(RevisionRejected::BodyTooLarge);
        }
        let name = input.name;
        // The registry's counter, as the 1-based ordinal AWS puts at the end of
        // a configuration ARN; the SAME value seeds the opaque id before it.
        let ordinal = suffix + 1;
        let digest = stable_digest(&format!(
            "configuration:{}:{region}:{name}:{ordinal}",
            ::engine::emulator_account_id()
        ));
        let arn = ::engine::emulator_arn(
            "kafka",
            region,
            &format!(
                "configuration/{name}/{}-{ordinal}",
                uuid_v4_shaped_from_digest(&digest)
            ),
        );
        // `Description` is optional on the request and on the readers that
        // report it, and an empty description is no description, so the one
        // normalisation decides what is stored.
        let description = stored_description(input.description);
        Ok(Self {
            arn,
            name,
            region: region.to_string(),
            kafka_versions: input.kafka_versions.unwrap_or_default(),
            creation_time: now,
            state: CONFIGURATION_STATE_ACTIVE.to_string(),
            removal_due: None,
            revisions: vec![ConfigurationRevisionRecord {
                revision: 1,
                creation_time: now,
                description: description.clone(),
                server_properties: input.server_properties,
            }],
            description,
        })
    }

    /// Whether this configuration is still part of the registry a caller at
    /// `now` sees: one whose removal the clock already owes is gone, whether or
    /// not the record has been pruned yet.
    pub fn is_live(&self, now: SystemTime) -> bool {
        self.removal_due.is_none_or(|due| now < due)
    }

    /// Accept this configuration's deletion at `now`: move it to `DELETING` and
    /// record the instant the clock owes its removal from the registry.
    ///
    /// Idempotent — a configuration already `DELETING` keeps the removal instant
    /// it was given, so repeating the call cannot postpone the removal.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.removal_due.is_some() {
            return;
        }
        self.state = CONFIGURATION_STATE_DELETING.to_string();
        self.removal_due = Some(now + CONFIGURATION_DELETION_DELAY);
    }

    /// The newest stored revision. Infallible on a registered configuration:
    /// [`Self::created`] stores revision 1 and an update only ever appends.
    pub fn latest_revision(&self) -> &ConfigurationRevisionRecord {
        self.revisions
            .last()
            .expect("a configuration always holds at least revision 1")
    }

    /// Store `server_properties` as this configuration's next revision and hand
    /// the stored revision back.
    ///
    /// A configuration's revision history is append-only: the submitted body
    /// becomes the revision one past the current latest and no stored revision
    /// is ever rewritten, which is what lets a reader of an earlier revision see
    /// the body that revision was created with. `description` is that revision's
    /// own description, normalised through [`stored_description`] — `None` when
    /// the request named none or named an empty one, since
    /// `ConfigurationRevision$Description` is optional and reports a revision
    /// with no description by omitting the member. The configuration's own
    /// description is the one it was created with.
    ///
    /// This is the one append path, so it is also the one place the history's
    /// ceilings are enforced: it refuses, storing nothing, a body past
    /// [`MAX_SERVER_PROPERTIES_BYTES`] and an append past
    /// [`MAX_REVISIONS_PER_CONFIGURATION`].
    pub fn append_revision(
        &mut self,
        now: SystemTime,
        description: Option<String>,
        server_properties: Vec<u8>,
    ) -> Result<&ConfigurationRevisionRecord, RevisionRejected> {
        if server_properties_too_large(&server_properties) {
            return Err(RevisionRejected::BodyTooLarge);
        }
        if self.revisions.len() >= MAX_REVISIONS_PER_CONFIGURATION {
            return Err(RevisionRejected::HistoryFull);
        }
        let revision = self.latest_revision().revision + 1;
        self.revisions.push(ConfigurationRevisionRecord {
            revision,
            creation_time: now,
            description: stored_description(description),
            server_properties,
        });
        Ok(self.latest_revision())
    }

    /// The stored revision numbered `revision`, or `None` when this
    /// configuration holds no such revision — the distinction between a revision
    /// number a reader reports and one it must refuse as absent.
    pub fn revision(&self, revision: i64) -> Option<&ConfigurationRevisionRecord> {
        self.revisions
            .iter()
            .find(|stored| stored.revision == revision)
    }

    /// The configuration as the `Configuration` shape `ListConfigurations` and
    /// `DescribeConfiguration` report, so every reader of this record renders it
    /// the same way.
    pub fn to_configuration(&self) -> Configuration {
        let latest = self.latest_revision();
        Configuration {
            arn: self.arn.clone(),
            creation_time: ::aws_smithy_types::DateTime::from(self.creation_time),
            // `Configuration$Description` is `@required` on this shape, so the
            // member is always written and a configuration that has no
            // description can only be rendered with the empty string here. The
            // readers whose own member is OPTIONAL
            // (`DescribeConfigurationResponse$Description`) omit it instead —
            // which is also why the empty string a request may submit is stored
            // as no description at all.
            description: self.description.clone().unwrap_or_default(),
            kafka_versions: self.kafka_versions.clone(),
            latest_revision: latest.to_configuration_revision(),
            name: self.name.clone(),
            state: self.state.clone(),
        }
    }
}

impl ConfigurationRevisionRecord {
    /// This revision as the modelled `ConfigurationRevision` shape, so the
    /// writer and every reader of a stored revision render it the same way.
    pub fn to_configuration_revision(&self) -> ConfigurationRevision {
        ConfigurationRevision {
            creation_time: ::aws_smithy_types::DateTime::from(self.creation_time),
            // Optional on the shape, so a revision that has no description is
            // reported by leaving the member out.
            description: self.description.clone(),
            revision: self.revision,
        }
    }
}

/// How many characters of a cluster's NAME the alias MSK derives for it carries.
///
/// The alias is the name truncated to this many characters, a hyphen and
/// [`KAFKA_CLUSTER_ALIAS_ID_CHARS`] characters of the cluster's id, so it stays
/// the same length whatever the name and tells two clusters whose names share a
/// prefix apart.
const KAFKA_CLUSTER_ALIAS_NAME_CHARS: usize = 28;

/// How many hexadecimal digits of a cluster's own id (the uuid segment of its
/// ARN) the alias carries, after the truncated name: the uuid's last group.
const KAFKA_CLUSTER_ALIAS_ID_CHARS: usize = 12;

/// The alias of one `KafkaCluster` entry: an Amazon MSK cluster is aliased by the
/// alias MSK DERIVES from its ARN ([`msk_cluster_alias`]), and an Apache Kafka
/// cluster by the id the caller gave it. The alias is what prefixes the names of
/// replicated topics, per `KafkaClusterDescription$KafkaClusterAlias`.
pub fn kafka_cluster_alias(cluster: &KafkaCluster) -> Option<String> {
    if let Some(msk) = cluster.amazon_msk_cluster.as_ref() {
        return flow_cluster_alias(Some(msk.msk_cluster_arn.as_str()), None);
    }
    cluster
        .apache_kafka_cluster
        .as_ref()
        .map(|apache| apache.apache_kafka_cluster_id.clone())
}

/// WHICH Kafka cluster one `KafkaClusters` entry names, as the request names it:
/// the Amazon MSK cluster's ARN, or the Apache Kafka cluster's id — the value
/// the entry identifies its cluster by, and `None` for an entry that identifies
/// none.
///
/// This is what makes the list a set: two entries are the same cluster when
/// they name the same one ([`DUPLICATE_KAFKA_CLUSTERS_MESSAGE`]). Read from the
/// entry itself rather than from the alias derived for it, so the test is over
/// the identifier the caller sent.
pub fn kafka_cluster_identity(cluster: &KafkaCluster) -> Option<&str> {
    if let Some(msk) = cluster.amazon_msk_cluster.as_ref() {
        return Some(msk.msk_cluster_arn.as_str());
    }
    cluster
        .apache_kafka_cluster
        .as_ref()
        .map(|apache| apache.apache_kafka_cluster_id.as_str())
}

/// Whether the `KafkaClusters` list `clusters` names any one Kafka cluster more
/// than once ([`kafka_cluster_identity`]) — the condition
/// [`DUPLICATE_KAFKA_CLUSTERS_MESSAGE`] is answered for.
///
/// An entry that identifies no cluster is not compared: that request is refused
/// on the entry itself, which is a different condition.
pub fn names_a_kafka_cluster_twice(clusters: &[KafkaCluster]) -> bool {
    let named: Vec<&str> = clusters.iter().filter_map(kafka_cluster_identity).collect();
    let distinct: std::collections::BTreeSet<&str> = named.iter().copied().collect();
    distinct.len() != named.len()
}

/// The alias of the cluster a replication flow names at one end, from the ARN or
/// the cluster id the flow carries. Same rule as [`kafka_cluster_alias`]: the
/// derived alias for an MSK ARN, the id itself for an Apache Kafka cluster.
pub fn flow_cluster_alias(arn: Option<&str>, id: Option<&str>) -> Option<String> {
    if let Some(arn) = arn.filter(|value| !value.is_empty()) {
        return Some(msk_cluster_alias(arn).unwrap_or_else(|| arn.to_string()));
    }
    id.filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// One END of a replication, as a request names it: the pair of members that can
/// carry the Kafka cluster at that end, spelled as the wire spells them, and the
/// word the service's refusals call that end by.
///
/// Both members of each pair are optional on every shape that takes a replication
/// (`ReplicationInfo`, and the `UpdateReplicationInfo` request itself) because
/// EITHER one identifies the cluster — not because an end may be left unnamed or
/// named twice. That rule is not something the model can state, so the service
/// states it at runtime, and it states it from here so every operation that takes
/// a replication's ends enforces it in the same words.
#[derive(Debug, Clone, Copy)]
pub struct ReplicationEnd {
    /// Which end this is, as the refusal names it.
    end: &'static str,
    /// The member carrying the end's Amazon MSK cluster ARN.
    arn_member: &'static str,
    /// The member carrying the end's Apache Kafka cluster id.
    id_member: &'static str,
}

/// The end a replication runs FROM.
pub const SOURCE_REPLICATION_END: ReplicationEnd = ReplicationEnd {
    end: "source",
    arn_member: "sourceKafkaClusterArn",
    id_member: "sourceKafkaClusterId",
};

/// The end a replication runs TO.
pub const TARGET_REPLICATION_END: ReplicationEnd = ReplicationEnd {
    end: "target",
    arn_member: "targetKafkaClusterArn",
    id_member: "targetKafkaClusterId",
};

impl ReplicationEnd {
    /// The member carrying this end's Amazon MSK cluster ARN, as the wire spells
    /// it: the parameter a refusal about this end names at fault, and the first
    /// of the two members [`Self::unnamed_message`] tells the caller to send.
    pub fn arn_member(&self) -> &'static str {
        self.arn_member
    }

    /// What the service refuses a request that does not name this end exactly
    /// once with: one fixed sentence per end, built from the two member names the
    /// wire carries and the word for the end, with NO part of it taken from the
    /// request.
    pub fn unnamed_message(&self) -> String {
        format!(
            "Must specify either {} or {} for the {} cluster.",
            self.arn_member, self.id_member, self.end
        )
    }
}

/// The alias of the cluster at `end`, from the pair of members a request can name
/// it by — or the refusal for that end when the pair does not name it exactly
/// once.
///
/// One of the two members, never both and never neither: a cluster given by both
/// its ARN and its cluster id is named twice, and one given by neither is not
/// named at all, so neither request describes an end this service can resolve.
/// A member sent EMPTY names nothing, the same reading [`flow_cluster_alias`]
/// gives it, so it counts as absent rather than as an end named by a value
/// nothing resolves. The alias itself comes from that one rule, so an end named
/// either way resolves to the alias every reader of a replicator reports it
/// under.
pub fn replication_end_alias(
    end: ReplicationEnd,
    arn: Option<&str>,
    id: Option<&str>,
) -> Result<String, String> {
    let arn = arn.filter(|value| !value.is_empty());
    let id = id.filter(|value| !value.is_empty());
    match (arn, id) {
        (Some(_), None) | (None, Some(_)) => {
            flow_cluster_alias(arn, id).ok_or_else(|| end.unnamed_message())
        }
        _ => Err(end.unnamed_message()),
    }
}

/// The alias Amazon MSK knows the cluster `arn` names a replicator by, or `None`
/// when `arn` is not an MSK cluster ARN.
///
/// DERIVED from the ARN, not read off it: MSK takes the cluster NAME, truncated
/// to [`KAFKA_CLUSTER_ALIAS_NAME_CHARS`] characters, a hyphen, and the cluster's
/// own id as [`KAFKA_CLUSTER_ALIAS_ID_CHARS`] hexadecimal digits
/// ([`cluster_alias_id`]). So the alias identifies the cluster rather than just
/// naming it: two clusters that share a name prefix, or the same name in two
/// accounts, get different aliases, and an alias is one fixed shape however long
/// the name is. One place, so the create and the describe projections of a
/// replicator cannot disagree about it.
pub fn msk_cluster_alias(arn: &str) -> Option<String> {
    let mut segments = kafka_arn_resource(arn)?.split('/');
    if segments.next()? != CLUSTER_RESOURCE_TYPE {
        return None;
    }
    let name = segments.next().filter(|name| !name.is_empty())?;
    let name: String = name.chars().take(KAFKA_CLUSTER_ALIAS_NAME_CHARS).collect();
    let id = cluster_alias_id(segments.next().unwrap_or_default());
    Some(format!("{name}-{id}"))
}

/// The [`KAFKA_CLUSTER_ALIAS_ID_CHARS`] hexadecimal digits of a cluster's id that
/// its alias carries: the FINAL GROUP of the uuid the ARN's id segment names, and
/// zeroes when that segment names no uuid — the alias keeps one shape either way,
/// and an ARN that carries no cluster id contributes none of its own.
fn cluster_alias_id(id: &str) -> String {
    /// The widths of a uuid's hyphen-separated groups, the last of which is the
    /// one an alias carries.
    const UUID_GROUP_WIDTHS: [usize; 5] = [8, 4, 4, 4, KAFKA_CLUSTER_ALIAS_ID_CHARS];

    // `splitn` with one field more than a uuid has leaves anything after the
    // uuid — the ordinal this emulator's own cluster ARNs carry — in the last
    // field, where it is not part of the uuid being read.
    let groups: Vec<&str> = id.splitn(UUID_GROUP_WIDTHS.len() + 1, '-').collect();
    let shaped = groups.len() >= UUID_GROUP_WIDTHS.len()
        && groups
            .iter()
            .zip(UUID_GROUP_WIDTHS)
            .all(|(group, width)| {
                group.len() == width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
            });
    match shaped {
        true => groups[UUID_GROUP_WIDTHS.len() - 1].to_string(),
        false => "0".repeat(KAFKA_CLUSTER_ALIAS_ID_CHARS),
    }
}

/// The `ReplicationStartingPositionType` a replication starts from when the
/// request names no `StartingPosition`: the newest records, so an accepted
/// replication does not begin by copying every record the source cluster still
/// retains.
pub const DEFAULT_REPLICATION_STARTING_POSITION: &str = "LATEST";

/// The `ReplicationTopicNameConfigurationType` a replication uses when the
/// request names no `TopicNameConfiguration`: replicated topic names are prefixed
/// with the source cluster's alias, which is what
/// `KafkaClusterDescription$KafkaClusterAlias` documents the alias is for ("Used
/// to prefix names of replicated topics").
pub const DEFAULT_REPLICATION_TOPIC_NAME_CONFIGURATION: &str =
    "PREFIXED_WITH_SOURCE_CLUSTER_ALIAS";

/// The `ConsumerGroupOffsetSyncMode` a replication uses when the request names
/// none: the mode that needs nothing else in place, since the model documents
/// `ENHANCED` as requiring a corresponding replicator replicating the other way
/// (`smithy_query(query="doc",shape="ConsumerGroupOffsetSyncMode")`).
pub const DEFAULT_CONSUMER_GROUP_OFFSET_SYNC_MODE: &str = "LEGACY";

/// One submitted `KafkaCluster` as the service APPLIES it: the client VPC
/// configuration reports the security groups attached to the replicator's network
/// interfaces, which is an EMPTY list — not an absent member — when the request
/// attached none.
///
/// Applied where the configuration is stored, so every reader of the record
/// reports the same thing and no projection has to re-derive it.
pub fn applied_kafka_cluster(mut cluster: KafkaCluster) -> KafkaCluster {
    if let Some(vpc_config) = cluster.vpc_config.as_mut() {
        vpc_config.security_group_ids.get_or_insert_with(Vec::new);
    }
    cluster
}

/// One submitted `ReplicationInfo` as the service APPLIES it: every member of the
/// topic and consumer-group replication that the request left out is filled in
/// with the value the service runs the replication with — a selection the request
/// did not narrow excludes nothing, the two configuration shapes carry the
/// service's default type, and a feature switch the request did not set is ON,
/// because a replication MSK sets up keeps the target's topics, their
/// configurations, their ACLs and their consumer groups matching the source
/// unless it is told otherwise.
///
/// So a reader reports the whole configuration the replication RUNS under rather
/// than only the subset the request happened to spell out.
pub fn applied_replication_info(mut info: ReplicationInfo) -> ReplicationInfo {
    let topics = &mut info.topic_replication;
    topics.copy_access_control_lists_for_topics.get_or_insert(true);
    topics.copy_topic_configurations.get_or_insert(true);
    topics.detect_and_copy_new_topics.get_or_insert(true);
    topics.topics_to_exclude.get_or_insert_with(Vec::new);
    topics
        .starting_position
        .get_or_insert_with(|| ReplicationStartingPosition {
            r#type: Some(DEFAULT_REPLICATION_STARTING_POSITION.to_string()),
        })
        .r#type
        .get_or_insert_with(|| DEFAULT_REPLICATION_STARTING_POSITION.to_string());
    topics
        .topic_name_configuration
        .get_or_insert_with(|| ReplicationTopicNameConfiguration {
            r#type: Some(DEFAULT_REPLICATION_TOPIC_NAME_CONFIGURATION.to_string()),
        })
        .r#type
        .get_or_insert_with(|| DEFAULT_REPLICATION_TOPIC_NAME_CONFIGURATION.to_string());
    let groups = &mut info.consumer_group_replication;
    groups.detect_and_copy_new_consumer_groups.get_or_insert(true);
    groups.synchronise_consumer_group_offsets.get_or_insert(true);
    groups.consumer_groups_to_exclude.get_or_insert_with(Vec::new);
    groups
        .consumer_group_offset_sync_mode
        .get_or_insert_with(|| DEFAULT_CONSUMER_GROUP_OFFSET_SYNC_MODE.to_string());
    info
}

/// `<host>:<port>` for each host, comma-separated. The hosts are borrowed, so
/// rendering a bootstrap string does not clone the stored endpoint list.
fn join_endpoints<'a>(hosts: impl Iterator<Item = &'a str>, port: u16) -> String {
    hosts
        .map(|host| format!("{host}:{port}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// How many broker nodes a cluster may be left holding: `target` bounded to the
/// range the model admits for a broker count
/// (`CreateClusterRequest$NumberOfBrokerNodes` and
/// `UpdateBrokerCountRequest$TargetNumberOfBrokerNodes` are both
/// `__integerMin1Max15`), i.e. 1..=[`MAX_BROKER_NODES_PER_CLUSTER`].
///
/// Every path that sizes a broker list, or derives the ids of the nodes a change
/// adds or removes, goes through this ONE bound: a request value is what those
/// paths are driven by, and the handlers that refuse an out-of-range value are
/// what make such a value a bad request rather than an allocation. Bounding it
/// first is also what keeps the id arithmetic those paths do inside `i32`.
fn admissible_broker_count(target: i32) -> i32 {
    target.clamp(1, MAX_BROKER_NODES_PER_CLUSTER)
}

/// What one cluster ordinal seeds: the cluster's ARN, and the DNS suffix its
/// endpoints sit under.
struct ClusterIdentity {
    arn: String,
    dns_suffix: String,
}

/// Derive a cluster's identity from the registry's monotonic counter, so one
/// number seeds its uuid, its ARN tail and its endpoint host names — and both
/// create paths mint an ARN the same way, whichever kind of cluster they
/// register.
fn cluster_identity(region: &str, name: &str, suffix: u64) -> ClusterIdentity {
    // The counter, as the 1-based ordinal AWS puts at the end of a cluster ARN.
    // The SAME value seeds the digest, so a cluster's uuid, DNS suffix and ARN
    // tail all follow from one number.
    let ordinal = suffix + 1;
    let digest = stable_digest(&format!(
        "{}:{region}:{name}:{ordinal}",
        ::engine::emulator_account_id()
    ));
    let cluster_uuid = uuid_v4_shaped_from_digest(&digest);
    ClusterIdentity {
        arn: ::engine::emulator_arn(
            "kafka",
            region,
            &format!("cluster/{name}/{cluster_uuid}-{ordinal}"),
        ),
        // The DNS suffix of an MSK endpoint is a short opaque cluster id plus the
        // cell the cluster landed in: `b-1.demo.a1b2c3.c4.kafka.<region>...`.
        dns_suffix: format!(
            "{:02x}{:02x}{:02x}.c{}",
            digest[16],
            digest[17],
            digest[18],
            1 + u32::from(digest[19] % 9)
        ),
    }
}

/// The `Provisioned` body of a `CreateClusterV2`, as the `CreateClusterInput`
/// [`Cluster::provisioned`] mints from.
///
/// The two requests carry the SAME provisioned configuration under different
/// shape names, so regrouping the v2 body here is what makes a v2 provisioned
/// create and a v1 create register one kind of record instead of two near-copies
/// that could drift apart.
pub fn provisioned_create_input(
    cluster_name: String,
    tags: Option<HashMap<String, String>>,
    request: ProvisionedRequest,
) -> CreateClusterInput {
    CreateClusterInput {
        broker_node_group_info: request.broker_node_group_info,
        rebalancing: request.rebalancing,
        client_authentication: request.client_authentication,
        cluster_name,
        configuration_info: request.configuration_info,
        encryption_info: request.encryption_info,
        enhanced_monitoring: request.enhanced_monitoring,
        open_monitoring: request.open_monitoring,
        kafka_version: request.kafka_version,
        logging_info: request.logging_info,
        number_of_broker_nodes: request.number_of_broker_nodes,
        tags,
        storage_mode: request.storage_mode,
    }
}

/// The submitted `EncryptionInfo` with every default resolved, so the stored
/// record describes the settings the cluster actually runs with.
fn resolve_encryption_info(region: &str, submitted: Option<EncryptionInfo>) -> EncryptionInfo {
    let submitted = submitted.unwrap_or_default();
    let in_transit = submitted.encryption_in_transit.unwrap_or_default();
    EncryptionInfo {
        encryption_at_rest: Some(submitted.encryption_at_rest.unwrap_or_else(|| {
            EncryptionAtRest {
                data_volume_kms_key_id: default_kafka_kms_key_arn(region),
            }
        })),
        encryption_in_transit: Some(EncryptionInTransit {
            client_broker: Some(
                in_transit
                    .client_broker
                    .unwrap_or_else(|| DEFAULT_CLIENT_BROKER.to_string()),
            ),
            // Documented default of `EncryptionInTransit$InCluster`: true.
            in_cluster: Some(in_transit.in_cluster.unwrap_or(true)),
        }),
    }
}

/// The AWS-managed KMS key MSK encrypts data volumes with when the request names
/// none. One key per region, as an AWS-managed key is.
fn default_kafka_kms_key_arn(region: &str) -> String {
    let digest = stable_digest(&format!(
        "{}:{region}:aws/kafka",
        ::engine::emulator_account_id()
    ));
    ::engine::emulator_arn(
        "kms",
        region,
        &format!("key/{}", uuid_v4_shaped_from_digest(&digest)),
    )
}

/// The storage attributes a cluster operation reports, as `MutableClusterInfo`.
///
/// A volume size and throughput belong to the brokers, so they are reported
/// through `BrokerEBSVolumeInfo`; `StorageMode` belongs to the cluster. Every
/// other member is left unset: an operation describes the attributes it changes,
/// not the whole cluster.
fn storage_cluster_info(
    volume_size_gb: Option<i32>,
    provisioned_throughput: Option<ProvisionedThroughput>,
    storage_mode: &str,
) -> MutableClusterInfo {
    MutableClusterInfo {
        broker_ebs_volume_info: Some(vec![BrokerEBSVolumeInfo {
            // "The broker identifier must be set to the keyword ALL. This means
            // the changes apply to all the brokers in the cluster."
            // (`BrokerEBSVolumeInfo`'s model documentation.)
            kafka_broker_node_id: "ALL".to_string(),
            provisioned_throughput,
            volume_size_gb,
        }]),
        storage_mode: Some(storage_mode.to_string()),
        ..MutableClusterInfo::default()
    }
}

/// The broker type a cluster operation reports, as `MutableClusterInfo`.
///
/// `InstanceType` is the only member a broker-type update touches, so it is the
/// only one set: an operation describes the attributes it changes, not the whole
/// cluster — the same rule [`storage_cluster_info`] follows.
fn broker_type_cluster_info(instance_type: &str) -> MutableClusterInfo {
    MutableClusterInfo {
        instance_type: Some(instance_type.to_string()),
        ..MutableClusterInfo::default()
    }
}

/// The software attributes a cluster operation reports, as `MutableClusterInfo`:
/// the Apache Kafka version the brokers run and the configuration revision they
/// run it with.
///
/// Those are what an Apache Kafka version update touches, so they are the only
/// members set — the same rule [`storage_cluster_info`] follows.
fn kafka_version_cluster_info(
    kafka_version: &str,
    configuration_info: Option<ConfigurationInfo>,
) -> MutableClusterInfo {
    MutableClusterInfo {
        kafka_version: Some(kafka_version.to_string()),
        configuration_info,
        ..MutableClusterInfo::default()
    }
}

/// The configuration revision a cluster operation reports, as
/// `MutableClusterInfo`.
///
/// `ConfigurationInfo` is the only member an `UpdateClusterConfiguration` call
/// touches, so it is the only one set — the same rule [`storage_cluster_info`]
/// follows.
fn configuration_cluster_info(configuration_info: Option<ConfigurationInfo>) -> MutableClusterInfo {
    MutableClusterInfo {
        configuration_info,
        ..MutableClusterInfo::default()
    }
}

/// The security attributes a cluster operation reports, as `MutableClusterInfo`:
/// the client authentication the cluster accepts and the encryption it runs
/// with.
///
/// Those are what a security update touches, so they are the only members set,
/// and the encryption is reported resolved rather than as submitted — the same
/// rule [`storage_cluster_info`] follows.
fn security_cluster_info(
    client_authentication: Option<ClientAuthentication>,
    encryption_info: EncryptionInfo,
) -> MutableClusterInfo {
    MutableClusterInfo {
        client_authentication,
        encryption_info: Some(encryption_info),
        ..MutableClusterInfo::default()
    }
}

/// The connectivity attributes a cluster operation reports, as
/// `MutableClusterInfo`.
///
/// The broker access configuration and the ZooKeeper access setting are what a
/// connectivity update touches, so they are the only members set, and the
/// ZooKeeper setting is reported resolved rather than absent — the same rule
/// [`storage_cluster_info`] follows.
fn connectivity_cluster_info(
    connectivity_info: Option<ConnectivityInfo>,
    zookeeper_access_enabled: bool,
) -> MutableClusterInfo {
    MutableClusterInfo {
        connectivity_info,
        zookeeper_access: Some(ZookeeperAccess {
            enabled: Some(zookeeper_access_enabled),
        }),
        ..MutableClusterInfo::default()
    }
}

/// The monitoring attributes a cluster operation reports, as
/// `MutableClusterInfo`: the CloudWatch metrics level the cluster gathers, the
/// open-monitoring exporters it runs and the broker log delivery it performs.
///
/// Those are what a monitoring update touches, so they are the only members
/// set, and the exporter switches are reported resolved rather than absent —
/// the same rule [`storage_cluster_info`] follows. `LoggingInfo` is left unset
/// where the cluster delivers no broker logs at all, which is what a cluster
/// created without it does.
fn monitoring_cluster_info(
    enhanced_monitoring: &str,
    open_monitoring: &OpenMonitoringInfo,
    logging_info: Option<LoggingInfo>,
) -> MutableClusterInfo {
    MutableClusterInfo {
        enhanced_monitoring: Some(enhanced_monitoring.to_string()),
        open_monitoring: Some(open_monitoring_report(open_monitoring)),
        logging_info,
        ..MutableClusterInfo::default()
    }
}

/// The `OpenMonitoringInfo` a request submits, as the `OpenMonitoring` shape a
/// reader reports. The two carry the same Prometheus exporter switches under
/// different shape names, and an exporter nobody asked for is off.
fn open_monitoring_report(info: &OpenMonitoringInfo) -> OpenMonitoring {
    OpenMonitoring {
        prometheus: Prometheus {
            jmx_exporter: Some(JmxExporter {
                enabled_in_broker: info
                    .prometheus
                    .jmx_exporter
                    .as_ref()
                    .is_some_and(|e| e.enabled_in_broker),
            }),
            node_exporter: Some(NodeExporter {
                enabled_in_broker: info
                    .prometheus
                    .node_exporter
                    .as_ref()
                    .is_some_and(|e| e.enabled_in_broker),
            }),
        },
    }
}

/// The stored `OpenMonitoringInfo` with every exporter switch resolved, which is
/// the shape the `Provisioned` detail reports. Same normalisation as
/// [`open_monitoring_report`] in the shape a request submits: an exporter nobody
/// asked for is reported off rather than absent, so a reader never leaves the
/// caller guessing which default applied.
fn open_monitoring_effective(info: &OpenMonitoringInfo) -> OpenMonitoringInfo {
    OpenMonitoringInfo {
        prometheus: PrometheusInfo {
            jmx_exporter: Some(JmxExporterInfo {
                enabled_in_broker: info
                    .prometheus
                    .jmx_exporter
                    .as_ref()
                    .is_some_and(|e| e.enabled_in_broker),
            }),
            node_exporter: Some(NodeExporterInfo {
                enabled_in_broker: info
                    .prometheus
                    .node_exporter
                    .as_ref()
                    .is_some_and(|e| e.enabled_in_broker),
            }),
        },
    }
}

/// An instant as the string a `__string` timestamp member carries: ISO-8601
/// (RFC 3339), the form every modelled timestamp of this service travels in.
/// `None` only for an instant no date-time can represent, which no stored record
/// holds.
fn rendered_instant(at: SystemTime) -> Option<String> {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// An EC2 long resource id for `kind` (`"eni"`): the prefix, a hyphen and 17
/// lowercase hexadecimal digits — the longer of the two published EC2
/// resource-id forms, which is what [`is_ec2_resource_id`] accepts. A pure
/// function of `seed`, so the resource it names keeps one id.
fn ec2_long_id(kind: &str, seed: &str) -> String {
    use std::fmt::Write;

    let digest = stable_digest(seed);
    let mut id = format!("{kind}-");
    // Nine bytes render 18 digits; the id takes the first 17 of them.
    for byte in digest.iter().take(9) {
        // Infallible: `{:02x}` raises no formatting error, and a `String` is the
        // one `fmt::Write` that cannot fail for any other reason.
        let _ = write!(id, "{byte:02x}");
    }
    id.truncate(kind.len() + 1 + 17);
    id
}

/// The private IPv4 address a network interface this service creates answers on,
/// as a pure function of `seed` so the interface keeps one address.
///
/// CHOSEN, not documented: the real service allocates from the client subnet's
/// CIDR, and this emulator holds no VPC inventory to read a CIDR from. The
/// address is drawn from 10.0.0.0/8 — a private range (RFC 1918) — with a host
/// octet in 1..=254, so it is never a network or broadcast address.
fn private_ipv4(seed: &str) -> String {
    let digest = stable_digest(seed);
    let host = 1 + u32::from(digest[2]) % 254;
    format!("10.{}.{}.{host}", digest[0], digest[1])
}

/// 32 digest bytes for `seed`, from the hasher the standard library ships
/// (`std::hash`), used four times with the output word's index mixed in so the
/// four 64-bit words are independent functions of the seed rather than four
/// copies of one.
///
/// This is an IDENTIFIER seed, not a security primitive: what the ids below need
/// is that one seed always maps to one id within a running emulator and that
/// distinct seeds map apart, which is exactly what a hasher gives. No
/// cryptographic claim is made and none is needed — nothing here authenticates
/// anything — and no hashing crate is pulled in for it. Raw bytes rather than a
/// hex string, so a consumer that folds a byte into an alphabet draws on all 256
/// values instead of the 16 a hex digit can take.
fn stable_digest(seed: &str) -> [u8; 32] {
    use std::hash::{Hash, Hasher};

    let mut digest = [0u8; 32];
    for (index, word) in digest.chunks_mut(8).enumerate() {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        index.hash(&mut hasher);
        seed.hash(&mut hasher);
        word.copy_from_slice(&hasher.finish().to_be_bytes());
    }
    digest
}

/// The first 16 digest bytes rendered as a UUID: the RFC 4122 version (4) and
/// variant bits are set and the 8-4-4-4-12 form is written out, so an id this
/// emulator mints is SHAPED exactly like the one the real service puts in an MSK
/// ARN. The name says shaped rather than random because the bytes come from the
/// digest, so the id stays a pure function of its seed.
fn uuid_v4_shaped_from_digest(digest: &[u8; 32]) -> String {
    use std::fmt::Write;

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version goes in the high nibble of byte 6 and the
    // variant in the two high bits of byte 8.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let mut rendered = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        // The hyphens of the 8-4-4-4-12 grouping fall after bytes 4, 6, 8 and 10.
        if matches!(index, 4 | 6 | 8 | 10) {
            rendered.push('-');
        }
        // Infallible: the only error a `fmt::Write` to a `String` can report is
        // one the formatter itself raises, and `{:02x}` raises none.
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

/// An MSK `CurrentVersion` token: `K` followed by 13 upper-case alphanumerics,
/// the form the real service's cluster version takes. It is a function of the
/// cluster ARN and its revision, so it changes exactly when the cluster does.
fn current_version_token(arn: &str, revision: u64) -> String {
    let digest = stable_digest(&format!("{arn}#{revision}"));
    let mut token = String::with_capacity(CURRENT_VERSION_TOKEN_LEN);
    token.push(CURRENT_VERSION_TOKEN_PREFIX);
    for byte in digest.iter().take(CURRENT_VERSION_TOKEN_LEN - 1) {
        token.push(CURRENT_VERSION_TOKEN_ALPHABET[usize::from(*byte) % 36] as char);
    }
    token
}

/// The character every [`current_version_token`] starts with.
const CURRENT_VERSION_TOKEN_PREFIX: char = 'K';

/// How many characters one carries in all.
const CURRENT_VERSION_TOKEN_LEN: usize = 14;

/// The characters its tail is drawn from.
const CURRENT_VERSION_TOKEN_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Whether `value` is shaped like a version token this service mints
/// ([`current_version_token`]).
///
/// A `CurrentVersion` is a token the SERVICE hands out and the caller quotes
/// back, so a value outside that shape is not a version of any resource — it is
/// a parameter the service will not read, which is a different refusal from
/// quoting a well-formed token the resource is not at
/// ([`CLUSTER_VERSION_NOT_CURRENT_MESSAGE`]): the first is answered from the
/// request alone, the second only against the stored record. The same
/// distinction this module draws between an ARN that is not one
/// ([`malformed_arn`]) and a well-formed ARN nothing holds.
pub fn is_current_version_token(value: &str) -> bool {
    let mut characters = value.chars();
    characters.next() == Some(CURRENT_VERSION_TOKEN_PREFIX)
        && value.len() == CURRENT_VERSION_TOKEN_LEN
        && characters.all(|character| {
            CURRENT_VERSION_TOKEN_ALPHABET.contains(&u8::try_from(character).unwrap_or_default())
        })
}

/// Encode the registry key a page stopped at as a pagination token, marked with
/// `prefix` so a token minted for one list cannot be mistaken for another's.
///
/// Base64 of a marked plaintext: the token is ENCODED, not authenticated, so a
/// caller could decode it or mint one. That is harmless here — the key inside is
/// only used as a `>` comparison against the keys of the caller's own registry,
/// so a forged resume point can only skip or repeat entries the caller may
/// already list.
pub fn encode_page_token(prefix: &str, last_key: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{prefix}{last_key}"))
}

/// The registry key a token resumes after, or `None` when the token is not one
/// `prefix` was minted with.
pub fn decode_page_token(prefix: &str, token: &str) -> Option<String> {
    let decoded = ::aws_smithy_types::base64::decode(token).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    decoded.strip_prefix(prefix).map(ToString::to_string)
}

/// Separates the three fields a cluster-operation listing's token carries: the
/// cluster ARN it was minted for, the millisecond the page's last operation was
/// filed at, and that operation's own ARN. Neither an MSK ARN nor a decimal
/// number can contain it, so the plaintext splits back into exactly three
/// fields.
const CLUSTER_OPERATION_TOKEN_SEPARATOR: char = '#';

/// Encode the cluster and the [`ClusterOperation::history_key`] a page of that
/// cluster's operation history stopped at, through the one page-token codec of
/// this service ([`encode_page_token`]) under `prefix`.
///
/// The cluster is INSIDE the plaintext, so a token minted from one cluster's
/// history cannot resume another's, and `prefix` is the minting listing's own,
/// so neither can a token minted by a different listing of the same history.
pub fn encode_cluster_operation_page_token(
    prefix: &str,
    cluster_arn: &str,
    last: (std::cmp::Reverse<u128>, &str),
) -> String {
    let (std::cmp::Reverse(millis), operation_arn) = last;
    encode_page_token(
        prefix,
        &format!(
            "{cluster_arn}{CLUSTER_OPERATION_TOKEN_SEPARATOR}{millis}\
             {CLUSTER_OPERATION_TOKEN_SEPARATOR}{operation_arn}"
        ),
    )
}

/// The history key a token resumes after, or `None` when the token is not one
/// `prefix` minted for the history of the cluster `cluster_arn` names — the
/// inverse of [`encode_cluster_operation_page_token`], so a listing resumes by
/// comparing the SAME key its page order is built from.
pub fn decode_cluster_operation_page_token(
    prefix: &str,
    token: &str,
    cluster_arn: &str,
) -> Option<(std::cmp::Reverse<u128>, String)> {
    let plaintext = decode_page_token(prefix, token)?;
    let (arn, rest) = plaintext.split_once(CLUSTER_OPERATION_TOKEN_SEPARATOR)?;
    if arn != cluster_arn {
        return None;
    }
    let (millis, operation_arn) = rest.split_once(CLUSTER_OPERATION_TOKEN_SEPARATOR)?;
    Some((
        std::cmp::Reverse(millis.parse::<u128>().ok()?),
        operation_arn.to_string(),
    ))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKafkaKafkaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKafkaKafkaState>)
}

/// The signing region of the request being served, which is the region every ARN
/// and endpoint this service mints belongs to.
///
/// The region travels in the SigV4 credential scope. A request that carries no
/// usable `Authorization` header is served in `us-east-1`, the region a generated
/// emulator defaults to.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// The resource type and id path of an MSK resource ARN —
/// `arn:<partition>:kafka:<region>:<account>:<type>/<id-path>` — or `None` when
/// `value` is not one.
///
/// Built on [`kafka_arn_resource`], the one ARN parse this module has, so an
/// operation that takes ANY resource ARN (the tagging operations) draws the
/// "this is not an ARN of mine" line exactly where the `is_*_arn` predicates
/// draw it.
pub fn kafka_resource_arn_parts(value: &str) -> Option<(&str, &str)> {
    let resource = kafka_arn_resource(value)?;
    let (resource_type, id_path) = resource.split_once('/')?;
    (!resource_type.is_empty() && !id_path.is_empty()).then_some((resource_type, id_path))
}

/// The resource type `value` names, for an operation that takes any MSK resource
/// ARN.
///
/// A value that is not an ARN of this service, or one whose type this service
/// mints in a shape `value` does not have, is [`TagTargetRejected::NotAnArn`] —
/// the request the service cannot parse, as distinct from the resource it cannot
/// find. The shape check goes through the same `is_*_arn` predicate the readers
/// of that resource type use, so one value is never a bad request to one
/// operation and a miss to another.
fn resource_type_of_arn(value: &str) -> Result<&str, TagTargetRejected> {
    let Some((resource_type, _)) = kafka_resource_arn_parts(value) else {
        return Err(TagTargetRejected::NotAnArn);
    };
    match resource_type {
        CLUSTER_RESOURCE_TYPE if !is_cluster_arn(value) => Err(TagTargetRejected::NotAnArn),
        CHANNEL_RESOURCE_TYPE if !is_channel_arn(value) => Err(TagTargetRejected::NotAnArn),
        REPLICATOR_RESOURCE_TYPE if !is_replicator_arn(value) => {
            Err(TagTargetRejected::NotAnArn)
        }
        other => Ok(other),
    }
}

/// Whether `value` is shaped like an MSK cluster ARN —
/// `arn:<partition>:kafka:<region>:<account>:cluster/<name>/<id>`.
///
/// A reader that models both errors answers `BadRequestException` for a value
/// that is not an ARN of this kind, and reserves `NotFoundException` for a
/// well-formed ARN no live cluster holds: that is the distinction the real
/// service draws between a request it cannot parse and a resource it cannot find.
pub fn is_cluster_arn(value: &str) -> bool {
    is_kafka_arn_for(value, "cluster")
}

/// The input member the cluster-scoped operations carry their cluster's ARN in,
/// named as the API documents it on the wire — the camelCase spelling the
/// request itself uses, not the Smithy structure member's identifier. What those
/// operations' refusals report as the parameter at fault, beside
/// [`REPLICATOR_ARN_MEMBER`].
pub const CLUSTER_ARN_MEMBER: &str = "clusterArn";

/// The member `CreateVpcConnection` carries the cluster's ARN in, under the same
/// rule: that operation names the cluster it connects to `TargetClusterArn`, so
/// its refusal reports that member's wire name rather than the one every
/// cluster-scoped operation binds.
pub const TARGET_CLUSTER_ARN_MEMBER: &str = "targetClusterArn";

/// The member the readers of one cluster OPERATION carry their target's ARN in,
/// named the same way.
pub const CLUSTER_OPERATION_ARN_MEMBER: &str = "clusterOperationArn";

/// The member an operation acting on one client VPC CONNECTION of a cluster
/// carries that connection's ARN in, under the same rule: the wire name of
/// `RejectClientVpcConnectionRequest$VpcConnectionArn`.
pub const VPC_CONNECTION_ARN_MEMBER: &str = "vpcConnectionArn";

/// The cluster-ARN-SPECIFIC refusal wording, which the SCRAM-secret surface and
/// the client-VPC-connection listing answer a cluster ARN they cannot parse with
/// instead of the generic [`BAD_REQUEST_MESSAGE`] (the apostrophe is U+2019, as
/// the service writes it).
///
/// Fixed text end to end, like the generic one: the offending value is not part
/// of it. Which of the two an operation answers is the operation's, so the two
/// are named separately here rather than evened out — a caller of one surface is
/// told what that surface tells callers.
pub const CLUSTER_ARN_NOT_VALID_MESSAGE: &str =
    "The specified cluster ARN isn\u{2019}t valid. Specify a valid cluster ARN, and try again.";

/// The refusal an operation answers a value that is not shaped like the kind of
/// ARN the member naming it carries ([`is_cluster_arn`],
/// [`is_cluster_operation_arn`]): a `BadRequestException` (HTTP 400) whose
/// message is the service's generic parameter sentence
/// ([`BAD_REQUEST_MESSAGE`]) — the WHOLE text, with no part taken from the
/// request, so two calls carrying different unusable values are told the same
/// thing and the offending value is never echoed back — and which names the
/// member at fault in `invalidParameter`, by its wire name
/// ([`CLUSTER_ARN_MEMBER`], [`TARGET_CLUSTER_ARN_MEMBER`],
/// [`CLUSTER_OPERATION_ARN_MEMBER`]).
///
/// Built HERE rather than beside each shape check, so no handler composes a
/// sentence of its own around the ARN it was handed: one condition, one
/// envelope, whichever operation met it.
pub fn malformed_arn(member: &str) -> BadRequestException {
    parameter_refusal(member, BAD_REQUEST_MESSAGE)
}

/// The same refusal for the surfaces that answer the cluster-ARN-specific
/// wording ([`CLUSTER_ARN_NOT_VALID_MESSAGE`]): the member at fault is the
/// cluster's own ARN, which is what that sentence is about.
pub fn invalid_cluster_arn() -> BadRequestException {
    parameter_refusal(CLUSTER_ARN_MEMBER, CLUSTER_ARN_NOT_VALID_MESSAGE)
}

/// The sentence the REPLICATOR surface answers a value its member cannot carry
/// with: fixed text end to end, so the value the request sent is never echoed
/// back and two calls carrying different unusable values are told the same thing.
///
/// A sentence of the service's own, beside the generic [`BAD_REQUEST_MESSAGE`]
/// the cluster surface answers and the cluster-specific
/// [`CLUSTER_ARN_NOT_VALID_MESSAGE`]: which of them a caller gets is the
/// operation's, so they are named separately here rather than evened out.
pub const VALUE_NOT_VALID_MESSAGE: &str = "The provided value is not valid.";

/// The refusal every replicator operation answers a value that is not shaped like
/// a replicator ARN ([`is_replicator_arn`]) with: a `BadRequestException` (HTTP
/// 400) carrying [`VALUE_NOT_VALID_MESSAGE`] and naming
/// [`REPLICATOR_ARN_MEMBER`] in `invalidParameter`.
///
/// Built HERE, like [`malformed_arn`], so the readers, the delete and the
/// replication update cannot drift apart on how an unusable replicator reference
/// is reported: one condition, one envelope, whichever operation met it. A
/// well-formed ARN no live replicator holds is the DIFFERENT refusal those
/// operations answer with [`parameter_not_found_message`].
pub fn invalid_replicator_arn() -> BadRequestException {
    parameter_refusal(REPLICATOR_ARN_MEMBER, VALUE_NOT_VALID_MESSAGE)
}

/// The sentence an operation that models NO `NotFoundException` answers a
/// well-formed resource ARN this endpoint's region holds nothing for with.
///
/// The SAME fixed, request-independent sentence [`malformed_arn`] carries
/// ([`BAD_REQUEST_MESSAGE`]): both are the one thing this service says about a
/// parameter it cannot act on, and neither takes any part of its text from the
/// request — not the value, not the region the endpoint serves. Operations that
/// DO model `NotFoundException` answer [`RESOURCE_NOT_FOUND_MESSAGE`] instead,
/// which is that error's own sentence.
///
/// Named here, beside the shape refusal, so no handler composes a sentence of
/// its own around the reference it was handed.
pub const UNRESOLVED_REFERENCE_MESSAGE: &str = BAD_REQUEST_MESSAGE;

/// One `BadRequestException` naming a request member and carrying one of the
/// service's fixed sentences, so the two refusals above cannot drift apart on
/// how a parameter is named or on the fact that the message carries nothing of
/// the request.
pub(crate) fn parameter_refusal(member: &str, message: &str) -> BadRequestException {
    BadRequestException {
        invalid_parameter: Some(member.to_string()),
        message: Some(message.to_string()),
    }
}

/// Whether `value` is shaped like an MSK replicator ARN —
/// `arn:<partition>:kafka:<region>:<account>:replicator/<name>/<id>`.
///
/// Same distinction, same reason, as [`is_cluster_arn`]: a value that is not an
/// ARN of this kind is a `BadRequestException`, and a well-formed ARN no live
/// replicator holds is a `NotFoundException`.
pub fn is_replicator_arn(value: &str) -> bool {
    is_kafka_arn_for(value, "replicator")
}

/// Whether `value` is an AWS Secrets Manager secret ARN —
/// `arn:<partition>:secretsmanager:<region>:<account>:secret:<name>`, the form
/// `SecretArnList` carries ("List of AWS Secrets Manager secret ARNs") and the
/// same requirement the shared ARN vocabulary states for such a member: the
/// vendor field is `secretsmanager` and the resource names a `secret`.
///
/// Read through [`arn_resource_of`], the one ARN split of this module, so this
/// predicate and the ones over this service's own ARNs cannot disagree about the
/// fields in front of the resource. The same distinction between the two client
/// errors as [`is_cluster_arn`]: a value that is not a secret ARN at all is a
/// request the service cannot act on (`BadRequestException`), not a resource it
/// cannot find.
pub fn is_secrets_manager_secret_arn(value: &str) -> bool {
    let Some(resource) = arn_resource_of(value, "secretsmanager") else {
        return false;
    };
    // A secret's resource field is `secret:<name>`; a name may itself carry ':'
    // and '/', so only the first separator ends the type and the rest is the
    // name, which must not be empty — an ARN naming no secret names none.
    resource
        .strip_prefix("secret:")
        .is_some_and(|name| !name.is_empty())
}

/// The sentence Amazon MSK answers a `SecretArnList` entry that is not an AWS
/// Secrets Manager secret ARN at all with: fixed text end to end, with the
/// offending value NOT echoed back, like this service's other parameter
/// refusals. Shared by the two SCRAM batch operations, so one condition is
/// stated one way whichever of them met it.
pub const SECRET_ARN_INVALID_MESSAGE: &str = "The provided secret ARN is invalid.";

/// Whether `value`'s resource part is `<kind>/<name>/<id>` — the shape a
/// cluster, a configuration and a replicator ARN all take, so the predicates
/// above cannot disagree about what a well-formed ARN of their kind is.
fn is_kafka_arn_for(value: &str, kind: &str) -> bool {
    let Some(resource) = kafka_arn_resource(value) else {
        return false;
    };
    let mut segments = resource.split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next()
        ),
        (Some(found), Some(name), Some(id), None)
            if found == kind && !name.is_empty() && !id.is_empty()
    )
}

/// Whether `value` is shaped like an MSK VPC-connection ARN —
/// `arn:<partition>:kafka:<region>:<account>:vpc-connection/<cluster-owner-account>/<cluster-name>/<id>`,
/// the form [`VpcConnection::requested`] mints and the one the real service
/// puts on a multi-VPC connection.
///
/// Same split of the two client errors as [`is_cluster_arn`]: a value of any
/// other shape is a request the service cannot parse, not a resource it cannot
/// find.
pub fn is_vpc_connection_arn(value: &str) -> bool {
    let Some(resource) = kafka_arn_resource(value) else {
        return false;
    };
    let mut segments = resource.split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next()
        ),
        (Some("vpc-connection"), Some(owner), Some(cluster), Some(id), None)
            if !owner.is_empty() && !cluster.is_empty() && !id.is_empty()
    )
}

/// Whether `value` is shaped like an MSK cluster-operation ARN —
/// `arn:<partition>:kafka:<region>:<account>:cluster-operation/<cluster
/// name>/<cluster id>/<operation id>`, the form
/// [`Cluster::operation_arn`] mints and `ClusterOperationArn` carries.
///
/// Same distinction as above: a value that is not an ARN of this kind is a bad
/// request, while a well-formed one the registry does not hold is a resource
/// that cannot be found.
pub fn is_cluster_operation_arn(value: &str) -> bool {
    let Some(resource) = kafka_arn_resource(value) else {
        return false;
    };
    let mut segments = resource.split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next()
        ),
        (
            Some("cluster-operation"),
            Some(name),
            Some(cluster_id),
            Some(operation_id),
            None
        ) if !name.is_empty() && !cluster_id.is_empty() && !operation_id.is_empty()
    )
}

/// Whether `value` is shaped like an MSK channel ARN —
/// `arn:<partition>:kafka:<region>:<account>:channel/<cluster>/<cluster-id>/<name>`,
/// optionally followed by the channel's own id: a channel is a sub-resource of
/// the cluster it reads from, so its ARN names that cluster and then the channel.
///
/// The cluster name, the cluster id and the channel name are what identify a
/// channel — a channel name is unique within its cluster — and
/// [`Channel::streaming`] appends the channel's own id after them. BOTH forms are
/// channel ARNs here: the behavioural corpus records the real service answering
/// `NotFoundException`, not `BadRequestException`, for one that stops at the
/// channel name, so that trailing id is not part of what a caller must supply.
///
/// Same division of labour as [`is_cluster_arn`]: a value that is not an ARN of
/// this kind is a `BadRequestException`, and `NotFoundException` is reserved for
/// a well-formed ARN no live channel holds.
pub fn is_channel_arn(value: &str) -> bool {
    let Some(resource) = kafka_arn_resource(value) else {
        return false;
    };
    let mut segments = resource.split('/');
    if segments.next() != Some(CHANNEL_RESOURCE_TYPE) {
        return false;
    }
    let identity: Vec<&str> = segments.collect();
    matches!(identity.len(), 3 | 4) && identity.iter().all(|segment| !segment.is_empty())
}

/// Whether `value` is an EC2 resource id of `kind` (`"vpc"`, `"subnet"`,
/// `"sg"`): the prefix, a hyphen, and then either 8 or 17 lowercase
/// hexadecimal digits — the two published EC2 resource-id forms, the original
/// 8-character one and the longer 17-character one.
///
/// A handler that takes EC2 ids refuses anything else as a bad request: the
/// emulator holds no VPC inventory to look the id up in, but a value that is
/// not an id at all is a request the service cannot act on either way.
pub fn is_ec2_resource_id(kind: &str, value: &str) -> bool {
    let Some(suffix) = value
        .strip_prefix(kind)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// The resource part of `value` when it is an ARN of THIS service in some
/// partition, region and account — `arn:<partition>:kafka:<region>:<account>:`
/// followed by the resource — and `None` otherwise. Borrowed from `value` and
/// split by the caller, so a predicate on the hot path of every reader allocates
/// nothing. The one ARN parse the `is_*_arn` predicates share, so they cannot
/// disagree about the five fields in front of the resource.
fn kafka_arn_resource(value: &str) -> Option<&str> {
    arn_resource_of(value, "kafka")
}

/// The resource part of `value` when it is an ARN of `vendor` in some partition,
/// region and account, and `None` otherwise.
///
/// The ONE ARN split of this module, shared by the predicates over this
/// service's own ARNs and by the one over the Secrets Manager secret ARNs a
/// member of this API carries, so they cannot disagree about the five fields in
/// front of the resource. `splitn` with the field count leaves the whole
/// resource — colons included — as the last field, so an ARN of the
/// `<type>:<id>` form splits exactly like one of the `<type>/<id>` form, and the
/// vendor comparison is exact because an ARN namespace is a fixed lowercase
/// token.
fn arn_resource_of<'a>(value: &'a str, vendor: &str) -> Option<&'a str> {
    let mut fields = value.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(found), Some(region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    if found != vendor || partition.is_empty() || region.is_empty() || account.is_empty() {
        return None;
    }
    Some(resource)
}

/// Whether `c` may appear in a topic or configuration name this service admits.
///
/// ONE character set for the two names a caller supplies and this service turns
/// into a resource identity: ASCII alphanumerics and the three separators `.`,
/// `_` and `-`. It is Apache Kafka's own name character set
/// (`org.apache.kafka.common.internals.Topic.validate`), and the one Amazon MSK
/// admits a configuration name from, so topic names and configuration names are
/// tested against the same predicate rather than two copies of the same list. A
/// cluster name is NOT governed by it: the model bounds `ClusterName`'s length
/// and declares no character set for it.
pub fn is_resource_name_character(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

/// The first character of `name` that is outside the service's name character
/// set, or `None` when every one of them is in it.
///
/// A name reaches the store only through this test: it becomes the resource's
/// identity — it keys a name index and goes into the ARN — so a name carrying
/// anything else names no resource this service could have issued.
pub fn name_character_outside_set(name: &str) -> Option<char> {
    name.chars().find(|c| !is_resource_name_character(*c))
}

/// What the service answers a name built from characters outside its name
/// character set with.
///
/// The register the service's name validator refuses in: it reports that the
/// name cannot be used and echoes no part of the request — not the name, not the
/// offending character — exactly as the same validator's refusal of a name of
/// the wrong length does ([`RESOURCE_NAME_LENGTH_MESSAGE`]). The same sentence
/// for every name the set rejects, since one rule was broken.
pub const RESOURCE_NAME_INVALID_CHARACTERS_MESSAGE: &str =
    "The resource name contains invalid characters";

/// What the service answers a name whose length is outside the bound with.
///
/// ONE sentence for both ends of the one constraint — a name too long and a name
/// too short broke the same rule — and it echoes no part of the request, so a
/// caller is not handed back a name the service refused to register.
pub const RESOURCE_NAME_LENGTH_MESSAGE: &str =
    "The resource name contains too many or too few characters";

/// The shortest and the longest configuration name Amazon MSK registers: a
/// 128-character name is created and read back byte-identical, while 129
/// characters and the empty string are both refused, so the bound is closed at
/// both ends.
pub const MIN_CONFIGURATION_NAME_LENGTH: usize = 1;
pub const MAX_CONFIGURATION_NAME_LENGTH: usize = 128;

/// Whether `name` is of a length this service registers a configuration under.
///
/// Counted in CHARACTERS, not bytes: the bound is on the name a caller reads
/// back, so a multi-byte character counts once — and a name outside it is
/// refused rather than truncated into a resource nobody asked for.
pub fn is_configuration_name_length(name: &str) -> bool {
    (MIN_CONFIGURATION_NAME_LENGTH..=MAX_CONFIGURATION_NAME_LENGTH).contains(&name.chars().count())
}

/// Why `name` is not a legal Apache Kafka topic name, or `None` when it is.
///
/// Apache Kafka's own rule (`org.apache.kafka.common.internals.Topic.validate`),
/// which the MSK topic operations sit in front of: a name is at most
/// [`MAX_TOPIC_NAME_LENGTH`] characters, is neither `.` nor `..`, and is made up
/// only of the service's name characters ([`is_resource_name_character`]). The
/// returned sentence names the member and the constraint it broke, so a refusal
/// says which part of the request was wrong.
pub fn topic_name_error(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("A topic name must not be empty.".to_string());
    }
    if name == "." || name == ".." {
        return Some(format!("The topic name {name} is reserved and cannot be used."));
    }
    let length = name.chars().count();
    if length > MAX_TOPIC_NAME_LENGTH {
        return Some(format!(
            "A topic name is at most {MAX_TOPIC_NAME_LENGTH} characters long, but this one is {length}."
        ));
    }
    if let Some(illegal) = name_character_outside_set(name) {
        return Some(format!(
            "A topic name may contain only ASCII alphanumerics, '.', '_' and '-', but this one contains {illegal:?}."
        ));
    }
    None
}

/// Whether `value` could be an Amazon MSK broker type.
///
/// Two conditions, both from what the model says such a value is:
///
/// * its length is within [`INSTANCE_TYPE_MIN_LEN`]..=[`INSTANCE_TYPE_MAX_LEN`],
///   the `length` trait on the member that STORES it
///   (`BrokerNodeGroupInfo$InstanceType`), which an update's own
///   `TargetInstanceType` (a plain `__string`) does not carry; and
/// * it has the `<prefix>.<family>.<size>` form every MSK broker type takes —
///   `kafka.m5.large` … `kafka.m5.24xlarge` as `BrokerNodeGroupInfo$InstanceType`'s
///   documentation lists them, and the families AWS added after that prose
///   (`kafka.m7g.*`, `express.m7g.*`) alike.
///
/// The FORM is checked rather than the model's prose list, which no longer
/// enumerates every type the service accepts; a value that cannot be a broker
/// type at all is refused with `BadRequestException`.
pub fn is_broker_instance_type(value: &str) -> bool {
    let length = value.chars().count();
    if !(INSTANCE_TYPE_MIN_LEN..=INSTANCE_TYPE_MAX_LEN).contains(&length) {
        return false;
    }
    let mut segments = value.split('.');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next()
        ),
        (Some(prefix), Some(family), Some(size), None)
            if !prefix.is_empty() && !family.is_empty() && !size.is_empty()
    )
}
