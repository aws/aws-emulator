// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-dsql-dsql service.
//!
//! The service's primary resource is the Aurora DSQL cluster, so the store is a
//! map of clusters keyed by their 26-character identifier, plus the two
//! bookkeeping maps the cluster lifecycle needs: the retired (deleted) clusters
//! whose identifiers must never be minted again, and the
//! `clientToken` ledgers that make every `@idempotencyToken`-carrying operation
//! idempotent — each record keyed by the token together with the region of the
//! endpoint that served the call, because a token names a request
//! made at ONE regional endpoint.
//!
//! A change data capture (CDC) stream belongs to exactly one cluster — every
//! stream operation is reached at `/stream/{clusterIdentifier}/...` and a
//! `StreamArn` nests the stream under its cluster's ARN — so a cluster's streams
//! are stored ON the cluster record rather than in a second map beside it: the
//! cluster's deletion takes its streams with it, and no handler can read one of
//! two copies.
//!
//! Besides the data, this module owns the few derivations every cluster handler
//! shares — the request's region, the cluster ARN, the connection endpoint, the
//! encryption details and the modelled "no such resource" errors — so the
//! handlers compute them one way rather than once each. The paginated readers'
//! page walk ([`PageBounds`]) lives here for the same reason: the cursor, the
//! `maxResults` bounds and the rule for when a `nextToken` is issued are one
//! implementation, parameterised by each operation's modelled default and the
//! summary it builds. The answer a served delete keeps for its retry
//! ([`RecordedDelete`]) lives here for the same reason: both deletes record and
//! read back the same members, so the encoding and the refusal of an incomplete
//! record are one implementation, parameterised by the output each handler
//! builds. The answer a served policy write keeps
//! ([`RecordedPolicyWrite`]) lives here on the same terms — both policy writers
//! record and read back the same version member — and so does the answer a
//! call whose only un-derivable member is the status it reported keeps
//! ([`RecordedStatus`]), on the same terms again, and the answer a served
//! `CreateCluster` keeps
//! ([`RecordedClusterCreation`]), as does the
//! refusal every replay path gives for a record that does not hold a member the
//! original call answered ([`recorded_result_unavailable`]), so no two of them
//! can decide that case differently.
//!
//! The checks a request's own input has to pass before anything is written live
//! here for the same reason: a cluster's resource-based policy is attached by two
//! different operations and ends up in ONE record, so the document checks
//! ([`validate_cluster_policy`]) — it has to parse, and unless the caller bypasses
//! the safety check it may not leave nobody able to replace it — are one
//! implementation both of them call, as the witness-Region, peer-list and tag-key
//! checks already are. The identities such a document GRANTS TO are resolved
//! against this account's IAM on the same terms
//! ([`unresolved_policy_principal`], over the principals
//! [`policy_principals`] reads out of it), because a document one entry point would
//! refuse as granting to nobody cannot be reachable through the other. An
//! identifier's outer FORM is the `@pattern` its shape declares, which the
//! boundary constraint walk decides before any handler runs; the preconditions
//! BEYOND that pattern live here, so no two operations that state one can word it
//! differently — the minted value RANGE three cluster-addressed operations state
//! as their own ([`validate_minted_cluster_identifier`]), and the base32 encoding
//! a stream id is written in, which every operation addressed at a stream requires
//! of its `streamIdentifier` label ([`validate_stream_identifier`]). A value that
//! satisfies the precondition an operation states is resolved against the store
//! like any other.
//!
//! The tagging operations address a resource by a caller-supplied `resourceArn`
//! rather than by identifier, so the read of that ARN ([`parse_resource_arn`]), the
//! check that the ARN is the one this endpoint serves and the one its resource was
//! minted with ([`ArnResource::is_scoped_to`] / [`ArnResource::is_arn_of`]), the tags
//! it resolves to ([`ComAmazonawsDsqlDsqlState::resource_tags_mut`]) and the refusal
//! of a value that names no resource of this service ([`invalid_resource_arn`]) live
//! here too, for the same reason.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::sha2::{Digest, Sha256};

use crate::ops::types::{
    ConflictException, EncryptionDetails, InternalServerException, MultiRegionProperties,
    ResourceNotFoundException, ServiceQuotaExceededException, StatusReason, TargetDefinition,
    ValidationException, ValidationExceptionField,
};

/// Whether the customer managed key a cluster's `kmsEncryptionKey` names EXISTS —
/// the question every operation that accepts that member asks, of the component
/// that owns keys, before it records an encryption configuration.
pub mod kms_key;

/// WHERE a cluster's connection endpoint is announced — the address this emulator is
/// served at, which is the only address a client handed the member can reach.
pub mod cluster_addressing;
pub use cluster_addressing::*;

/// The CREDENTIAL a session on a cluster's PostgreSQL endpoint presents — an auth
/// token — and whether the cluster accepts it.
pub mod connection_auth;

/// The DATA plane of one cluster: the PostgreSQL endpoint a client connects to, and
/// what admits a session on it.
pub mod cluster_data_plane;
pub use cluster_data_plane::ClusterDataPlane;

/// The ADVANCED MONITORING a cluster collects: the metric groups enabled on it, as a
/// create asks for them, an update replaces them and a read reports them.
pub mod metric_groups;
pub use metric_groups::*;

/// The `ClusterStatus` values this service drives a cluster through, spelled as
/// the enum spells them on the wire (`smithy_query(query="shape",
/// shape="ClusterStatus")`: CREATING | ACTIVE | IDLE | INACTIVE | UPDATING |
/// DELETING | DELETED | FAILED | PENDING_SETUP | PENDING_DELETE).
pub const STATUS_CREATING: &str = "CREATING";
/// See [`STATUS_CREATING`].
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// See [`STATUS_CREATING`].
pub const STATUS_IDLE: &str = "IDLE";
/// See [`STATUS_CREATING`].
pub const STATUS_UPDATING: &str = "UPDATING";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETING: &str = "DELETING";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETED: &str = "DELETED";
/// See [`STATUS_CREATING`]: the status of a multi-Region cluster whose peering is
/// not established on both sides yet, so the cluster is not usable
/// ([`multi_region_setup_complete`]).
pub const STATUS_PENDING_SETUP: &str = "PENDING_SETUP";
/// See [`STATUS_CREATING`]: the status of a multi-Region cluster whose deletion is
/// unwinding its peering, before the removal itself starts.
pub const STATUS_PENDING_DELETE: &str = "PENDING_DELETE";

/// The `StreamStatus` values a stream is driven through, spelled as the enum
/// spells them on the wire (`smithy_query(query="shape", shape="StreamStatus")`:
/// ACTIVE | CREATING | DELETED | DELETING | FAILED | IMPAIRED).
pub const STREAM_STATUS_CREATING: &str = "CREATING";
/// See [`STREAM_STATUS_CREATING`].
pub const STREAM_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`STREAM_STATUS_CREATING`]: the status of a stream whose deletion has
/// started and which the clock has yet to remove.
pub const STREAM_STATUS_DELETING: &str = "DELETING";
/// See [`STREAM_STATUS_CREATING`]: the status a deleted stream reaches, at which
/// point it is no longer held by its cluster ([`ComAmazonawsDsqlDsqlState::settle`]).
pub const STREAM_STATUS_DELETED: &str = "DELETED";
/// See [`STREAM_STATUS_CREATING`]: the status of a stream whose delivery target
/// was found unusable, so no database change reaches it. `GetStream` reports the
/// [`Stream::failure`] behind it as the `statusReason`.
pub const STREAM_STATUS_FAILED: &str = "FAILED";

/// The `StreamFailureErrorCode` values a stream's `statusReason` reports, spelled
/// as the enum spells them on the wire (`smithy_query(query="shape",
/// shape="StreamFailureErrorCode")`: KINESIS_THROUGHPUT_EXCEEDED |
/// KINESIS_STREAM_NOT_FOUND | ROLE_ACCESS_DENIED | KINESIS_ACCESS_DENIED |
/// KINESIS_KMS_ACCESS_DENIED | KINESIS_OVERSIZE_RECORD | CLUSTER_CMK_INACCESSIBLE
/// | INTERNAL_ERROR).
///
/// The two below are the ones the service that OWNS a stream's DESTINATION answers
/// about it: the delivery service reporting that it holds no such stream, or that it
/// refuses access to it. The rest are about DELIVERING a record (a throughput
/// ceiling, an oversize record), about a store this service does not ask, or about a
/// delivery role — which is resolved at request time, so a stream is never created
/// against a role this service cannot assume
/// (`crate::ops::create_stream`) — and no answer grounds them, so nothing reports
/// them.
pub const STREAM_FAILURE_KINESIS_STREAM_NOT_FOUND: &str = "KINESIS_STREAM_NOT_FOUND";
/// See [`STREAM_FAILURE_KINESIS_STREAM_NOT_FOUND`].
pub const STREAM_FAILURE_KINESIS_ACCESS_DENIED: &str = "KINESIS_ACCESS_DENIED";

/// `EncryptionType` values (`smithy_query(query="shape", shape="EncryptionType")`).
pub const ENCRYPTION_AWS_OWNED: &str = "AWS_OWNED_KMS_KEY";
/// See [`ENCRYPTION_AWS_OWNED`].
pub const ENCRYPTION_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED_KMS_KEY";
/// `EncryptionStatus` value for a cluster whose encryption is in place
/// (`smithy_query(query="shape", shape="EncryptionStatus")`: ENABLED | UPDATING |
/// KMS_KEY_INACCESSIBLE | ENABLING).
pub const ENCRYPTION_STATUS_ENABLED: &str = "ENABLED";
/// See [`ENCRYPTION_STATUS_ENABLED`]: the status of a cluster whose encryption is
/// being established for the first time — the window the cluster itself is being
/// created in, before any data of it is protected by the configuration it asked
/// for.
pub const ENCRYPTION_STATUS_ENABLING: &str = "ENABLING";
/// See [`ENCRYPTION_STATUS_ENABLED`]: the status of a cluster whose encryption
/// configuration is being CHANGED to another one, reported for as long as the
/// change is in flight, so a client can poll for it to finish.
pub const ENCRYPTION_STATUS_UPDATING: &str = "UPDATING";

/// The region an ARN and an endpoint are scoped to when the request carries no
/// SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The service segment of every ARN this service mints and resolves
/// (`ClusterArn`'s pattern fixes it: `^arn:aws(-[^:]+)?:dsql:...$`).
pub const ARN_SERVICE: &str = "dsql";

/// The partition segment of every ARN this emulator mints: `::engine::emulator_arn`
/// builds `arn:aws:{service}:...`, so a resource this emulator holds is always in
/// this partition. `ClusterArn`'s pattern admits others (`aws(-[^:]+)?`), so an ARN
/// naming one of them reaches a handler and names a resource held elsewhere.
pub const ARN_PARTITION: &str = "aws";

/// The resource-type segment of a cluster ARN (`...:cluster/{identifier}`), and
/// the `resourceType` a cluster's `ResourceNotFoundException` carries.
pub const RESOURCE_TYPE_CLUSTER: &str = "cluster";

/// The resource-type segment of a stream ARN
/// (`...:cluster/{cluster}/stream/{stream}`), and the `resourceType` a stream's
/// `ResourceNotFoundException` carries.
pub const RESOURCE_TYPE_STREAM: &str = "stream";

/// The `resourceType` the `ResourceNotFoundException` of a cluster's
/// resource-based policy carries. A policy has no ARN of its own: it is addressed
/// by the cluster's identifier.
pub const RESOURCE_TYPE_POLICY: &str = "policy";

/// The `resourceType` a refusal about the ACCOUNT itself carries.
///
/// The account is the resource that holds every per-account collection of this
/// service — the clusters an account owns, which is the account segment of every
/// ARN this service mints ([`cluster_arn`]) — so it is the resource a ceiling on
/// such a collection reports, identified by its own 12-digit id.
pub const RESOURCE_TYPE_ACCOUNT: &str = "account";

/// The `ValidationExceptionReason` for a member whose value failed this service's
/// own check (`smithy_query(query="shape", shape="ValidationExceptionReason")`:
/// cannotParse | deletionProtectionEnabled | fieldValidationFailed | other |
/// unknownOperation).
pub const VALIDATION_REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// How long a cluster or a stream stays in a transient status (`CREATING`,
/// `UPDATING`, `DELETING`) before the clock settles it into its terminal one.
///
/// A span this emulator CHOOSES, not a duration the service documents: AWS gives
/// no timing contract for cluster or stream creation. It is deliberately
/// non-zero so a caller that acts immediately after `CreateCluster` or
/// `CreateStream` still observes `CREATING` and the state errors that go with it.
pub const SETTLE_AFTER: Duration = Duration::from_millis(750);

/// A status change the clock owes a cluster or a stream: at `due_at` it becomes
/// `next_status`, whether or not anybody calls.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The instant the change became (or becomes) due.
    pub due_at: SystemTime,
    /// The `ClusterStatus` (or `StreamStatus`) the resource holds from `due_at` on.
    pub next_status: &'static str,
}

/// The resource-based policy attached to a cluster: the document the caller
/// supplied and the version token `PutClusterPolicy` answered with.
#[derive(Debug, Clone)]
pub struct ClusterPolicy {
    /// The `PolicyDocument` verbatim.
    pub document: String,
    /// The `PolicyVersion` token this document was stored under.
    pub version: String,
}

/// One persisted change data capture (CDC) stream of a cluster.
#[derive(Debug, Clone)]
pub struct Stream {
    /// The 26-character lowercase identifier (`StreamId`).
    pub stream_identifier: String,
    /// The stream ARN, as [`stream_arn`] builds it.
    pub arn: String,
    /// The current `StreamStatus`.
    pub status: String,
    /// When `CreateStream` minted this stream.
    pub creation_time: SystemTime,
    /// The `StreamOrdering` the caller asked for.
    pub ordering: String,
    /// The `StreamFormat` the caller asked for.
    pub format: String,
    /// Where the stream writes its records.
    pub target_definition: TargetDefinition,
    /// The `StreamFailureErrorCode` the stream's delivery target was found
    /// unusable with when the stream was created — the answer the service that
    /// OWNS that target gave about it — or `None` when nothing answered that it
    /// is. It is what [`Self::status_reason`] reports once the clock has settled
    /// the stream into [`STREAM_STATUS_FAILED`].
    pub failure: Option<&'static str>,
    /// When this stream's [`Self::status`] last changed, which is the `updatedAt`
    /// a `statusReason` reports. Written only through [`Self::enter_status`], so
    /// the instant and the status it belongs to cannot drift apart.
    pub status_updated_at: SystemTime,
    /// The stream's tags. Sorted so a read is deterministic.
    pub tags: BTreeMap<String, String>,
    /// The status change the clock owes this stream, if any.
    pub pending: Option<PendingTransition>,
}

impl Stream {
    /// Move this stream into `status` as of `at`.
    ///
    /// The ONE writer of [`Self::status`], so the instant the status changed is
    /// recorded with every change rather than by whoever remembers to: that
    /// instant is the `updatedAt` a `statusReason` reports
    /// ([`Self::status_reason`]), and a status written without it would report a
    /// stale one.
    pub fn enter_status(&mut self, status: &str, at: SystemTime) {
        self.status = status.to_string();
        self.status_updated_at = at;
    }

    /// The `statusReason` a read of this stream reports: the failure its delivery
    /// target was found unusable with, beside the instant the status that failure
    /// produced was reached.
    ///
    /// `statusReason` is "stream status reason with error code and timestamp (if
    /// applicable)" (`smithy_query(query="shape", shape="GetStreamOutput",
    /// member="statusReason")`), and what makes it applicable is the stream being
    /// in a status a failure produced — [`STREAM_STATUS_FAILED`] — so a stream in
    /// any other status reports none, whatever its target once answered.
    pub fn status_reason(&self) -> Option<StatusReason> {
        let error = self.failure.filter(|_| self.status == STREAM_STATUS_FAILED)?;
        Some(StatusReason {
            error: error.to_string(),
            updated_at: ::aws_smithy_types::DateTime::from(self.status_updated_at),
        })
    }

    /// Whether the stream is in a status that accepts a mutation. A stream the
    /// clock still owes a transition ([`STREAM_STATUS_CREATING`]) or one already
    /// on its way out ([`STREAM_STATUS_DELETING`], [`STREAM_STATUS_DELETED`]) is
    /// not; every other `StreamStatus` names a stream that exists and is not
    /// changing by itself.
    pub fn accepts_mutation(&self) -> bool {
        !matches!(
            self.status.as_str(),
            STREAM_STATUS_CREATING | STREAM_STATUS_DELETING | STREAM_STATUS_DELETED
        )
    }

    /// Whether this stream's deletion is already under way: its status is a leg
    /// of the deletion chain a caller observes before the stream leaves its
    /// cluster's record. The stream-side counterpart of
    /// [`Cluster::deletion_in_flight`], asked by the delete for the same reason;
    /// a stream's chain has one such leg ([`STREAM_STATUS_DELETING`]), since
    /// [`STREAM_STATUS_DELETED`] is where the stream is removed.
    pub fn deletion_in_flight(&self) -> bool {
        self.status == STREAM_STATUS_DELETING
    }

    /// The modelled refusal a request that MUTATES this stream answers with while
    /// its status does not accept a mutation, or `None` when it does.
    ///
    /// The stream-side counterpart of [`Cluster::mutation_conflict`], and worded
    /// the same way, so neither resource states the condition differently.
    /// `attempted` completes the sentence after the stream's identifier ("be
    /// deleted").
    pub fn mutation_conflict(&self, attempted: &str) -> Option<ConflictException> {
        if self.accepts_mutation() {
            return None;
        }
        Some(
            ResourceIdentity::stream(&self.stream_identifier).conflict(format!(
                "stream {} cannot {attempted} while its status is {}",
                self.stream_identifier, self.status
            )),
        )
    }
}

/// One persisted Aurora DSQL cluster.
///
/// Its connection endpoint is NOT among these members: that value is the address THIS
/// emulator answers on ([`cluster_endpoint`]), so it is derived on every report rather
/// than stored — a host recorded when the cluster was minted would name wherever this
/// process was reachable then, which is not where a later caller has to connect.
#[derive(Debug, Clone)]
pub struct Cluster {
    /// The 26-character lowercase identifier (`ClusterId`).
    pub identifier: String,
    /// The SHORT endpoint-service suffix this cluster's VPC endpoint service is
    /// qualified by, minted ONCE when the cluster was created
    /// ([`ComAmazonawsDsqlDsqlState::mint_vpc_endpoint_service_suffix`]) and
    /// stored here because it is part of the cluster's identity rather than of
    /// the call that reads it: `GetVpcEndpointServiceName` names the endpoint
    /// service from it ([`vpc_endpoint_service_name`]), so two reads of one
    /// cluster name one endpoint service. It is not derived from
    /// [`Self::identifier`]: the endpoint service has a name of its own, and the
    /// recorded service carries a short token there and not the cluster id.
    pub vpc_endpoint_service_suffix: String,
    /// The cluster ARN, as [`cluster_arn`] builds it.
    pub arn: String,
    /// The current `ClusterStatus`.
    pub status: String,
    /// When `CreateCluster` minted this cluster.
    pub creation_time: SystemTime,
    /// Whether the cluster currently refuses deletion.
    pub deletion_protection_enabled: bool,
    /// The customer-managed `kmsEncryptionKey` the caller supplied, verbatim, if
    /// the cluster has one. `None` when the cluster is on the key owned by Amazon
    /// Web Services — whether because no key was ever supplied or because the
    /// reserved `AWS_OWNED_KMS_KEY` switched it back (see
    /// [`customer_managed_key`]).
    pub kms_encryption_key: Option<String>,
    /// The encryption configuration derived from `kms_encryption_key`.
    pub encryption_details: Option<EncryptionDetails>,
    /// Multi-Region configuration, if the cluster has any.
    pub multi_region_properties: Option<MultiRegionProperties>,
    /// The cluster's tags. Sorted so a read is deterministic.
    pub tags: BTreeMap<String, String>,
    /// The advanced-monitoring metric groups this cluster collects, spelled as
    /// `MetricGroup` spells them on the wire. A SET: the member that enables them is
    /// `@uniqueItems`, so a group is either collected or not and the order a request
    /// named them in carries nothing (see [`metric_groups`]). Empty for a cluster
    /// created without the member, and emptied again by an update that carries an
    /// empty list.
    pub metric_groups: std::collections::BTreeSet<String>,
    /// The resource-based policy currently attached, if any.
    pub policy: Option<ClusterPolicy>,
    /// The cluster's CDC streams, keyed by `StreamId`. Bounded by
    /// [`MAX_STREAMS_PER_CLUSTER`], which `CreateStream` checks before it
    /// inserts.
    pub streams: BTreeMap<String, Stream>,
    /// The status change the clock owes this cluster, if any.
    pub pending: Option<PendingTransition>,
    /// The `EncryptionStatus` change the clock owes this cluster's
    /// `encryptionDetails`, if any: an encryption configuration this cluster is
    /// still establishing ([`ENCRYPTION_STATUS_ENABLING`]) or changing to
    /// ([`ENCRYPTION_STATUS_UPDATING`]) reaches
    /// [`ENCRYPTION_STATUS_ENABLED`] at `due_at`, whether or not anybody calls.
    /// Kept apart from [`Self::pending`] because the configuration's progress is
    /// not the cluster's own status: a cluster waiting for its multi-Region
    /// peering has finished encrypting long before it is usable, and an update
    /// that touches no key leaves the configuration where it was.
    pub encryption_pending: Option<PendingTransition>,
}

impl Cluster {
    /// Whether the cluster is in a status that accepts a mutation. A cluster the
    /// clock still owes a transition (`CREATING`, `UPDATING`) or one already on
    /// its way out (`DELETING`) is not.
    pub fn accepts_mutation(&self) -> bool {
        self.status == STATUS_ACTIVE || self.status == STATUS_IDLE
    }

    /// Whether this cluster's deletion is already under way: its status is one of
    /// the legs the deletion chain reports to a caller before the cluster goes
    /// away — the peering tear-down of a multi-Region cluster
    /// ([`STATUS_PENDING_DELETE`]) and the removal itself ([`STATUS_DELETING`]),
    /// the chain [`ComAmazonawsDsqlDsqlState::owed_after`] drives.
    ///
    /// Asked by the delete, which answers a cluster whose deletion is in flight
    /// with that deletion rather than with the status gate's refusal, whichever
    /// leg of the chain the call lands in. [`STATUS_DELETED`] is not a leg: it is
    /// the status at which the cluster leaves the store, so no request observes
    /// it.
    pub fn deletion_in_flight(&self) -> bool {
        self.status == STATUS_PENDING_DELETE || self.status == STATUS_DELETING
    }

    /// The modelled refusal a request that MUTATES this cluster answers with
    /// while its status does not accept a mutation, or `None` when it does.
    ///
    /// Every operation that writes a cluster record asks this one question, so
    /// none of them can state the condition differently from
    /// [`Self::accepts_mutation`] or word the refusal differently. `attempted`
    /// completes the sentence after the cluster's identifier ("be updated",
    /// "take a stream"), which is the only part that is about the caller's
    /// request rather than about the cluster.
    pub fn mutation_conflict(&self, attempted: &str) -> Option<ConflictException> {
        if self.accepts_mutation() {
            return None;
        }
        Some(
            ResourceIdentity::cluster(&self.identifier).conflict(format!(
                "cluster {} cannot {attempted} while its status is {}",
                self.identifier, self.status
            )),
        )
    }

    /// The refusal an update of this cluster answers with, or `None` when the
    /// cluster takes it.
    ///
    /// `configures_multi_region_only` says the request asks for NOTHING BUT this
    /// cluster's multi-Region configuration. That one change is the only mutation a
    /// cluster waiting in [`STATUS_PENDING_SETUP`] takes: the peering it is waiting
    /// for is established by configuring BOTH sides, so the configuration change is
    /// the very call the wait is for. The exemption belongs to that change and not
    /// to the request carrying it — a request that asks for anything else asks for
    /// an ordinary mutation of a cluster whose status takes none, so it is refused
    /// however it spells the rest of itself. Every other status answers exactly as
    /// it does to any other mutation ([`Self::mutation_conflict`]).
    pub fn update_conflict(&self, configures_multi_region_only: bool) -> Option<ConflictException> {
        if configures_multi_region_only && self.status == STATUS_PENDING_SETUP {
            return None;
        }
        self.mutation_conflict("be updated")
    }

    /// The peer clusters this cluster's multi-Region configuration names, or an
    /// empty slice when it names none.
    pub fn multi_region_peers(&self) -> &[String] {
        self.multi_region_properties
            .as_ref()
            .and_then(|properties| properties.clusters.as_deref())
            .unwrap_or_default()
    }

    /// Whether this cluster has a multi-Region configuration at all: one naming a
    /// witness Region, peer clusters, or both. A `multiRegionProperties` carrying
    /// neither member asks for no multi-Region cluster, so such a cluster is a
    /// single-Region one.
    pub fn is_multi_region(&self) -> bool {
        self.multi_region_properties.as_ref().is_some_and(|properties| {
            properties
                .witness_region
                .as_deref()
                .is_some_and(|witness| !witness.trim().is_empty())
                || !self.multi_region_peers().is_empty()
        })
    }

    /// Whether this cluster's multi-Region configuration names `peer_arn` as one
    /// of its peer clusters.
    pub fn names_peer(&self, peer_arn: &str) -> bool {
        self.multi_region_peers()
            .iter()
            .any(|peer| peer == peer_arn)
    }

    /// Every resource identifier that ceases to exist when this cluster does:
    /// the cluster itself, and each stream stored on its record — a stream lives
    /// ON the cluster, so removing the cluster removes the stream too. The
    /// identifiers an idempotency record can be keyed by, so
    /// [`ComAmazonawsDsqlDsqlState::forget_tokens_for`] reaches a record keyed by
    /// either kind.
    pub fn removed_identifiers(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.identifier.as_str())
            .chain(self.streams.keys().map(String::as_str))
    }
}

/// The most live clusters this emulator stores for one account.
///
/// A ceiling, not a guess at Aurora DSQL's published quota: a generated emulator
/// keeps every resource in memory for the life of the process, so without a
/// maximum entry count a `CreateCluster` loop grows its heap without bound.
/// `CreateCluster` models `ServiceQuotaExceededException`, which is the refusal
/// once the store is full.
pub const MAX_CLUSTERS: usize = 100;

/// The most retired (deleted) clusters kept as tombstones.
///
/// A tombstone exists only so an identifier this service has already handed out
/// is never minted again; it is never read for anything else. The cap is
/// enforced by evicting the oldest tombstone, so a create/delete loop cannot
/// accumulate records forever even while never holding more than one live
/// cluster.
pub const MAX_RETIRED_CLUSTERS: usize = MAX_CLUSTERS;

/// The most tags one resource holds.
///
/// Not a guess: `TagMap` is modelled `@length(min: 0, max: 200)`
/// (`smithy_query(query="shape", shape="TagMap")`), and every reader of a
/// resource's tags reports them through that shape, so a resource holding more
/// tags than the map admits could not be reported at all. A write that would
/// leave a resource past this ceiling is refused with the tagging operation's
/// own `ServiceQuotaExceededException`.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The most `clientToken` entries one ledger keeps.
///
/// At most one token is recorded per call that carried one, and a token is
/// dropped with the resource it names, so this cap is rarely the binding one; it
/// is enforced anyway, by evicting the oldest token, so a ledger cannot
/// accumulate caller-chosen strings without bound.
pub const MAX_CLIENT_TOKENS: usize = MAX_CLUSTERS;

/// The cap on `CreateCluster`'s ledger, named for the operation; every ledger
/// shares [`MAX_CLIENT_TOKENS`].
pub const MAX_CREATE_TOKENS: usize = MAX_CLIENT_TOKENS;

/// The most CDC streams one cluster holds.
///
/// A ceiling this emulator CHOOSES for the same reason as [`MAX_CLUSTERS`]:
/// every stream lives in memory for the life of the process. `CreateStream`
/// models `ServiceQuotaExceededException`, which is the refusal once a cluster's
/// stream table is full.
pub const MAX_STREAMS_PER_CLUSTER: usize = 20;

/// Which operation's `clientToken`s a ledger holds. One ledger per operation, so
/// the same token used on two different operations is two separate requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenScope {
    /// `CreateCluster.clientToken`.
    CreateCluster,
    /// `UpdateCluster.clientToken`.
    UpdateCluster,
    /// `DeleteCluster.clientToken`.
    DeleteCluster,
    /// `CreateStream.clientToken`.
    CreateStream,
    /// `DeleteStream.clientToken`.
    DeleteStream,
    /// `PutClusterPolicy.clientToken`.
    PutClusterPolicy,
    /// `DeleteClusterPolicy.clientToken`.
    DeleteClusterPolicy,
}

impl TokenScope {
    /// Every scope, in ledger order. The array the state holds is indexed by
    /// this order, so the two cannot drift.
    pub const ALL: [TokenScope; 7] = [
        TokenScope::CreateCluster,
        TokenScope::UpdateCluster,
        TokenScope::DeleteCluster,
        TokenScope::CreateStream,
        TokenScope::DeleteStream,
        TokenScope::PutClusterPolicy,
        TokenScope::DeleteClusterPolicy,
    ];

    /// This scope's slot in the state's ledger array.
    fn index(self) -> usize {
        match self {
            TokenScope::CreateCluster => 0,
            TokenScope::UpdateCluster => 1,
            TokenScope::DeleteCluster => 2,
            TokenScope::CreateStream => 3,
            TokenScope::DeleteStream => 4,
            TokenScope::PutClusterPolicy => 5,
            TokenScope::DeleteClusterPolicy => 6,
        }
    }

    /// Whether a token of this scope stays in its ledger after the resource it
    /// names is gone.
    ///
    /// Every other scope records a call ON a live resource, so the token goes
    /// when the resource goes; the scope of a call that REMOVES a resource
    /// records the removal itself, and a retry of it arrives precisely when the
    /// resource no longer exists, so its record has to outlive it. Such a
    /// ledger is still bounded by [`MAX_CLIENT_TOKENS`].
    fn outlives_its_resource(self) -> bool {
        matches!(self, TokenScope::DeleteCluster | TokenScope::DeleteStream)
    }
}

/// What a `clientToken` the ledger may already hold means for the call carrying
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenVerdict {
    /// The token has not been used on this operation: serve the request.
    Unused,
    /// The token was used with the SAME parameters: this is a retry of that
    /// request, so answer from the call the ledger recorded and have no further
    /// effect.
    Replay(RecordedCall),
    /// The token was used with DIFFERENT parameters: refuse with the
    /// operation's `ConflictException`.
    Mismatch,
}

/// The call a `clientToken` was first used on, as the ledger keeps it: the
/// resource that call acted on, and the values it answered with.
///
/// A retry has to answer with the result of the ORIGINAL call, and the state the
/// service holds when the retry arrives need not still determine that result: a
/// value the first call MINTED can be superseded by a later call or dropped
/// altogether. Such a value is therefore kept here, keyed by the output member
/// it belongs to, so the retry is served from the record rather than
/// re-derived. A value that IS a field of [`Self::resource`]'s stored record
/// needs no copy: the handler renders it from that record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedCall {
    /// The resource that call acted on, as the handler names it — the record a
    /// retry's answer is rendered from, and the resource
    /// [`TokenLedger::forget`] drops the token with.
    pub resource: String,
    /// The values the call answered, keyed by output member name. Sorted so the
    /// record is deterministic.
    answer: BTreeMap<String, String>,
}

impl RecordedCall {
    /// A call against `resource` that answered nothing the resource's own record
    /// does not determine.
    pub fn new(resource: &str) -> Self {
        Self {
            resource: resource.to_string(),
            answer: BTreeMap::new(),
        }
    }

    /// The same call, recording that it answered `member` with `value`.
    pub fn answering(mut self, member: &str, value: &str) -> Self {
        self.answer.insert(member.to_string(), value.to_string());
        self
    }

    /// What the call answered for `member`, or `None` when it recorded no value
    /// for it — which is the case for every member the handler renders from
    /// [`Self::resource`]'s stored record instead ([`Self::answering`] is the
    /// only writer). `None` is never a value to serve: a handler that recorded a
    /// member has to find it, so one that does not must refuse rather than
    /// answer an empty one.
    pub fn answered(&self, member: &str) -> Option<&str> {
        self.answer.get(member).map(String::as_str)
    }
}

/// The sentence a handler refuses a recognised retry with when the record that
/// retry has to be served from does not hold a member the original call
/// answered.
pub const RECORDED_RESULT_UNAVAILABLE_MESSAGE: &str =
    "the recorded result of this request is unavailable";

/// The modelled server error EVERY replay path of this service refuses with when
/// the record it is serving from is missing a member the original call answered.
///
/// [`RecordedCall::answered`] holds a value for exactly the members the handler
/// cannot re-derive, so a handler that finds one absent has no answer to give and
/// nothing it may derive, mint or retire in its place. The decision is the same
/// for every one of them, so the refusal is built here rather than once per
/// handler, and each handler only lifts it into its own error channel.
pub fn recorded_result_unavailable() -> InternalServerException {
    InternalServerException {
        message: RECORDED_RESULT_UNAVAILABLE_MESSAGE.to_string(),
        retry_after_seconds: None,
    }
}

/// One recorded use of a `clientToken`.
#[derive(Debug, Clone)]
struct TokenEntry {
    /// [`request_fingerprint`] of the parameters the first call carried.
    fingerprint: String,
    /// The call that token served.
    call: RecordedCall,
}

/// What one recorded use of a `clientToken` is keyed by: the token itself, and
/// the region of the endpoint the call carrying it was made at.
///
/// A `clientToken` names a request made AT one regional endpoint, and this
/// service is regional: every endpoint serves only the resources minted for its
/// own region ([`ArnResource::is_scoped_to`]), so the earlier requests a token
/// can name are the ones that endpoint served. The same string presented at
/// another region's endpoint names no request there — it is free, exactly as it
/// is before any call carries it — and nothing one region recorded can be
/// replayed as the answer of a call made at another, whose answer would be
/// derived from a region of its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TokenKey {
    /// The region of the endpoint the call was made at ([`request_region`]).
    region: String,
    /// The `clientToken` the call carried, exactly as it carried it:
    /// `ClientToken` is documented case-sensitive.
    token: String,
}

impl TokenKey {
    /// The key a call carrying `token` at `endpoint_region`'s endpoint records
    /// and looks itself up under.
    fn new(endpoint_region: &str, token: &str) -> Self {
        Self {
            region: endpoint_region.to_string(),
            token: token.to_string(),
        }
    }
}

/// The `clientToken`s one operation has already served, each keyed by the token
/// together with the region of the endpoint that served it, and bounded by
/// [`MAX_CLIENT_TOKENS`].
#[derive(Debug, Default)]
pub struct TokenLedger {
    entries: HashMap<TokenKey, TokenEntry>,
    /// Insertion order, so the cap evicts the oldest token rather than an
    /// arbitrary one.
    order: VecDeque<TokenKey>,
}

impl TokenLedger {
    /// What `token` means for a call made at `endpoint_region`'s endpoint
    /// carrying `fingerprint`: a token this ledger holds for ANOTHER region is
    /// not this call's token at all, so it answers [`TokenVerdict::Unused`].
    pub fn verdict(
        &self,
        endpoint_region: &str,
        token: &str,
        fingerprint: &str,
    ) -> TokenVerdict {
        match self.entries.get(&TokenKey::new(endpoint_region, token)) {
            None => TokenVerdict::Unused,
            Some(entry) if entry.fingerprint == fingerprint => {
                TokenVerdict::Replay(entry.call.clone())
            }
            Some(_) => TokenVerdict::Mismatch,
        }
    }

    /// Record that `token`, at `endpoint_region`'s endpoint, served the request
    /// `fingerprint` describes, as `call` describes what it did and answered,
    /// evicting the oldest token first if that would push the ledger past
    /// [`MAX_CLIENT_TOKENS`].
    pub fn record(
        &mut self,
        endpoint_region: &str,
        token: String,
        fingerprint: String,
        call: RecordedCall,
    ) {
        while self.entries.len() >= MAX_CLIENT_TOKENS {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.entries.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the
                // cap an invariant even if they ever drift.
                None => {
                    self.entries.clear();
                    break;
                }
            }
        }
        // The token is MOVED into its key: the key is the token qualified with
        // the endpoint's region, not a copy of it.
        let key = TokenKey {
            region: endpoint_region.to_string(),
            token,
        };
        let entry = TokenEntry { fingerprint, call };
        if self.entries.insert(key.clone(), entry).is_none() {
            self.order.push_back(key);
        }
    }

    /// How many tokens the ledger holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ledger holds no tokens.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drop every token recorded against `resource`, so a token never outlives
    /// the resource it names. Keyed by the resource rather than by the region,
    /// so the record goes whichever endpoint recorded it.
    pub fn forget(&mut self, resource: &str) {
        self.entries.retain(|_, entry| entry.call.resource != resource);
        let live = &self.entries;
        self.order.retain(|key| live.contains_key(key));
    }
}

/// A fingerprint of the parameters a `clientToken` was first used with.
///
/// The `@idempotencyToken` contract compares the WHOLE request, so a handler
/// renders every member it honours into `parts` (in a fixed order, maps and
/// lists sorted) and this hashes them into one comparable value. Hashed rather
/// than stored verbatim so a ledger entry's size does not grow with the request
/// it describes.
///
/// Two requests that differ in ANY member they carry must hash differently, so
/// the parts are rendered by [`fingerprint_sequence`] — and so is every part that
/// stands for more than one value ([`fingerprint_tags`],
/// [`fingerprint_multi_region`], [`fingerprint_target`]) or for a member that may
/// be absent ([`fingerprint_optional`]).
pub fn request_fingerprint(parts: &[String]) -> String {
    fingerprint_sequence(parts)
}

/// Render a SEQUENCE of values into one fingerprint value that no other sequence
/// of values renders to.
///
/// This is the rule [`request_fingerprint`] states for a request's parts, and it
/// holds one level down too: a member whose value is composite — a map, a list, a
/// structure, a union — is itself a sequence of values, and joining those values
/// with a separator character the values themselves may carry renders two
/// DIFFERENT composites alike. Each value is length-prefixed instead, so the
/// rendering is decided by the values and nothing else, and the digest that comes
/// out is fixed-width, so it can be handed back in as one value of an enclosing
/// sequence.
pub fn fingerprint_sequence<I>(values: I) -> String
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut hasher = Sha256::new();
    for value in values {
        let value = value.as_ref();
        // The length prefix keeps the values unambiguous: without it
        // ["a", "bc"] and ["ab", "c"] hash alike.
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    ::hex::encode(hasher.finalize())
}

/// Render an OPTIONAL value into one fingerprint value, so a request that omits
/// the member and a request that carries it empty are not read as one request:
/// the empty rendering is reserved for the absent member, and a member that IS
/// there renders as a digest, which is never empty.
pub fn fingerprint_optional<S: AsRef<str>>(value: Option<S>) -> String {
    match value {
        None => String::new(),
        Some(value) => fingerprint_sequence([value]),
    }
}

/// Render a tag map into one fingerprint part, in key order.
///
/// A key and its value are two values of the sequence rather than one joined
/// string: `TagKey` and `TagValue` both admit `=` (`smithy_query(query="shape",
/// shape="TagKey,TagValue")`), so a joined rendering would read two different
/// maps as the same one.
pub fn fingerprint_tags(tags: Option<&std::collections::HashMap<String, String>>) -> String {
    let sorted: BTreeMap<&str, &str> = tags
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect()
        })
        .unwrap_or_default();
    fingerprint_sequence(sorted.into_iter().flat_map(|(key, value)| [key, value]))
}

/// Render `multiRegionProperties` into one fingerprint part, peer clusters in
/// sorted order (the member is a set of peers, so the order a caller sends them
/// in is not part of the request's meaning).
///
/// The witness Region and the peer list are values of one sequence, each rendered
/// for whether it is there at all: the configuration is persisted and reported
/// back as the request spelled it, so a member the request omits and a member it
/// carries empty are different requests, and `Region` constrains no character a
/// joined rendering could be split on.
pub fn fingerprint_multi_region(properties: Option<&MultiRegionProperties>) -> String {
    match properties {
        None => String::new(),
        Some(properties) => {
            let clusters = properties.clusters.as_ref().map(|clusters| {
                let mut sorted: Vec<&str> = clusters.iter().map(String::as_str).collect();
                sorted.sort_unstable();
                fingerprint_sequence(sorted)
            });
            fingerprint_sequence([
                fingerprint_optional(properties.witness_region.as_deref()),
                fingerprint_optional(clusters),
            ])
        }
    }
}

/// Render a `targetDefinition` into one fingerprint part, naming the destination
/// the union carries and every member of it, so two targets compare equal only
/// when they name the same destination with the same values.
pub fn fingerprint_target(target: &TargetDefinition) -> String {
    match target {
        TargetDefinition::Kinesis(kinesis) => fingerprint_sequence([
            "kinesis",
            kinesis.stream_arn.as_str(),
            kinesis.role_arn.as_str(),
        ]),
    }
}

/// The message every `@idempotencyToken` operation refuses a token reuse with.
///
/// The token names a request, so reusing it for a DIFFERENT request is a
/// conflict rather than a retry; AWS states it with this sentence.
pub const CLIENT_TOKEN_MISMATCH_MESSAGE: &str =
    "Requests using a client token should supply identical request parameters";

/// Per-service state for com-amazonaws-dsql-dsql. A handler reaches it through [`service_state`].
///
/// Every collection here is caller-driven and therefore BOUNDED: see
/// [`MAX_CLUSTERS`], [`MAX_RETIRED_CLUSTERS`], [`MAX_CLIENT_TOKENS`] and
/// [`MAX_STREAMS_PER_CLUSTER`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDsqlDsqlState {
    /// Live clusters, keyed by identifier. A `BTreeMap` because `ListClusters`
    /// pages over identifier order, and that order has to be the same on every
    /// page for the cursor to resume where the previous page stopped. Bounded by
    /// [`MAX_CLUSTERS`], which `CreateCluster` checks before it inserts.
    pub clusters: BTreeMap<String, Cluster>,
    /// Clusters whose deletion has completed, so an identifier this service has
    /// already handed out is never minted again ([`Self::mint_cluster_identifier`]).
    /// Bounded by [`MAX_RETIRED_CLUSTERS`]; written only by [`Self::settle`],
    /// never directly.
    pub retired_clusters: BTreeMap<String, Cluster>,
    /// The open PostgreSQL data plane of each live cluster, keyed by identifier:
    /// the listener the core opened for the cluster, the wire served on it and
    /// the cluster's own SQL engine ([`cluster_data_plane`]).
    ///
    /// Held HERE, beside the record that says the cluster exists, because the
    /// listener is what keeps the port open: the clock-settled deletion that
    /// retires a cluster drops its plane, which closes the port, severs the
    /// sessions on it and drops the cluster's engine. Bounded by
    /// [`MAX_CLUSTERS`], the ceiling on the clusters that can own one.
    pub cluster_planes: BTreeMap<String, ClusterDataPlane>,
    /// The order [`Self::retired_clusters`] was written in, so the cap evicts the
    /// oldest tombstone rather than an arbitrary one.
    retired_order: VecDeque<String>,
    /// One `clientToken` ledger per [`TokenScope`], reached through
    /// [`Self::tokens`] / [`Self::tokens_of`].
    tokens: [TokenLedger; TokenScope::ALL.len()],
    /// How many `PolicyVersion` tokens this store has issued, which is what the
    /// next one is drawn from ([`Self::mint_policy_version`]).
    policy_versions_issued: u64,
}

impl ComAmazonawsDsqlDsqlState {
    /// The `clientToken` ledger of `scope`, for writing.
    pub fn tokens(&mut self, scope: TokenScope) -> &mut TokenLedger {
        &mut self.tokens[scope.index()]
    }

    /// The `clientToken` ledger of `scope`, for reading.
    pub fn tokens_of(&self, scope: TokenScope) -> &TokenLedger {
        &self.tokens[scope.index()]
    }

    /// Mint the `PolicyVersion` token the policy document being attached is stored
    /// under: the next value of this store's own version sequence.
    ///
    /// The sequence is the STORE's and not one cluster's, so a version names ONE
    /// stored policy document of this account: a version that has been issued is
    /// never issued again — neither by the cluster that held it, after its policy
    /// was removed and another attached, nor by any other cluster — so an
    /// `expectedPolicyVersion` held from an earlier policy selects nothing
    /// anywhere.
    ///
    /// `CreateCluster` mints the version of a policy supplied INLINE through here,
    /// because the cluster it attaches to is not in this store yet; every other
    /// attach goes through [`Self::attach_cluster_policy`].
    pub fn mint_policy_version(&mut self) -> String {
        self.policy_versions_issued += 1;
        self.policy_versions_issued.to_string()
    }

    /// Attach `document` to the cluster `identifier` names, under a freshly minted
    /// version ([`Self::mint_policy_version`]), and answer that version — or `None`
    /// when this store holds no such cluster, which is that surface's own
    /// not-found.
    ///
    /// The one place a cluster's policy is REPLACED, so the version a document is
    /// stored under is minted in the same step that stores it: a call that never
    /// reaches here — one refused on the cluster's status, on its
    /// `expectedPolicyVersion` claim or on the document itself — consumes no
    /// version at all.
    pub fn attach_cluster_policy(&mut self, identifier: &str, document: String) -> Option<String> {
        if !self.clusters.contains_key(identifier) {
            return None;
        }
        let version = self.mint_policy_version();
        let cluster = self.clusters.get_mut(identifier)?;
        cluster.policy = Some(ClusterPolicy {
            document,
            version: version.clone(),
        });
        Some(version)
    }

    /// Materialise every status change the clock already owes at `now`.
    ///
    /// Called by reporting operations too: it writes nothing a caller asked for,
    /// only what was already due at this instant even if nobody had called.
    ///
    /// A change can owe a FURTHER change of its own ([`Self::owed_after`]), so the
    /// passes run until nothing more moves, bounded by [`Self::SETTLE_PASSES`].
    pub fn settle(&mut self, now: SystemTime) {
        for _ in 0..Self::SETTLE_PASSES {
            if !self.settle_once(now) {
                return;
            }
        }
    }

    /// The most passes one [`Self::settle`] makes.
    ///
    /// The longest chain the clock owes is a multi-Region cluster's deletion — the
    /// peering tear-down, then the removal, then the cluster going away — so this
    /// many passes leave nothing behind that was already due, and the bound keeps
    /// the work of one request constant whatever the store holds.
    const SETTLE_PASSES: usize = 3;

    /// Materialise every change due at `now`, reporting whether anything moved.
    ///
    /// The scan is over [`Self::clusters`], which is capped at [`MAX_CLUSTERS`],
    /// and each cluster's streams, capped at [`MAX_STREAMS_PER_CLUSTER`], so the
    /// per-pass cost and the critical section it runs in are bounded by a
    /// constant rather than by anything a caller can grow. Nothing is allocated
    /// unless a transition is actually due.
    fn settle_once(&mut self, now: SystemTime) -> bool {
        let is_due = |pending: &Option<PendingTransition>| {
            pending.as_ref().is_some_and(|pending| pending.due_at <= now)
        };
        let anything_due = self.clusters.values().any(|cluster| {
            is_due(&cluster.pending)
                || is_due(&cluster.encryption_pending)
                || cluster
                    .streams
                    .values()
                    .any(|stream| is_due(&stream.pending))
        });
        if !anything_due {
            return false;
        }

        // Every cluster is visited, because a settled stream transition can be
        // owed to a cluster that owes none itself.
        let identifiers: Vec<String> = self.clusters.keys().cloned().collect();

        for identifier in identifiers {
            // The streams whose deletion came due: a settled deletion REMOVES the
            // stream, so `GetStream` answers `ResourceNotFoundException` and
            // `ListStreams` stops listing it, exactly as a settled cluster
            // deletion retires the cluster below.
            let mut removed_streams: Vec<String> = Vec::new();
            {
                let Some(cluster) = self.clusters.get_mut(&identifier) else {
                    continue;
                };
                for stream in cluster.streams.values_mut() {
                    let settled = stream
                        .pending
                        .as_ref()
                        .filter(|pending| pending.due_at <= now)
                        .map(|pending| pending.next_status);
                    if let Some(next_status) = settled {
                        let due_at = stream
                            .pending
                            .as_ref()
                            .map_or(now, |pending| pending.due_at);
                        // The status is reached at the instant it fell DUE, not at
                        // the instant some later request happened to materialise
                        // it, so the `updatedAt` a `statusReason` reports is the
                        // same for every reader.
                        stream.enter_status(next_status, due_at);
                        stream.pending = None;
                        if next_status == STREAM_STATUS_DELETED {
                            removed_streams.push(stream.stream_identifier.clone());
                        }
                    }
                }
                for stream_identifier in &removed_streams {
                    cluster.streams.remove(stream_identifier);
                }
                // The encryption configuration whose establishment or change came
                // due: from here on `encryptionDetails` reports the configuration
                // as in place rather than as still arriving.
                let settled = cluster
                    .encryption_pending
                    .as_ref()
                    .filter(|pending| pending.due_at <= now)
                    .map(|pending| pending.next_status);
                if let Some(next_status) = settled {
                    if let Some(details) = cluster.encryption_details.as_mut() {
                        details.encryption_status = next_status.to_string();
                    }
                    cluster.encryption_pending = None;
                }
            }
            for stream_identifier in &removed_streams {
                // A record keyed by a stream that is gone goes with it, bar the
                // ledger of the call that removed it
                // ([`TokenScope::outlives_its_resource`]).
                self.forget_tokens_for(stream_identifier);
            }

            let Some(cluster) = self.clusters.get_mut(&identifier) else {
                continue;
            };
            if !is_due(&cluster.pending) {
                continue;
            }
            let Some(pending) = cluster.pending.take() else {
                continue;
            };
            cluster.status = pending.next_status.to_string();
            // A settled deletion retires the cluster: `GetCluster` answers
            // `ResourceNotFoundException` and `ListClusters` stops listing it.
            if pending.next_status == STATUS_DELETED {
                if let Some(retired) = self.clusters.remove(&identifier) {
                    // The cluster's data plane goes with the cluster: dropping
                    // the record closes the port the core opened for it, severs
                    // every session on it and drops the cluster's engine with
                    // its rows, so nothing answers at an address a retired
                    // cluster once published.
                    self.cluster_planes.remove(&identifier);
                    // Every resource the removal takes with it, not just the
                    // cluster: a record is keyed by the resource its call acted
                    // on, and `CreateStream` acts on the STREAM, so a token
                    // keyed by a stream identifier is only reachable here.
                    for resource in retired.removed_identifiers() {
                        self.forget_tokens_for(resource);
                    }
                    self.retire(identifier, retired);
                }
                continue;
            }
            // The status this change reached can owe a further one of its own, a
            // settle span after the change that reached it — materialised by a
            // later pass, or by a later request, once it is due in turn.
            if let Some(next_status) = self.owed_after(&identifier) {
                let due_at = pending.due_at + SETTLE_AFTER;
                if let Some(cluster) = self.clusters.get_mut(&identifier) {
                    cluster.pending = Some(PendingTransition {
                        due_at,
                        next_status,
                    });
                }
            }
        }
        true
    }

    /// The status change the clock owes a cluster that has just reached a status
    /// of its own, or `None` when it reached one it stays in.
    ///
    /// The statuses the clock itself produces are the ones a chain passes
    /// through: a deletion's peering tear-down ([`STATUS_PENDING_DELETE`]) is
    /// followed by the removal ([`STATUS_DELETING`]) and that by the cluster
    /// going away ([`STATUS_DELETED`]). A cluster waiting for its peering
    /// ([`STATUS_PENDING_SETUP`]) is owed nothing while its configuration still
    /// leaves it waiting, which a request against ANOTHER cluster is what ends
    /// ([`Self::owe_active_to_clusters_done_waiting`]).
    fn owed_after(&self, identifier: &str) -> Option<&'static str> {
        let cluster = self.clusters.get(identifier)?;
        match cluster.status.as_str() {
            STATUS_PENDING_DELETE => Some(STATUS_DELETING),
            STATUS_DELETING => Some(STATUS_DELETED),
            STATUS_PENDING_SETUP => {
                (settled_cluster_status(cluster, &self.clusters) == STATUS_ACTIVE)
                    .then_some(STATUS_ACTIVE)
            }
            _ => None,
        }
    }

    /// Owe [`STATUS_ACTIVE`] to every cluster whose wait for its multi-Region
    /// peering is over.
    ///
    /// A cluster waiting in [`STATUS_PENDING_SETUP`] waits for a configuration it
    /// does not own: the OTHER side of its peering. A request against that other
    /// cluster is what ends the wait — by configuring the peering, so that every
    /// peer this cluster names names it back, or by taking the other cluster away,
    /// so that the configuration asking for the peering is unwound with it
    /// ([`Self::unwind_peerings_with`]) — and the clock carries each cluster whose
    /// own configuration no longer leaves it waiting
    /// ([`settled_cluster_status`]) to `ACTIVE` a settle span later. Called by the
    /// operations that WRITE a cluster's multi-Region configuration and by the one
    /// that unwinds it; a cluster still waiting is owed nothing.
    ///
    /// The scan is over [`Self::clusters`], capped at [`MAX_CLUSTERS`], and reads
    /// a cluster's peers — capped at [`MAX_MULTI_REGION_PEERS`] — only once the
    /// cheap status test has selected it, so the per-request cost is bounded by a
    /// constant the way [`Self::settle_once`]'s is.
    pub fn owe_active_to_clusters_done_waiting(&mut self, now: SystemTime) {
        let done: Vec<String> = self
            .clusters
            .values()
            .filter(|cluster| {
                cluster.status == STATUS_PENDING_SETUP
                    && cluster.pending.is_none()
                    && settled_cluster_status(cluster, &self.clusters) == STATUS_ACTIVE
            })
            .map(|cluster| cluster.identifier.clone())
            .collect();
        for identifier in done {
            if let Some(cluster) = self.clusters.get_mut(&identifier) {
                cluster.pending = Some(PendingTransition {
                    due_at: now + SETTLE_AFTER,
                    next_status: STATUS_ACTIVE,
                });
            }
        }
    }

    /// Unwind the multi-Region peering of the cluster `arn` names, taking its half
    /// of the relationship out of every OTHER cluster's configuration.
    ///
    /// The tear-down counterpart of
    /// [`Self::owe_active_to_clusters_done_waiting`]: a peering is a relationship
    /// BOTH clusters carry, so a cluster leaving this service has to stop being
    /// named by the clusters that stay — otherwise a survivor reports a peer that
    /// no longer exists, and the next call that recomputes its status reads that
    /// reference as a peering still waiting to be established. Called by the
    /// operation that starts a cluster's removal, which is what retires the
    /// relationship; a cluster no other names is nothing to unwind.
    ///
    /// A survivor the unwinding leaves naming NO peer at all is no longer part of a
    /// multi-Region cluster, so its whole `multiRegionProperties` goes with the
    /// peering — the peer list and the witness Region that held the quorum vote for
    /// it — leaving the single-Region cluster it now is; a survivor that still names
    /// other peers keeps its configuration, minus the one entry. Either way the
    /// clock then owes `ACTIVE` to a survivor whose remaining configuration no
    /// longer leaves it waiting.
    ///
    /// The scan is over [`Self::clusters`], capped at [`MAX_CLUSTERS`], and each
    /// cluster's peers, capped at [`MAX_MULTI_REGION_PEERS`], so the cost is
    /// bounded by a constant the way [`Self::settle_once`]'s is.
    pub fn unwind_peerings_with(&mut self, arn: &str, now: SystemTime) {
        self.unwind_peering_halves(arn, now, |_| true);
    }

    /// Unwind the peerings of the cluster `arn` names with the clusters
    /// `removed_peers` names, and with those only.
    ///
    /// The same tear-down as [`Self::unwind_peerings_with`], narrowed to the peers
    /// a rewritten peer list no longer names: a peering is a relationship BOTH
    /// clusters carry, so a cluster that stops naming a peer dismantles the
    /// relationship on that peer's side too, while the peers it still names keep
    /// theirs. Called by the operation that REPLACES a cluster's peer list, which
    /// is what retires the relationships the new list omits.
    ///
    /// A named cluster this store does not hold, and one that was not naming
    /// `arn` back, are nothing to unwind.
    ///
    /// `removed_peers` comes from a peer list the cluster itself carried, capped at
    /// [`MAX_MULTI_REGION_PEERS`], so the narrowed scan's cost stays bounded by a
    /// constant the way [`Self::unwind_peerings_with`]'s is.
    pub fn unwind_peerings_removed_from(
        &mut self,
        arn: &str,
        removed_peers: &[String],
        now: SystemTime,
    ) {
        if removed_peers.is_empty() {
            return;
        }
        self.unwind_peering_halves(arn, now, |cluster| {
            removed_peers.iter().any(|peer| peer == &cluster.arn)
        });
    }

    /// Take the cluster `arn` names out of the configuration of every cluster that
    /// names it and that `unwinds` selects, and let the clock settle whichever of
    /// them is no longer left waiting.
    ///
    /// The one place a peering's other half is taken away, so a tear-down reached
    /// through a deletion and one reached through a rewritten peer list leave the
    /// same end state.
    fn unwind_peering_halves(
        &mut self,
        arn: &str,
        now: SystemTime,
        mut unwinds: impl FnMut(&Cluster) -> bool,
    ) {
        let mut unwound = false;
        for cluster in self.clusters.values_mut() {
            if !cluster.names_peer(arn) || !unwinds(cluster) {
                continue;
            }
            unwound = true;
            let remaining: Vec<String> = cluster
                .multi_region_peers()
                .iter()
                .filter(|peer| peer.as_str() != arn)
                .cloned()
                .collect();
            cluster.multi_region_properties = match cluster.multi_region_properties.take() {
                Some(properties) if !remaining.is_empty() => Some(MultiRegionProperties {
                    clusters: Some(remaining),
                    ..properties
                }),
                _ => None,
            };
        }
        if unwound {
            self.owe_active_to_clusters_done_waiting(now);
        }
    }

    /// Mint a fresh, unused cluster identifier.
    ///
    /// The shape comes from [`new_resource_identifier`]; the loop re-draws rather
    /// than ever handing back an identifier this store already holds, live or
    /// retired.
    pub fn mint_cluster_identifier(&mut self) -> String {
        loop {
            let candidate = new_resource_identifier();
            if !self.clusters.contains_key(&candidate)
                && !self.retired_clusters.contains_key(&candidate)
            {
                return candidate;
            }
        }
    }

    /// Mint a fresh, unused per-cluster endpoint-service suffix.
    ///
    /// The shape comes from [`new_vpc_endpoint_service_suffix`]; the loop
    /// re-draws rather than ever handing back a suffix a cluster this store holds
    /// — live or retired — already carries, so the endpoint service of one
    /// cluster is never the endpoint service another is reported under. At most
    /// [`MAX_CLUSTERS`] live and [`MAX_RETIRED_CLUSTERS`] retired clusters hold
    /// one, out of the 32 to the power of
    /// [`VPC_ENDPOINT_SERVICE_SUFFIX_LENGTH`] a draw can land on, so the re-draw
    /// is the rare path and the walk it costs is bounded by those ceilings.
    pub fn mint_vpc_endpoint_service_suffix(&self) -> String {
        let taken = |suffix: &str| {
            let carried = |cluster: &Cluster| cluster.vpc_endpoint_service_suffix == suffix;
            self.clusters.values().any(carried) || self.retired_clusters.values().any(carried)
        };
        loop {
            let candidate = new_vpc_endpoint_service_suffix();
            if !taken(&candidate) {
                return candidate;
            }
        }
    }

    /// Drop every idempotency token recorded against `resource` in the ledgers
    /// whose record is about a resource that is still there, so such a token
    /// never outlives the resource it names. The ledger of the call that removes
    /// the resource keeps its record ([`TokenScope::outlives_its_resource`]).
    ///
    /// Takes ONE resource, so a removal that takes other resources with it calls
    /// it for each of them ([`Cluster::removed_identifiers`]): a record is keyed
    /// by the resource its own call acted on, which for a stream's writer is the
    /// stream rather than the cluster holding it.
    fn forget_tokens_for(&mut self, resource: &str) {
        for scope in TokenScope::ALL {
            if scope.outlives_its_resource() {
                continue;
            }
            self.tokens(scope).forget(resource);
        }
    }

    /// The cluster, and the stream of it, that a parsed tagging ARN addresses.
    ///
    /// Both tagged resources of this service are written under the cluster's own
    /// resource type: a cluster is `cluster/{identifier}` and a stream of it is
    /// `cluster/{cluster}/stream/{stream}` (`ClusterArn` / `StreamArn`), and
    /// [`parse_resource_arn`] splits the resource field at its FIRST separator, so
    /// a stream arrives as the type `cluster` with the id `{cluster}/stream/{stream}`.
    /// Anything else addresses nothing here.
    ///
    /// An ARN scoped to another endpoint addresses nothing here either
    /// ([`ArnResource::is_scoped_to`]): `endpoint_region` is the region the request
    /// reached, and only resources minted for it live in this store.
    ///
    /// `None` here is a resource that is ABSENT, not a request that is malformed:
    /// the ARN does name this service and one of its resource types, so a sub-path
    /// this service mints no resource for (`cluster/{id}/foo/bar`, or a stream path
    /// carrying more segments) belongs with an unknown identifier — the modelled
    /// 404 — rather than with [`invalid_resource_arn`]'s 400, which is reserved for
    /// a value that names no Aurora DSQL resource at all.
    fn addressed_resource<'a>(
        resource: &'a ArnResource,
        endpoint_region: &str,
    ) -> Option<(&'a str, Option<&'a str>)> {
        resource
            .is_scoped_to(endpoint_region)
            .then(|| Self::arn_path(resource))
            .flatten()
    }

    /// The identifiers a tagging ARN's resource PATH names, whatever scope the ARN
    /// carries: the cluster, and the stream nested under it when the path nests one.
    ///
    /// The scope is [`Self::addressed_resource`]'s business; this is the read of the
    /// path alone, so a refusal can name the resource an out-of-scope ARN addresses
    /// just as precisely as one this endpoint could have held
    /// ([`Self::addressed_identity`]).
    fn arn_path(resource: &ArnResource) -> Option<(&str, Option<&str>)> {
        if resource.resource_type != RESOURCE_TYPE_CLUSTER {
            return None;
        }
        match resource.resource_id.split_once('/') {
            None => Some((resource.resource_id.as_str(), None)),
            Some((cluster, rest)) => {
                let stream = rest
                    .strip_prefix(RESOURCE_TYPE_STREAM)
                    .and_then(|rest| rest.strip_prefix('/'))?;
                (!cluster.is_empty() && !stream.is_empty()).then_some((cluster, Some(stream)))
            }
        }
    }

    /// The identity a modelled error reports for the resource a tagging ARN
    /// ADDRESSES: that resource's OWN type segment and its OWN identifier.
    ///
    /// A resource is identified by the type segment it is stored under and the
    /// identifier that segment carries, so a stream of a cluster is a
    /// [`RESOURCE_TYPE_STREAM`] named by its own stream identifier — the identity
    /// [`stream_not_found`] reports for that very stream — and never the leading
    /// `cluster` segment [`parse_resource_arn`] split the resource field at, carrying
    /// the rest of the path for an identifier. An ARN whose path names no resource of
    /// this service at all is reported exactly as it was spelled, there being no
    /// resource of this service to name instead.
    ///
    /// Read by the refusals a tagging operation states ABOUT a resource it resolved —
    /// the tag ceiling of the resource addressed (`TagResource`'s
    /// `ServiceQuotaExceededException`). An ARN that resolves to nothing is not such a
    /// refusal: the tagging surface answers that in its own terms, naming no resource
    /// at all ([`tagging_resource_not_found`]).
    pub fn addressed_identity(&self, resource: &ArnResource) -> ResourceIdentity {
        match Self::arn_path(resource) {
            Some((_, Some(stream_identifier))) => ResourceIdentity {
                resource_type: RESOURCE_TYPE_STREAM.to_string(),
                resource_id: stream_identifier.to_string(),
            },
            Some((cluster_identifier, None)) => ResourceIdentity {
                resource_type: RESOURCE_TYPE_CLUSTER.to_string(),
                resource_id: cluster_identifier.to_string(),
            },
            None => ResourceIdentity {
                resource_type: resource.resource_type.clone(),
                resource_id: resource.resource_id.clone(),
            },
        }
    }

    /// The tags of the resource `resource` names, if this service holds it.
    ///
    /// The one place a parsed `resourceArn` becomes the tag map it addresses, so
    /// every tagging operation agrees about which ARNs name something here: a
    /// cluster, a stream of a cluster (see [`Self::addressed_resource`]), and
    /// nothing else — a well-formed ARN of any other Aurora DSQL resource type, or
    /// one scoped to another region, account or partition than the one this endpoint
    /// serves, or one whose scope is not the scope the resource was MINTED in,
    /// answers `None` exactly as an unknown cluster id does. A retired (deleted)
    /// cluster is not addressable either; its tombstone exists only for delete
    /// idempotency.
    pub fn resource_tags(
        &self,
        resource: &ArnResource,
        endpoint_region: &str,
    ) -> Option<&BTreeMap<String, String>> {
        let (cluster_identifier, stream_identifier) =
            Self::addressed_resource(resource, endpoint_region)?;
        let cluster = self.clusters.get(cluster_identifier)?;
        match stream_identifier {
            Some(stream_identifier) => cluster
                .streams
                .get(stream_identifier)
                .filter(|stream| resource.is_arn_of(&stream.arn))
                .map(|stream| &stream.tags),
            None => resource.is_arn_of(&cluster.arn).then_some(&cluster.tags),
        }
    }

    /// [`Self::resource_tags`] for a caller that WRITES the tags — the same
    /// resolution, so a tag write and a tag read never disagree about which ARNs
    /// name something.
    pub fn resource_tags_mut(
        &mut self,
        resource: &ArnResource,
        endpoint_region: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        let (cluster_identifier, stream_identifier) =
            Self::addressed_resource(resource, endpoint_region)?;
        let cluster = self.clusters.get_mut(cluster_identifier)?;
        match stream_identifier {
            Some(stream_identifier) => cluster
                .streams
                .get_mut(stream_identifier)
                .filter(|stream| resource.is_arn_of(&stream.arn))
                .map(|stream| &mut stream.tags),
            None => resource.is_arn_of(&cluster.arn).then_some(&mut cluster.tags),
        }
    }

    /// Keep `cluster` as a retired-identifier tombstone, evicting the oldest one
    /// first if that would push the map past [`MAX_RETIRED_CLUSTERS`].
    fn retire(&mut self, identifier: String, cluster: Cluster) {
        while self.retired_clusters.len() >= MAX_RETIRED_CLUSTERS {
            match self.retired_order.pop_front() {
                Some(oldest) => {
                    self.retired_clusters.remove(&oldest);
                }
                // Same invariant as `TokenLedger::record`: unreachable, and
                // clearing keeps the cap rather than spinning.
                None => {
                    self.retired_clusters.clear();
                    break;
                }
            }
        }
        if self
            .retired_clusters
            .insert(identifier.clone(), cluster)
            .is_none()
        {
            self.retired_order.push_back(identifier);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDsqlDsqlState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDsqlDsqlState>)
}

/// The region this request is scoped to: the region of its SigV4 credential
/// scope, or [`DEFAULT_REGION`] when it carries none.
///
/// Narrowed to what `ClusterArn`'s pattern accepts for the region segment
/// (`[a-z0-9-]{1,20}`), so a cluster minted for an oddly-signed request still
/// carries a well-formed ARN.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    let signed = ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let narrowed: String = signed
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .take(20)
        .collect();
    if narrowed.is_empty() {
        DEFAULT_REGION.to_string()
    } else {
        narrowed
    }
}

/// The ARN of a cluster: `arn:aws:dsql:{region}:{account}:cluster/{identifier}`,
/// built through the one ARN helper so the account segment is the emulator's.
pub fn cluster_arn(region: &str, identifier: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RESOURCE_TYPE_CLUSTER}/{identifier}"),
    )
}

/// The resource an Aurora DSQL ARN names: the scope it is in — partition, region
/// and account — plus its type segment and the id after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArnResource {
    /// The partition segment, as the ARN spells it.
    pub partition: String,
    /// The region segment, as the ARN spells it (empty when the ARN carries none).
    pub region: String,
    /// The account segment, as the ARN spells it (empty when the ARN carries none).
    pub account: String,
    /// The resource type, e.g. [`RESOURCE_TYPE_CLUSTER`].
    pub resource_type: String,
    /// The resource's own identifier.
    pub resource_id: String,
}

impl ArnResource {
    /// Whether this ARN names a resource the endpoint serving the request could
    /// hold: one in this partition, in `endpoint_region`, and in the account this
    /// emulator serves.
    ///
    /// Aurora DSQL is a regional service and every ARN it mints carries the region
    /// the minting request was signed for, the account that owns the resource and
    /// the partition it lives in ([`cluster_arn`], [`stream_arn`]); `ClusterArn`'s
    /// pattern pins all three segments
    /// (`^arn:aws(-[^:]+)?:dsql:[a-z0-9-]{1,20}:[0-9]{12}:cluster/[a-z0-9]{26}$`),
    /// so they are part of the resource's identity rather than decoration. An ARN
    /// differing in any of them therefore names a resource of some OTHER region,
    /// account or partition — one this endpoint holds none of, and must neither read
    /// nor write.
    pub fn is_scoped_to(&self, endpoint_region: &str) -> bool {
        self.partition == ARN_PARTITION
            && self.region == endpoint_region
            && self.account == ::engine::emulator_account_id()
    }

    /// Whether this ARN is the ARN `minted` is — the one a stored resource was
    /// minted with.
    ///
    /// A resource is created in ONE region, under ONE account, in ONE partition, and
    /// the ARN this service hands back carries all three ([`cluster_arn`],
    /// [`stream_arn`]). The type and the id are what resolved a stored resource, so
    /// what remains to agree on is that scope: an ARN that spells it differently
    /// names some other region's, account's or partition's resource that happens to
    /// share an identifier, and no identifier makes those the same resource.
    pub fn is_arn_of(&self, minted: &str) -> bool {
        parse_resource_arn(minted).is_some_and(|minted| {
            self.partition == minted.partition
                && self.region == minted.region
                && self.account == minted.account
        })
    }
}

/// The resource a tagging ARN names, or `None` when the ARN names no Aurora DSQL
/// resource at all.
///
/// An ARN is the six colon-separated fields
/// `arn:{partition}:{service}:{region}:{account}:{resource}`, and its resource
/// field is `{type}/{id}` or `{type}:{id}`; anything else — too few fields, a
/// service field that is not this service's, a resource field with no type —
/// names nothing here.
///
/// Every field is reported, because an ARN's partition, region and account are part
/// of the resource's identity rather than decoration: a caller needs the region an
/// ARN was minted in (see [`arn_region`]), and the resolution of a tagging ARN
/// matches all three against the endpoint's own scope
/// ([`ArnResource::is_scoped_to`]) and against the scope the resource it names was
/// minted in ([`ArnResource::is_arn_of`]).
pub fn parse_resource_arn(arn: &str) -> Option<ArnResource> {
    // `splitn(6, ..)` keeps the resource field whole: it is the only field that
    // may itself contain a colon.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    if fields.next()? != ARN_SERVICE {
        return None;
    }
    let region = fields.next()?;
    let account = fields.next()?;
    let (resource_type, resource_id) = fields.next()?.split_once(['/', ':'])?;
    if resource_type.is_empty() || resource_id.is_empty() {
        return None;
    }
    Some(ArnResource {
        partition: partition.to_string(),
        region: region.to_string(),
        account: account.to_string(),
        resource_type: resource_type.to_string(),
        resource_id: resource_id.to_string(),
    })
}

/// The ARN of a stream of a cluster:
/// `arn:aws:dsql:{region}:{account}:cluster/{cluster}/stream/{stream}`, which is
/// what `StreamArn`'s pattern
/// (`^arn:aws(-[^:]+)?:dsql:[a-z0-9-]{1,20}:[0-9]{12}:cluster/[a-z0-9]{26}/stream/[a-z0-9]{26}$`,
/// `smithy_query(query="shape", shape="StreamArn")`) accepts.
pub fn stream_arn(region: &str, cluster_identifier: &str, stream_identifier: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{RESOURCE_TYPE_CLUSTER}/{cluster_identifier}/{RESOURCE_TYPE_STREAM}/{stream_identifier}"
        ),
    )
}

/// The region segment of an ARN this service minted
/// (`arn:aws:dsql:{region}:{account}:cluster/{identifier}`), when the ARN is one of
/// this service's and carries a region `ClusterArn`'s pattern accepts.
///
/// A thin accessor over [`parse_resource_arn`], so this module walks an ARN's fields
/// in exactly one place; an ARN of another service names nothing here, for the same
/// reason it names no resource. The segment is narrowed the way [`request_region`]
/// narrows a signing region, so a region read out of an ARN can only ever be
/// interpolated into another well-formed value; anything else answers `None`, and the
/// caller falls back to the request's own region.
pub fn arn_region(arn: &str) -> Option<String> {
    parse_resource_arn(arn)
        .map(|resource| resource.region)
        .filter(|region| is_arn_region_segment(region))
}

/// The region a resource ALREADY IN THE STORE lives in: the region of the ARN it
/// was minted with, with `request_region` only as a fallback for a stored ARN that
/// carries no region segment [`arn_region`] accepts.
///
/// Every region-qualified value a handler derives FOR such a resource — the ARN of
/// a resource nested under it, a name or an endpoint that fronts it, the KMS key
/// ARN one of its members expands into — is qualified with this region, not with
/// the region the request happens to be signed for: the region an ARN was minted
/// in is part of the resource's identity ([`ArnResource::is_arn_of`]) rather than a
/// property of the call that reads or changes it. Only the call that MINTS a
/// resource takes the region from the request, which is then the one its ARN
/// carries ever after.
pub fn resource_region(arn: &str, request_region: &str) -> String {
    arn_region(arn).unwrap_or_else(|| request_region.to_string())
}

/// Whether `value` is what `ClusterArn`'s pattern accepts for its region segment:
/// `[a-z0-9-]{1,20}`.
fn is_arn_region_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// How many characters the endpoint-service suffix of a cluster is written with.
///
/// An interface VPC endpoint service is named `com.amazonaws.{region}.{service}`,
/// and Aurora DSQL qualifies that PER CLUSTER with a SHORT token of its own —
/// recorded replies carry four characters after `dsql-`, and `ServiceName`'s
/// pattern (`smithy_query(query="shape", shape="ServiceName")`:
/// `^com\.amazonaws\.[a-z0-9-]+\.dsql[a-z0-9-]*$`) admits exactly such a suffix
/// after `dsql`. The token is the endpoint service's own name and not the
/// cluster's, so it is drawn independently of the identifier rather than cut
/// from it ([`new_vpc_endpoint_service_suffix`]) — the NAME of the service is a
/// label of its own, where the HOST the same reply reports is an address a
/// client dials and is therefore read from where this emulator answers
/// ([`cluster_endpoint`]).
pub const VPC_ENDPOINT_SERVICE_SUFFIX_LENGTH: usize = 4;

/// A fresh per-cluster endpoint-service suffix: [`VPC_ENDPOINT_SERVICE_SUFFIX_LENGTH`]
/// characters of the alphabet this service writes an identifier in
/// ([`CROCKFORD_SYMBOLS`]), which is what `ServiceName`'s `[a-z0-9-]*` and one
/// DNS label both admit.
///
/// Drawn from the entropy of a fresh identifier draw ([`new_resource_identifier`],
/// whose low-order symbols are its 80 entropy bits rather than its leading
/// timestamp), so two clusters minted in one millisecond get different suffixes
/// and a suffix reveals nothing of the cluster it fronts. The draw is short, so
/// it can repeat: [`ComAmazonawsDsqlDsqlState::mint_vpc_endpoint_service_suffix`]
/// re-draws rather than ever handing back a suffix this store already has.
#[must_use]
pub fn new_vpc_endpoint_service_suffix() -> String {
    let drawn = new_resource_identifier();
    let start = drawn
        .len()
        .saturating_sub(VPC_ENDPOINT_SERVICE_SUFFIX_LENGTH);
    drawn[start..].to_string()
}

/// The VPC endpoint service that fronts one cluster:
/// `com.amazonaws.{region}.dsql-{suffix}`, for that cluster's own stored
/// endpoint-service suffix ([`Cluster::vpc_endpoint_service_suffix`]).
///
/// The suffix is read off the CLUSTER and never derived from the call, so two
/// reads of one cluster name one endpoint service, and the endpoint service of
/// one cluster is never the endpoint service another is reported under.
///
/// `region` is at most 20 characters and the suffix is
/// [`VPC_ENDPOINT_SERVICE_SUFFIX_LENGTH`] characters, so the result is well
/// inside `ServiceName`'s `@length(1, 128)`.
pub fn vpc_endpoint_service_name(region: &str, suffix: &str) -> String {
    format!("com.amazonaws.{region}.dsql-{suffix}")
}

/// The tag set a read of one resource REPORTS: every pair the resource holds, and
/// the EMPTY map for a resource holding none.
///
/// `TagMap` is `@length(0, 200)` (`smithy_query(query="shape", shape="TagMap")`),
/// so the empty map is a value the member carries rather than an absence: a reader
/// that left the member out would leave a client unable to tell a resource with no
/// tags from one whose tags this reply does not report, and would disagree with
/// `ListTagsForResource`, which reports the same set for the same resource.
///
/// Every reader of a resource's tags builds the reported map here, so a cluster's
/// and a stream's tags are reported on the same terms.
#[must_use]
pub fn reported_tags(held: &BTreeMap<String, String>) -> HashMap<String, String> {
    held.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// The customer-managed key a `kmsEncryptionKey` names, if it names one at all.
///
/// `AWS_OWNED_KMS_KEY` is a RESERVED value, not a key identifier: it is the
/// `EncryptionType` member that stands for the key owned by Amazon Web Services,
/// and `UpdateCluster`'s `kmsEncryptionKey` documentation
/// (`smithy_query(query="doc", shape="UpdateClusterInput",
/// member="kmsEncryptionKey")`) spells out that supplying it switches the cluster
/// TO that key. So it selects Amazon-owned encryption and names no customer key —
/// the same answer as a request that carries no `kmsEncryptionKey` at all. Every
/// caller resolves a supplied key through here, so neither the stored key nor the
/// reported [`EncryptionDetails`] can ever hold the sentinel as if it were a key.
pub fn customer_managed_key(kms_encryption_key: Option<&str>) -> Option<&str> {
    kms_encryption_key.filter(|key| *key != ENCRYPTION_AWS_OWNED)
}

/// The `EncryptionDetails` of a cluster, derived from the `kmsEncryptionKey` the
/// caller supplied: a customer-managed key when there is one, the
/// Amazon-owned key when there is not — including when the supplied value is the
/// reserved `AWS_OWNED_KMS_KEY` (see [`customer_managed_key`]).
///
/// `kmsKeyArn` is modelled as an ARN, so a key id or an alias is expanded into one
/// in `region` — the region of the CLUSTER whose encryption this describes (see
/// [`resource_region`]), which for the call that mints the cluster is the request's
/// own. A key supplied as a full ARN already names its region and is kept as it was
/// given.
///
/// `encryption_status` is how far the cluster is from actually being protected by
/// this configuration: a configuration a call has just asked for is reported as
/// arriving ([`ENCRYPTION_STATUS_ENABLING`] for the cluster's first one,
/// [`ENCRYPTION_STATUS_UPDATING`] for a change to another one) and reaches
/// [`ENCRYPTION_STATUS_ENABLED`] when the clock settles it
/// ([`Cluster::encryption_pending`]), so the member carries the state machine the
/// enum models rather than one of its values.
pub fn encryption_details(
    region: &str,
    kms_encryption_key: Option<&str>,
    encryption_status: &str,
) -> EncryptionDetails {
    match customer_managed_key(kms_encryption_key) {
        Some(key) => EncryptionDetails {
            encryption_type: ENCRYPTION_CUSTOMER_MANAGED.to_string(),
            kms_key_arn: Some(kms_key_arn(region, key)),
            encryption_status: encryption_status.to_string(),
        },
        None => EncryptionDetails {
            encryption_type: ENCRYPTION_AWS_OWNED.to_string(),
            kms_key_arn: None,
            encryption_status: encryption_status.to_string(),
        },
    }
}

/// The `EncryptionStatus` the FIRST configuration of a cluster starts in, for the
/// `kmsEncryptionKey` the creating call supplied.
///
/// A cluster created on the key owned by Amazon Web Services has nothing to put in
/// place: that key protects the service's clusters before any of them exists, and
/// no data of a cluster being created was ever under another key — so the
/// configuration is already in place in the create's own answer
/// ([`ENCRYPTION_STATUS_ENABLED`]). A customer-managed key has to be established
/// for this cluster, which is work the clock owes, so it is reported as arriving
/// ([`ENCRYPTION_STATUS_ENABLING`]) until [`encryption_settlement`] comes due.
pub fn initial_encryption_status(kms_encryption_key: Option<&str>) -> &'static str {
    match customer_managed_key(kms_encryption_key) {
        Some(_) => ENCRYPTION_STATUS_ENABLING,
        None => ENCRYPTION_STATUS_ENABLED,
    }
}

/// What the clock owes a cluster's `encryptionDetails` once a call has asked for
/// `encryption_status`: an encryption configuration still being put in place
/// reaches [`ENCRYPTION_STATUS_ENABLED`] a settle span later, and one already in
/// place is owed nothing.
///
/// Every caller that writes the member goes through here, so none of them can
/// decide differently when the configuration a request asked for becomes the one
/// the cluster's data is actually protected by.
pub fn encryption_settlement(
    encryption_status: &str,
    now: SystemTime,
) -> Option<PendingTransition> {
    (encryption_status != ENCRYPTION_STATUS_ENABLED).then(|| PendingTransition {
        due_at: now + SETTLE_AFTER,
        next_status: ENCRYPTION_STATUS_ENABLED,
    })
}

/// Whether `requested` protects the cluster's data differently from `stored`: a
/// different `EncryptionType`, or a different key under the customer-managed one.
///
/// The CONFIGURATION is the type and the key, never the `encryptionStatus`, which
/// only says how far a configuration has got. A request that asks for the
/// configuration a cluster already has asks for no re-encryption, so nothing is in
/// flight for it to report.
pub fn encryption_configuration_changed(
    stored: Option<&EncryptionDetails>,
    requested: &EncryptionDetails,
) -> bool {
    stored.is_none_or(|stored| {
        stored.encryption_type != requested.encryption_type
            || stored.kms_key_arn != requested.kms_key_arn
    })
}

/// Expand a `kmsEncryptionKey` (an ARN, an alias, or a bare key id) into a KMS
/// key ARN in `region`.
fn kms_key_arn(region: &str, kms_encryption_key: &str) -> String {
    if kms_encryption_key.starts_with("arn:") {
        kms_encryption_key.to_string()
    } else if kms_encryption_key.starts_with("alias/") {
        ::engine::emulator_arn("kms", region, kms_encryption_key)
    } else {
        ::engine::emulator_arn("kms", region, &format!("key/{kms_encryption_key}"))
    }
}

/// The length `ClusterId`'s and `StreamId`'s `^[a-z0-9]{26}$` fixes
/// (`smithy_query(query="shape", shape="ClusterId,StreamId")`).
pub const RESOURCE_ID_LEN: usize = 26;

/// Whether `value` is inside the value range this service MINTS an identifier
/// from: a minted identifier is 26 symbols of [`CROCKFORD_SYMBOLS`] rendering the
/// 128 bits [`new_resource_identifier`] builds, while 26 such symbols carry 130
/// bits, so both the alphabet and the leading symbol decide it.
///
/// Narrower than `ClusterId`'s and `StreamId`'s `^[a-z0-9]{26}$`, which is the
/// FORM an identifier has to be in and all the form asks: the pattern admits the
/// four letters the encoding leaves out (`i` and `l` read as `1`, `o` as `0`, and
/// `u` is left out of the encoding altogether) and strings above every value this
/// service could have minted, and a value inside the pattern is an identifier this
/// account simply does not hold — reported as [`cluster_not_found`] or
/// [`stream_not_found`] by every operation that resolves one.
///
/// So this is asked only where the range is part of what a caller has to send
/// ([`validate_minted_cluster_identifier`]) or where this service is reading back
/// something it issued itself ([`decode_pagination_cursor`]).
///
/// Decided by the ROUND TRIP through the renderer this service mints with, so the
/// range is spelled in exactly one place: a value is inside it when it decodes to
/// a 128-bit value that renders back to that same value.
fn renders_a_minted_identifier(value: &str) -> bool {
    decode_resource_identifier(value).is_some_and(|bits| crockford_base32(bits) == value)
}

/// The 128-bit value `value`'s symbols spell, or `None` when `value` is not 26
/// symbols of [`CROCKFORD_SYMBOLS`] at all.
///
/// 26 symbols carry 130 bits, so the two bits above the 128th are dropped here
/// and the leading symbol of the re-rendering differs from the one that was read:
/// the round trip in [`renders_a_minted_identifier`] is what sees such a value is
/// outside the minted range.
fn decode_resource_identifier(value: &str) -> Option<u128> {
    if value.len() != RESOURCE_ID_LEN {
        return None;
    }
    value.bytes().try_fold(0u128, |bits, symbol| {
        let index = CROCKFORD_SYMBOLS.iter().position(|known| *known == symbol)?;
        Some((bits << 5) | index as u128)
    })
}

/// The page walk a cursor belongs to: the operation that issued it, together with
/// the collection that operation walked.
///
/// A `nextToken` names a position in ONE walk, so it is only a cursor of the walk
/// that issued it: `ListClusters` pages the account's clusters, while `ListStreams`
/// pages the streams of the ONE cluster it was addressed at, and neither of those
/// positions means anything in the other walk. Both scopes are spelled here so the
/// two readers cannot name their walks differently.
pub fn cluster_page_scope() -> String {
    "ListClusters/clusters".to_string()
}

/// See [`cluster_page_scope`]: `ListStreams` walks the streams of the cluster it
/// was addressed at, so the addressed cluster is part of the walk a cursor resumes.
pub fn stream_page_scope(cluster_identifier: &str) -> String {
    format!("ListStreams/streams/{cluster_identifier}")
}

/// How many bytes of the integrity check a cursor carries: 16 of the 32 a
/// SHA-256 digest holds, which is enough that no two walks or entries of this
/// service share one and keeps the token short.
const CURSOR_TAG_LEN: usize = 16;

/// The integrity check a cursor carries: a one-way digest over the walk the
/// cursor was issued by and the entry it resumes after.
///
/// This is what makes a value a cursor of ONE walk over ONE of its entries and of
/// nothing else: the digest is not a secret, so it does not authenticate the
/// caller — it binds the token, so a value this service issued for another walk,
/// or never issued at all, carries a check that does not match the walk reading it
/// and therefore names no page of it.
///
/// The scope and the identifier are fed in with a separator between them, so no
/// two different pairs are one byte string.
fn cursor_tag(scope: &str, identifier: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(scope.as_bytes());
    hasher.update([0]);
    hasher.update(identifier.as_bytes());
    hasher.finalize()[..CURSOR_TAG_LEN].to_vec()
}

/// Encode a list operation's pagination cursor: the identifier of the last entry
/// the page returned, carried behind the [`cursor_tag`] that binds it to `scope`,
/// the walk that issued it, and hex-encoded so the `nextToken` a client
/// round-trips is an opaque token rather than a resource name.
///
/// Shared by every paginated reader of this service (`ListClusters` pages over
/// `ClusterId` order, `ListStreams` over a cluster's `StreamId` order), so a
/// cursor is spelled one way.
pub fn encode_pagination_cursor(scope: &str, last_identifier: &str) -> String {
    let mut token = cursor_tag(scope, last_identifier);
    token.extend_from_slice(last_identifier.as_bytes());
    ::hex::encode(token)
}

/// Decode a cursor [`encode_pagination_cursor`] issued for `scope`: the identifier
/// the page it ended stopped on.
///
/// `None` for every value this service did not issue for THIS walk — a token that
/// is not hex, one whose identifier half is no identifier this service minted
/// ([`renders_a_minted_identifier`]: a cursor only ever carries one this service
/// issued), one whose integrity check does not match the walk and the identifier it
/// is read with (so a fabricated cursor, or one carrying an identifier no page of
/// this walk ever stopped on, resolves to no page rather than to a seek after some
/// arbitrary key), and one issued by a different walk, whose check is over that
/// walk's scope.
pub fn decode_pagination_cursor(scope: &str, token: &str) -> Option<String> {
    let decoded = ::hex::decode(token).ok()?;
    if decoded.len() <= CURSOR_TAG_LEN {
        return None;
    }
    let (tag, identifier) = decoded.split_at(CURSOR_TAG_LEN);
    let identifier = String::from_utf8(identifier.to_vec()).ok()?;
    if !renders_a_minted_identifier(&identifier) {
        return None;
    }
    (cursor_tag(scope, &identifier) == tag).then_some(identifier)
}

/// `maxResults`' published `@range` bounds. Both list operations bind the one
/// `MaxResults` shape (`smithy_query(query="shape", shape="MaxResults")`:
/// `@range(min: 1, max: 100)`), so the bounds belong to the shape and are
/// spelled once here rather than per operation; only the `@default` is
/// per-operation (`smithy_query(query="operation",
/// operation="ListClusters,ListStreams")`: 20 and 10), and that is what each
/// handler passes to [`PageBounds::resolve`].
pub const MAX_RESULTS_MIN: usize = 1;
/// See [`MAX_RESULTS_MIN`].
pub const MAX_RESULTS_MAX: usize = 100;

/// One page a list operation answers with: the summaries it returns, and the
/// `nextToken` it carries only when the walk left an entry behind.
#[derive(Debug)]
pub struct Page<S> {
    /// The page's items, in the identifier order the walked map keeps them in,
    /// at most as many as the resolved page size.
    pub items: Vec<S>,
    /// The cursor the next page resumes after — the last item this page
    /// returned — present only when entries remain beyond it.
    pub next_token: Option<String>,
}

/// Where a list call's page starts and how many entries it may hold.
///
/// Every paginated reader of this service walks an identifier-ordered
/// `BTreeMap` the same way: resume after the cursor the previous page issued,
/// return at most `maxResults` entries, and issue a cursor only when a further
/// entry exists. That walk lives here, so the two list operations differ only
/// in the modelled default, the map walked and the summary built.
#[derive(Debug)]
pub struct PageBounds {
    /// The walk this page belongs to: the cursor it issues is bound to it, and a
    /// cursor issued by any other walk does not resolve against it
    /// ([`cluster_page_scope`] / [`stream_page_scope`]).
    scope: String,
    /// The identifier the previous page stopped on; this page starts after it.
    resume_after: Option<String>,
    /// The most items this page may hold.
    page_size: usize,
}

impl PageBounds {
    /// Read a list call's `nextToken` and `maxResults` into the bounds its page
    /// is cut at, refusing a `nextToken` no previous page of THIS walk issued
    /// with the modelled [`unissued_pagination_token`] refusal.
    ///
    /// `scope` is the walk the call is paging ([`cluster_page_scope`] /
    /// [`stream_page_scope`]) and `default_max_results` is the operation's own
    /// modelled `@default`.
    pub fn resolve(
        scope: String,
        next_token: Option<&str>,
        max_results: i32,
        default_max_results: usize,
    ) -> Result<Self, ValidationException> {
        let resume_after = match next_token {
            Some(token) => Some(
                decode_pagination_cursor(&scope, token).ok_or_else(unissued_pagination_token)?,
            ),
            None => None,
        };

        // `maxResults` carries the operation's `@default` and the shape's
        // `@range`, both enforced before a handler runs. The conversion and
        // clamp cover only a caller that reached the handler some other way (an
        // in-process call that built the input directly): a negative value falls
        // back to the modelled default, and anything outside the modelled range
        // is pulled back into it.
        let page_size = usize::try_from(max_results)
            .unwrap_or(default_max_results)
            .clamp(MAX_RESULTS_MIN, MAX_RESULTS_MAX);

        Ok(Self {
            scope,
            resume_after,
            page_size,
        })
    }

    /// The most items this page may hold, after the clamp.
    pub fn page_size(&self) -> usize {
        self.page_size
    }

    /// Cut one page out of `entries`, an identifier-keyed map, mapping each
    /// entry this page returns through `summarize`.
    ///
    /// The walk reads one entry past the page, so the `nextToken` is issued
    /// exactly when an entry remains beyond the page — and it names the last
    /// entry the page returned, which is where the next call resumes.
    pub fn page<V, S>(
        &self,
        entries: &BTreeMap<String, V>,
        mut summarize: impl FnMut(&V) -> S,
    ) -> Page<S> {
        let mut items = Vec::new();
        let mut last_identifier: Option<&String> = None;
        let mut entries_remain = false;

        for (identifier, value) in entries
            .iter()
            .filter(|(identifier, _)| {
                self.resume_after
                    .as_ref()
                    .is_none_or(|cursor| *identifier > cursor)
            })
            // One past the page, so the token is emitted only when something
            // remains.
            .take(self.page_size + 1)
        {
            if items.len() == self.page_size {
                entries_remain = true;
                break;
            }
            items.push(summarize(value));
            last_identifier = Some(identifier);
        }

        let next_token = match (entries_remain, last_identifier) {
            (true, Some(identifier)) => Some(encode_pagination_cursor(&self.scope, identifier)),
            _ => None,
        };

        Page { items, next_token }
    }
}

/// A fresh cluster or stream identifier: a ULID, lowercased.
///
/// A ULID is a 48-bit millisecond timestamp followed by 80 bits of entropy,
/// rendered in Crockford base32 — 26 characters, which is the form Aurora DSQL
/// cluster and stream ids take and which satisfies `ClusterId`'s and `StreamId`'s
/// `^[a-z0-9]{26}$`. The timestamp leads, so ULIDs sort in creation order and the
/// identifier order `ListClusters` pages over is creation order.
///
/// Prefer [`ComAmazonawsDsqlDsqlState::mint_cluster_identifier`], which re-draws
/// rather than ever handing back an id already in use.
pub fn new_resource_identifier() -> String {
    /// Distinguishes two ids minted inside the same nanosecond.
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let millis = since_epoch.as_millis() & ((1u128 << 48) - 1);

    // 80 bits of entropy from a one-way hash of the sub-nanosecond clock, a
    // process-lifetime counter and the process id, so two ids never collide
    // within a run and ids from different runs stay apart.
    let mut hasher = Sha256::new();
    hasher.update(since_epoch.as_nanos().to_be_bytes());
    hasher.update(SEQUENCE.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    hasher.update(std::process::id().to_be_bytes());
    let digest = hasher.finalize();
    let entropy = digest
        .iter()
        .take(10)
        .fold(0u128, |acc, byte| (acc << 8) | u128::from(*byte));

    crockford_base32((millis << 80) | entropy)
}

/// The 32 symbols Crockford base32 (and therefore ULID) uses, lowercased: the
/// digits plus the letters with `i`, `l`, `o` and `u` left out.
///
/// The alphabet an identifier of this service is both WRITTEN and READ in
/// ([`crockford_base32`] renders with it, [`decode_resource_identifier`] reads a
/// value by it), so the two cannot disagree about which letters are symbols.
const CROCKFORD_SYMBOLS: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Render the 128 bits of `value` as the 26 Crockford base32 characters a ULID
/// is written with, most significant group first.
fn crockford_base32(value: u128) -> String {
    (0..26)
        .map(|position| {
            let shift = 5 * (25 - position);
            let symbol = ((value >> shift) & 0x1f) as usize;
            char::from(CROCKFORD_SYMBOLS[symbol])
        })
        .collect()
}

/// How one resource is identified in the modelled shapes that carry a resource's
/// identity (`ResourceNotFoundException` and `ServiceQuotaExceededException`, both
/// of which model a `@required` `resourceId` and `resourceType`, and
/// `ConflictException`, which models both as optional): the type segment the
/// resource is stored under, and that resource's own identifier.
///
/// The two members describe ONE resource — the model words them as the resource
/// that "could not be found" and the resource that "exceeds a quota" — so a
/// refusal never pairs one resource's type with another's identifier.
///
/// Built from the store rather than per handler
/// ([`ComAmazonawsDsqlDsqlState::addressed_identity`]), so every operation that
/// reports a resource's identity reports the same two values for the same resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceIdentity {
    /// The resource's own type segment, e.g. [`RESOURCE_TYPE_STREAM`] for a stream.
    pub resource_type: String,
    /// The resource's own identifier, carrying none of a parent's segments.
    pub resource_id: String,
}

impl ResourceIdentity {
    /// The identity of the cluster `identifier` names: the type segment a cluster
    /// is stored under, and that cluster's own identifier.
    pub fn cluster(identifier: &str) -> Self {
        Self {
            resource_type: RESOURCE_TYPE_CLUSTER.to_string(),
            resource_id: identifier.to_string(),
        }
    }

    /// The identity of the account this emulator serves: the type segment an
    /// account is reported under, and that account's own 12-digit identifier —
    /// the account segment every ARN this service mints carries.
    ///
    /// The account is what HOLDS the clusters of this service, so it is the
    /// resource a refusal about a per-account collection reports; an account id
    /// is not an identifier of any resource stored under another type segment
    /// (`ClusterId` is `^[a-z0-9]{26}$`), so it is reported with this type and
    /// never with another's.
    pub fn account() -> Self {
        Self {
            resource_type: RESOURCE_TYPE_ACCOUNT.to_string(),
            resource_id: ::engine::emulator_account_id().to_string(),
        }
    }

    /// The identity of the stream `identifier` names: the type segment a stream is
    /// stored under, and that stream's OWN identifier — never the identifier of
    /// the cluster the stream is stored on.
    pub fn stream(identifier: &str) -> Self {
        Self {
            resource_type: RESOURCE_TYPE_STREAM.to_string(),
            resource_id: identifier.to_string(),
        }
    }

    /// The modelled `ConflictException` for a conflict about THIS resource, worded
    /// with `message`.
    ///
    /// The one place a conflict's identity pair is filled, so every refusal that
    /// names a resource names one resource's own type and own identifier.
    pub fn conflict(&self, message: String) -> ConflictException {
        ConflictException {
            message,
            resource_id: Some(self.resource_id.clone()),
            resource_type: Some(self.resource_type.clone()),
        }
    }

    /// The modelled `ConflictException` a call whose `clientToken` is already
    /// recorded against a DIFFERENT request answers with, reported against this
    /// resource — the one the refused call addressed.
    pub fn client_token_mismatch(&self) -> ConflictException {
        self.conflict(CLIENT_TOKEN_MISMATCH_MESSAGE.to_string())
    }

    /// The modelled `ServiceQuotaExceededException` for a collection THIS resource
    /// holds being at its ceiling: the resource whose quota was exceeded is the one
    /// that holds the collection, so the refusal reports that resource's own type
    /// and identifier, `message` says which ceiling was reached and `quota_code`
    /// names the quota.
    pub fn quota_exceeded(
        &self,
        message: String,
        quota_code: &str,
    ) -> ServiceQuotaExceededException {
        ServiceQuotaExceededException {
            message,
            resource_id: self.resource_id.clone(),
            resource_type: self.resource_type.clone(),
            // The Service Quotas service code of Aurora DSQL, which is the same
            // string as the service segment of its ARNs.
            service_code: ARN_SERVICE.to_string(),
            quota_code: quota_code.to_string(),
        }
    }
}

/// The modelled `ResourceNotFoundException` for a resource this service does not
/// hold, worded the way AWS words it on the operations addressed by an IDENTIFIER:
/// the message NAMES THE ARN the call addressed, which is how a caller learns which
/// resource was missing. Every member of the shape is `@required`, so all three are
/// filled.
///
/// The ARN-addressed tagging surface states its not-found differently
/// ([`tagging_resource_not_found`]), so this is not reached from there.
pub fn resource_not_found(
    arn: &str,
    resource_id: &str,
    resource_type: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("The resource with ARN {arn} doesn't exist. Verify the ARN and try again."),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// The modelled `ValidationException` for an input member whose value failed a
/// requirement this service enforces itself (one the model cannot express, so the
/// boundary validator passed the request through).
///
/// Every such refusal is built HERE rather than per handler, because the shape has
/// members a client reads: `message` and the `@required` `reason` are always filled,
/// and `fieldList` names the member and the requirement it failed — so a refusal is
/// never the synthetic single-member `smithy.framework#ValidationException`, which
/// carries no `reason` at all.
pub fn invalid_member_value(member: &str, requirement: &str, value: &str) -> ValidationException {
    ValidationException {
        message: format!("{member} {requirement}, got {value}"),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: requirement.to_string(),
        }]),
    }
}

/// The modelled `ValidationException` for a `resourceArn`-shaped member whose value
/// names no resource of this service at all, as [`parse_resource_arn`] reads it.
///
/// It names the member and the requirement the value failed, the way the boundary
/// validator does, so the caller can fix the request from the message alone, and it
/// fills the shape's `@required` `reason` so a client that branches on it can.
pub fn invalid_resource_arn(member: &str, value: &str) -> ValidationException {
    invalid_member_value(
        member,
        &format!(
            "must be the ARN of an Aurora DSQL resource \
             (arn:<partition>:{ARN_SERVICE}:<region>:<account>:<type>/<id>)"
        ),
        value,
    )
}

/// The member a cluster identifier arrives in on the operations addressed at a
/// cluster (`smithy_query(query="operation", operation="GetCluster,DeleteCluster,
/// UpdateCluster,GetClusterPolicy,PutClusterPolicy,DeleteClusterPolicy,
/// GetVpcEndpointServiceName")`: `identifier`).
pub const IDENTIFIER_MEMBER: &str = "identifier";

/// The form an identifier of this service has to be in, as a refusal of a value
/// that is not in it states it: 26 characters of the base32 alphabet a minted
/// identifier is rendered with ([`renders_a_minted_identifier`]).
const IDENTIFIER_FORMAT: &str = "a valid base32-encoded 26-character string";

/// Check a CLUSTER identifier on an operation that states the minted value RANGE
/// as a precondition of its own: the value has to be one the minting range covers
/// ([`renders_a_minted_identifier`]), or the call is refused before the cluster is
/// looked up.
///
/// This is the ONLY check an identifier meets beyond its shape's `@pattern`, and
/// only these operations state it. Which ones they are is not something the model
/// says — `ClusterId` carries one `@pattern` and every cluster-addressed operation
/// models both `ValidationException` and `ResourceNotFoundException` — so it is
/// read off the wire behaviour recorded for each one: `GetCluster`,
/// `DeleteCluster` and `GetClusterPolicy` answer a value outside the range with
/// this refusal (recorded in `dsql-cluster-error-paths` steps task#1 and task#2
/// and in `dsql-request-validation-error-surface` step task#5), while every other
/// identifier-addressed operation resolves each value of the pattern against the
/// store and reports an absent resource ([`cluster_not_found`] /
/// [`stream_not_found`], recorded in `dsql-missing-cluster-error-surface` steps
/// task#1 to task#5 and in `dsql-cluster-subresource-guards` steps task#4 and
/// task#5). The precondition is about the VALUE and not about the request that
/// carried it, so each of those three applies it to every value it is sent.
///
/// `member` is the input member the value arrived in ([`IDENTIFIER_MEMBER`] on
/// every operation that states this precondition), so the refusal names what the
/// caller has to fix and quotes the value it rejected.
pub fn validate_minted_cluster_identifier(
    member: &str,
    value: &str,
) -> Result<(), ValidationException> {
    if renders_a_minted_identifier(value) {
        return Ok(());
    }
    Err(invalid_cluster_identifier(member, value))
}

/// The modelled `ValidationException` a cluster identifier outside the form is
/// refused with, worded the way Aurora DSQL words that case: the sentence names
/// the member and quotes the value, and `fieldList` states the form the value had
/// to be in.
fn invalid_cluster_identifier(member: &str, value: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "Invalid input '{member}: {value}'. The cluster identifier format is invalid. \
             Provide a valid cluster identifier and try again."
        ),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: format!("Member must be {IDENTIFIER_FORMAT}"),
        }]),
    }
}

/// The member a stream identifier arrives in on the operations addressed at a
/// stream (`smithy_query(query="operation", operation="GetStream,DeleteStream")`:
/// `streamIdentifier`, an `@httpLabel` targeting `StreamId`).
pub const STREAM_IDENTIFIER_MEMBER: &str = "streamIdentifier";

/// Check a STREAM identifier against the encoding a stream id is written in: the
/// value has to be [`RESOURCE_ID_LEN`] symbols of [`CROCKFORD_SYMBOLS`] — a
/// base32-encoded identifier ([`decode_resource_identifier`] reads exactly those)
/// — or the call is refused before the cluster or the stream is looked up.
///
/// `StreamId`'s `@pattern` is wider than the encoding: `^[a-z0-9]{26}$` admits the
/// four letters base32 leaves out (`i` and `l` read as `1`, `o` as `0`, and `u` is
/// left out of the encoding altogether), so a value inside the pattern can be no
/// base32-encoded identifier at all. Such a value names no stream that could
/// exist — `StreamArn`'s own path runs over the same encoded id, so it could not
/// appear in one — and both operations that take this label answer it as a
/// malformed request instead of as an absent resource, before any lookup runs
/// (recorded in `dsql-cdc-stream-error-handling` steps task#5 and task#6). A value
/// the encoding DOES spell is an identifier, resolved against the store like any
/// other and reported as [`stream_not_found`] when the cluster does not hold it.
///
/// `member` is the input member the value arrived in
/// ([`STREAM_IDENTIFIER_MEMBER`] on both of them), so the refusal names what the
/// caller has to fix.
pub fn validate_stream_identifier(member: &str, value: &str) -> Result<(), ValidationException> {
    if decode_resource_identifier(value).is_some() {
        return Ok(());
    }
    Err(unreadable_identifier(member))
}

/// The modelled `ValidationException` an identifier that is no base32-encoded
/// identifier at all is refused with, in Aurora DSQL's sentence for a member whose
/// value it cannot READ as the thing that member carries
/// ([`unreadable_member_message`]): it names the member and the format the value
/// had to be in, and echoes nothing the caller sent — a value outside the encoding
/// is not an identifier to quote back. `fieldList` names the member that failed, so
/// the refusal carries the same members as every other identifier refusal here.
fn unreadable_identifier(member: &str) -> ValidationException {
    ValidationException {
        message: unreadable_member_message(member, &format!("the format '{IDENTIFIER_FORMAT}'")),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: format!("Member must be {IDENTIFIER_FORMAT}"),
        }]),
    }
}

/// The prefix that marks a tag key as belonging to AWS rather than to the caller.
///
/// A tag under this prefix is written and maintained by AWS itself, so a caller can
/// neither create nor change nor remove one, and such tags are not counted against
/// a resource's tag ceiling ([`MAX_TAGS_PER_RESOURCE`]) either. `TagKey`'s modelled
/// pattern admits `:` (`smithy_query(query="shape", shape="TagKey")`), so the model
/// cannot express the rule and every operation that accepts tag keys enforces it
/// here instead.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The member name a tag MAP's refusal names, as the operations that accept one
/// spell it on the wire (`smithy_query(query="operation",
/// operation="TagResource,CreateCluster,CreateStream")`: `tags`).
pub const TAGS_MEMBER: &str = "tags";

/// The member name a tag KEY LIST's refusal names, as `UntagResource` spells it on
/// the wire (`smithy_query(query="operation", operation="UntagResource")`:
/// `tagKeys`).
pub const TAG_KEYS_MEMBER: &str = "tagKeys";

/// The requirement a caller-supplied tag key has to satisfy, as the refusal words
/// it: the subject is the MEMBER the refusal names, so the sentence reads about the
/// key the member carries, and it names the prefix and why it is refused, so the
/// caller can fix the request from the message alone.
const RESERVED_TAG_KEY_REQUIREMENT: &str = "must not contain a key beginning with \
     the prefix aws:, which is reserved for tags created and maintained by AWS";

/// Whether `key` lies in the namespace [`RESERVED_TAG_KEY_PREFIX`] reserves.
///
/// The prefix is matched ignoring ASCII case: it names a namespace rather than one
/// literal spelling, so accepting a differently-cased spelling of it would let a
/// caller write a tag that reads as an AWS-owned one.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}

/// Check the tag keys a request carries against the reserved namespace, refusing
/// the WHOLE request when any one of them lies in it.
///
/// `member` is the input member the keys came from ([`TAGS_MEMBER`] for a map a
/// request writes, [`TAG_KEYS_MEMBER`] for the keys a request removes), so the
/// refusal names what the caller has to fix. Every key is inspected before any is
/// applied, which is what leaves a refused call's resource exactly as it was, and
/// the key the message names is the lexicographically first reserved one, so a
/// request whose keys arrive in any order is refused identically.
pub fn validate_tag_keys<'a, K>(member: &str, keys: K) -> Result<(), ValidationException>
where
    K: IntoIterator<Item = &'a str>,
{
    match keys
        .into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .min()
    {
        Some(reserved) => Err(invalid_member_value(
            member,
            RESERVED_TAG_KEY_REQUIREMENT,
            reserved,
        )),
        None => Ok(()),
    }
}

/// The special characters a caller-supplied tag key or value may carry, beside
/// letters, digits and the space.
///
/// The character set is the one both tag shapes' modelled pattern spells
/// (`smithy_query(query="shape", shape="TagKey,TagValue")`:
/// `^[a-zA-Z0-9_.:/=+\-@ ]*$`), so a key and a value are held to the same set — and
/// to the set the boundary constraint walk holds a tag VALUE to, which is where that
/// pattern is already enforced.
const TAG_SPECIAL_CHARACTERS: &str = "_.:/=+-@ ";

/// The sentence a tag outside that character set is refused with, in full.
///
/// FIXED text for the error class: nothing of the refused request is interpolated
/// into it, not the key or value that failed and not the member it arrived in.
pub const TAG_CHARACTER_SET_MESSAGE: &str = "Tags can only contain letters, numbers, spaces, and \
     the following special characters: _ . : / = + - @";

/// The wire error code a request BODY Aurora DSQL will not take is refused with,
/// and the status it carries.
///
/// Not a modelled shape: the service's REST frontend answers this class — a tag map
/// over its modelled entry count, and a tag key or value outside
/// [`TAG_SPECIAL_CHARACTERS`]'s character set — before any operation's own errors
/// are reachable, so the body carries the sentence for the class and nothing else.
pub const BAD_REQUEST_CODE: &str = "BadRequestException";
/// See [`BAD_REQUEST_CODE`].
pub const BAD_REQUEST_STATUS: u16 = 400;

/// Whether `value` is built only from the characters a tag key or value admits.
pub fn is_within_the_tag_character_set(value: &str) -> bool {
    value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || TAG_SPECIAL_CHARACTERS.contains(c))
}

/// Check the tag keys and values a request carries against that character set,
/// refusing the WHOLE request when any one of them leaves it.
///
/// Every operation that accepts caller-supplied tags runs this before it writes
/// anything, so a refused call leaves the addressed resource exactly as it was and
/// mints nothing. The refusal is the frontend's [`BAD_REQUEST_CODE`] carrying
/// [`TAG_CHARACTER_SET_MESSAGE`] — the same answer `crate::service`'s refusal path
/// gives for the tag VALUES the boundary walk itself catches, so one request cannot
/// be refused two ways depending on which half of the pair failed.
pub fn validate_tag_character_set<'a, E, V>(
    values: V,
) -> Result<(), crate::scaffolded::Scaffolded<E>>
where
    V: IntoIterator<Item = &'a str>,
{
    if values.into_iter().all(is_within_the_tag_character_set) {
        return Ok(());
    }
    Err(crate::scaffolded::Scaffolded::bad_request(
        TAG_CHARACTER_SET_MESSAGE,
    ))
}

/// The `ValidationExceptionReason` for input this service could not read as the
/// document kind the member carries (`smithy_query(query="shape",
/// shape="ValidationExceptionReason")`: cannotParse | deletionProtectionEnabled |
/// fieldValidationFailed | other | unknownOperation).
pub const VALIDATION_REASON_CANNOT_PARSE: &str = "cannotParse";

/// See [`VALIDATION_REASON_CANNOT_PARSE`]: the reason for a refusal that is about
/// the MEANING of a well-formed value rather than its form.
pub const VALIDATION_REASON_OTHER: &str = "other";

/// The action a principal needs to replace a cluster's resource-based policy — the
/// one the lockout safety check is about. A policy that denies this to everybody
/// can never be replaced.
pub const POLICY_MANAGEMENT_ACTION: &str = "dsql:PutClusterPolicy";

/// Check a resource-based policy document a request asks this service to attach to
/// a cluster, refusing the WHOLE request before the document is persisted.
///
/// Every operation that WRITES a cluster's policy enforces this here and refuses
/// identically, because the document ends up in one place — the cluster's own
/// policy record, which `GetClusterPolicy` reports — so a document one entry point
/// would refuse cannot be reachable through the other:
///
/// * the value has to be a JSON policy document (an object) before anything about
///   it can be reasoned about, and one that is not is refused as unparseable;
/// * unless the caller sets the operation's own `bypassPolicyLockoutSafetyCheck`,
///   which exists to let a caller accept exactly this risk, a document that leaves
///   NOBODY able to replace it is refused — `cluster_arn` is the ARN of the cluster
///   the document would be attached to, which is the resource the document's
///   `Resource` set is read against.
pub fn validate_cluster_policy(
    document: &str,
    cluster_arn: &str,
    bypass_lockout_safety_check: bool,
) -> Result<(), ValidationException> {
    let Some(parsed) = parse_policy(document) else {
        return Err(ValidationException {
            message: unreadable_member_message(POLICY_MEMBER, POLICY_DOCUMENT_FORM),
            reason: VALIDATION_REASON_CANNOT_PARSE.to_string(),
            field_list: None,
        });
    };
    if !bypass_lockout_safety_check && locks_out_every_principal(&parsed, cluster_arn) {
        return Err(ValidationException {
            message: format!(
                "The policy would prevent every principal from calling \
                 {POLICY_MANAGEMENT_ACTION} on this cluster. Set \
                 bypassPolicyLockoutSafetyCheck to true to apply it anyway."
            ),
            reason: VALIDATION_REASON_OTHER.to_string(),
            field_list: None,
        });
    }
    Ok(())
}

/// The member a resource-based policy document arrives in, on both operations that
/// write one (`smithy_query(query="operation",
/// operation="CreateCluster,PutClusterPolicy")`: `policy`).
pub const POLICY_MEMBER: &str = "policy";

/// The form a policy document had to be in, as [`unreadable_member_message`] names
/// it: the document is a whole policy language rather than a value with a spellable
/// shape, so the sentence asks for a valid format instead of quoting a grammar.
const POLICY_DOCUMENT_FORM: &str = "a valid format";

/// Aurora DSQL's sentence for a member whose value it cannot read as the thing that
/// member carries: it names the member and the form the value had to take, and
/// echoes nothing the caller sent.
///
/// `form` completes "Provide the {member} in …", so it is a named format ("the format
/// '…'") or the bare requirement ("a valid format"). The one place this family of
/// refusals is worded, so every member refused for the same reason — an identifier
/// that is no identifier, a policy document that is no policy — is refused in the
/// same sentence.
fn unreadable_member_message(member: &str, form: &str) -> String {
    format!(
        "Aurora DSQL can't process the request because the {member} that you specified \
         isn't valid. Provide the {member} in {form} and try again."
    )
}

/// Every identity the statements of the policy `document` GRANT TO: the values of
/// their `Principal` elements — the element a resource-based policy states its
/// grantees in — flattened out of the bare string, the list or the
/// principal-type block each of them may be written as ([`principal_entries`]), in
/// the order the document names them and without repeats, because the same identity
/// named twice is one identity to resolve.
///
/// `NotPrincipal` states the principals a statement EXEMPTS rather than the ones it
/// grants to, so nothing is resolved out of it: a statement that exempts an identity
/// nobody holds still grants to everyone else.
///
/// A value that is no policy document names nobody rather than failing here: what
/// makes a document unusable is stated by [`validate_cluster_policy`], once.
///
/// The repeats are dropped through a membership SET rather than by rescanning what
/// has been named already, so one request's document costs one pass over its
/// entries however many of them it carries: the document's own ceiling is
/// `PolicyDocument`'s `@length(1..20480)` (`smithy_query(query="shape",
/// shape="PolicyDocument")`), decided by the boundary constraint walk, and a
/// document right at it still names enough principals for a rescan per entry to
/// be work this service does not need to do.
pub fn policy_principals(document: &str) -> Vec<String> {
    let Some(parsed) = parse_policy(document) else {
        return Vec::new();
    };
    let statements = statements(&parsed);
    let mut named = HashSet::new();
    statements
        .iter()
        .filter_map(|statement| statement.get("Principal"))
        .flat_map(principal_entries)
        .filter(|principal| named.insert(principal.clone()))
        .collect()
}

/// The refusal a policy document earns when one of the identities it GRANTS TO is
/// not there, or `None` when every identity it names resolves.
///
/// A resource-based policy states who a permission is granted to, so the identities
/// a submitted document names are resolved against this account's IAM before the
/// document is stored — a grant to an identity that does not exist is a grant to
/// nobody — and every operation that writes a cluster's policy refuses such a
/// document with the modelled `ValidationException`, leaving the policy the cluster
/// already carries, and the version it carries it under, exactly as they were.
///
/// The resolution itself is [`::service_common::iam_principal::identity_is_absent`],
/// the tree's one reading of a policy principal against IAM, so a refusal is
/// grounded in what IAM ANSWERED: a principal that names no IAM role or user of this
/// account — the wildcard, an account or its root, a service or federated principal,
/// an identity of another account whose IAM this emulator is not the one to ask — is
/// resolved by nothing here and refuses nothing.
pub async fn unresolved_policy_principal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    document: &str,
) -> Option<ValidationException> {
    for principal in policy_principals(document) {
        if ::service_common::iam_principal::identity_is_absent(ctx, region, &principal).await {
            return Some(ValidationException {
                message: inapplicable_policy_message(POLICY_MEMBER),
                reason: VALIDATION_REASON_OTHER.to_string(),
                field_list: None,
            });
        }
    }
    None
}

/// Aurora DSQL's sentence for a policy document it cannot apply: it names the member
/// the document arrived in and the elements a document it can apply carries, and
/// echoes nothing of the document itself.
///
/// The one place that refusal is worded, so both operations that write a cluster's
/// policy state it identically.
fn inapplicable_policy_message(member: &str) -> String {
    format!(
        "Invalid input for '{member}'. Aurora DSQL can't apply the policy because the \
         policy document isn't valid. Make sure your policy includes all required \
         elements (Version, Statement, Principal, Action, and Resource) in valid JSON \
         format and try again."
    )
}

/// Parse a `PolicyDocument` into its JSON object, or `None` when it is not one.
fn parse_policy(document: &str) -> Option<::serde_json::Map<String, ::serde_json::Value>> {
    match ::serde_json::from_str::<::serde_json::Value>(document) {
        Ok(::serde_json::Value::Object(object)) => Some(object),
        _ => None,
    }
}

/// Whether `document` denies EVERY principal the action that replaces it on the
/// cluster named by `resource_arn`, which is what the lockout safety check exists
/// to catch.
///
/// Read the way an IAM resource policy is read: an explicit `Deny` beats every
/// `Allow`, so a `Deny` statement that covers all principals, covers
/// [`POLICY_MANAGEMENT_ACTION`], covers the cluster being protected and carries no
/// `Condition` narrowing it leaves nobody able to call that action again. Each of
/// the three sets is read in whichever of its two forms the statement spells it,
/// the positive element or its negated complement — `Principal`/`NotPrincipal`,
/// `Action`/`NotAction`, `Resource`/`NotResource` — because the negated form states
/// the same set by naming what it leaves out.
fn locks_out_every_principal(
    document: &::serde_json::Map<String, ::serde_json::Value>,
    resource_arn: &str,
) -> bool {
    // The account the policy protects is the account segment of the ARN it is
    // attached to: a principal of that account is who the check asks about.
    let account = parse_resource_arn(resource_arn)
        .map(|arn| arn.account)
        .unwrap_or_default();
    statements(document).iter().any(|statement| {
        effect_is(statement, "Deny")
            && !statement.contains_key("Condition")
            && covers_every_principal(statement, &account)
            && covers_management_action(statement)
            && covers_this_resource(statement, resource_arn)
    })
}

/// The `Statement` elements of a policy document: IAM accepts a single statement
/// object as well as a list of them.
fn statements(
    document: &::serde_json::Map<String, ::serde_json::Value>,
) -> Vec<::serde_json::Map<String, ::serde_json::Value>> {
    match document.get("Statement") {
        Some(::serde_json::Value::Array(entries)) => entries
            .iter()
            .filter_map(|entry| entry.as_object().cloned())
            .collect(),
        Some(::serde_json::Value::Object(one)) => vec![one.clone()],
        _ => Vec::new(),
    }
}

/// Whether the statement's `Effect` is `effect`, case-insensitively as IAM reads
/// it.
fn effect_is(statement: &::serde_json::Map<String, ::serde_json::Value>, effect: &str) -> bool {
    statement
        .get("Effect")
        .and_then(::serde_json::Value::as_str)
        .is_some_and(|stated| stated.eq_ignore_ascii_case(effect))
}

/// Whether the statement applies to every principal that could still replace the
/// policy: `"Principal": "*"`, or a `Principal` block whose entries include `*`.
///
/// `NotPrincipal` is the complement, so the statement applies to every principal it
/// does NOT name — which still leaves the policy replaceable when one of the
/// principals it exempts could call [`POLICY_MANAGEMENT_ACTION`] on a resource of
/// `account_id` ([`can_manage_policy`]), and locks everybody out otherwise.
fn covers_every_principal(
    statement: &::serde_json::Map<String, ::serde_json::Value>,
    account_id: &str,
) -> bool {
    if let Some(principals) = statement.get("Principal") {
        return principal_entries(principals)
            .iter()
            .any(|principal| principal == "*");
    }
    if let Some(not_principals) = statement.get("NotPrincipal") {
        return !principal_entries(not_principals)
            .iter()
            .any(|principal| can_manage_policy(principal, account_id));
    }
    false
}

/// Every principal a statement's `Principal` or `NotPrincipal` element names: IAM
/// writes the element as the bare string `*`, or as a block keyed by principal type
/// (`AWS`, `Service`, …) whose entries are a string or a list of them.
fn principal_entries(element: &::serde_json::Value) -> Vec<String> {
    match element {
        ::serde_json::Value::Object(block) => block.values().flat_map(string_values).collect(),
        other => string_values(other),
    }
}

/// Whether `principal` is one that could still call [`POLICY_MANAGEMENT_ACTION`] on
/// a resource of `account_id`: every principal (`*`), or a principal of that
/// account — an account id written on its own, or the account segment of a
/// principal ARN. A principal of any OTHER account, and a service or federated
/// principal (which carries no account segment at all), could not, so exempting one
/// of those from a `Deny` leaves nobody who can.
///
/// No wildcard matching beyond the whole-element `*`: IAM reads a `Principal` entry
/// as an exact value, not as a pattern.
fn can_manage_policy(principal: &str, account_id: &str) -> bool {
    if principal == "*" {
        return true;
    }
    if account_id.is_empty() {
        return false;
    }
    principal == account_id || principal.split(':').nth(4) == Some(account_id)
}

/// Whether the statement's `Action` set covers [`POLICY_MANAGEMENT_ACTION`].
/// `NotAction` is the complement, so it covers the action unless it names it.
fn covers_management_action(statement: &::serde_json::Map<String, ::serde_json::Value>) -> bool {
    if let Some(actions) = statement.get("Action") {
        return string_values(actions)
            .iter()
            .any(|action| matches_pattern(action, POLICY_MANAGEMENT_ACTION));
    }
    if let Some(not_actions) = statement.get("NotAction") {
        return !string_values(not_actions)
            .iter()
            .any(|action| matches_pattern(action, POLICY_MANAGEMENT_ACTION));
    }
    false
}

/// Whether the statement's `Resource` set covers the cluster the policy is being
/// attached to, named by `resource_arn`.
///
/// An entry covers that cluster when the entry, read as an IAM resource pattern
/// (`*` any run of characters, `?` any one), matches the cluster's own ARN: the
/// literal ARN of the cluster being protected covers it exactly as a wildcard
/// spanning it does, and an ARN naming any other resource covers nothing here. An
/// absent `Resource` is the whole resource in a resource-based policy, which is
/// this cluster; `NotResource` is the complement, so it covers this cluster unless
/// it names it.
fn covers_this_resource(
    statement: &::serde_json::Map<String, ::serde_json::Value>,
    resource_arn: &str,
) -> bool {
    if let Some(resources) = statement.get("Resource") {
        return string_values(resources)
            .iter()
            .any(|resource| matches_pattern(resource, resource_arn));
    }
    if let Some(not_resources) = statement.get("NotResource") {
        return !string_values(not_resources)
            .iter()
            .any(|resource| matches_pattern(resource, resource_arn));
    }
    true
}

/// Every string in a JSON value that IAM accepts as a string or a list of them.
fn string_values(value: &::serde_json::Value) -> Vec<String> {
    match value {
        ::serde_json::Value::String(one) => vec![one.clone()],
        ::serde_json::Value::Array(many) => many
            .iter()
            .filter_map(::serde_json::Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether an IAM pattern (`*` matches any run of characters, `?` any one)
/// matches `candidate`, case-insensitively as IAM matches action names.
fn matches_pattern(pattern: &str, candidate: &str) -> bool {
    let pattern: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let candidate: Vec<char> = candidate.to_ascii_lowercase().chars().collect();
    // Classic two-cursor wildcard match: on a mismatch, fall back to the last
    // `*` and let it consume one more character. Linear in the candidate.
    let (mut p, mut c) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut resume = 0usize;
    while c < candidate.len() {
        if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            p += 1;
            resume = c;
        } else if p < pattern.len() && (pattern[p] == '?' || pattern[p] == candidate[c]) {
            p += 1;
            c += 1;
        } else if let Some(last_star) = star {
            p = last_star + 1;
            resume += 1;
            c = resume;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|symbol| *symbol == '*')
}

/// The member a witness-Region refusal names, spelled with its path from the
/// request root the way the operation's own documentation spells it
/// (`multiRegionProperties.witnessRegion`), so a caller reading `fieldList` knows
/// which nested member to fix.
pub const WITNESS_REGION_MEMBER: &str = "multiRegionProperties.witnessRegion";

/// The requirement a `multiRegionProperties.witnessRegion` has to satisfy, as the
/// refusal words it.
const WITNESS_REGION_REQUIREMENT: &str = "must name a Region other than the cluster's own Region";

/// Check a `multiRegionProperties` against the one constraint this service enforces
/// on it itself: a multi-Region cluster's WITNESS Region is a third Region that
/// holds a quorum vote outside the cluster's own, so it can never be the cluster's
/// own Region.
///
/// Both operations that accept the member document the constraint in the same
/// sentence — "The witness Region specified in multiRegionProperties.witnessRegion
/// cannot be the same as the cluster's Region" (`smithy_query(query="operation",
/// operation="CreateCluster,UpdateCluster")`) — and the model cannot express it (it
/// compares one member against the cluster's Region), so the boundary validator
/// passes such a request through and the handler refuses it here, identically for
/// both. A request carrying no `multiRegionProperties`, or one carrying no
/// `witnessRegion` to compare, names no witness and so satisfies it.
///
/// Region names are compared ignoring surrounding whitespace and ASCII case: those
/// differences spell the same Region, and a refusal that a different spelling slips
/// past would persist exactly the configuration the constraint forbids.
pub fn validate_witness_region(
    properties: Option<&MultiRegionProperties>,
    cluster_region: &str,
) -> Result<(), ValidationException> {
    let Some(witness) = properties.and_then(|properties| properties.witness_region.as_deref())
    else {
        return Ok(());
    };
    if witness.trim().eq_ignore_ascii_case(cluster_region.trim()) {
        return Err(invalid_member_value(
            WITNESS_REGION_MEMBER,
            WITNESS_REGION_REQUIREMENT,
            witness,
        ));
    }
    Ok(())
}

/// The member a peer-cluster-list refusal names, spelled with its path from the
/// request root the way the operations' own documentation spells it
/// (`multiRegionProperties.clusters`), so a caller reading `fieldList` knows which
/// nested member to fix.
pub const PEER_CLUSTERS_MEMBER: &str = "multiRegionProperties.clusters";

/// The most peer clusters one multi-Region configuration may name.
///
/// A ceiling this emulator CHOOSES, not a published quota: `ClusterArnList`
/// carries no `@length` (`smithy_query(query="shape",
/// shape="MultiRegionProperties")`), yet the list is held on the cluster record
/// for the life of the process and is walked whenever a peering is resolved
/// ([`multi_region_setup_complete`]), so without a maximum one request would
/// decide how much memory and work every later one spends. A multi-Region cluster
/// is formed from the clusters peered with it, so a handful is the shape of the
/// member.
pub const MAX_MULTI_REGION_PEERS: usize = 8;

/// Check a `multiRegionProperties` against [`MAX_MULTI_REGION_PEERS`].
///
/// An input-constraint refusal the model cannot express, so every operation that
/// WRITES the member checks it here and refuses identically — with the
/// `ValidationException` both of them model, before anything is created or
/// changed.
pub fn validate_multi_region_peers(
    properties: Option<&MultiRegionProperties>,
) -> Result<(), ValidationException> {
    let peers = properties
        .and_then(|properties| properties.clusters.as_deref())
        .unwrap_or_default();
    if peers.len() > MAX_MULTI_REGION_PEERS {
        return Err(invalid_member_value(
            PEER_CLUSTERS_MEMBER,
            &format!("must name at most {MAX_MULTI_REGION_PEERS} peer clusters"),
            &peers.len().to_string(),
        ));
    }
    Ok(())
}

/// The cluster a `multiRegionProperties.clusters` entry names, when this service
/// holds it.
///
/// A peer is named by ARN, and an ARN carries the partition, Region and account
/// its cluster was minted in, so an entry resolves only to a cluster MINTED with
/// that very ARN ([`cluster_arn`]): a cluster of another Region, account or
/// partition that happens to share an identifier is a different cluster.
fn peer_cluster<'a>(
    clusters: &'a BTreeMap<String, Cluster>,
    peer_arn: &str,
) -> Option<&'a Cluster> {
    let resource = parse_resource_arn(peer_arn)?;
    if resource.resource_type != RESOURCE_TYPE_CLUSTER {
        return None;
    }
    clusters
        .get(&resource.resource_id)
        .filter(|cluster| cluster.arn == peer_arn)
}

/// Whether `cluster`'s multi-Region configuration is established.
///
/// A peering is a relationship both clusters carry, so it is established when
/// the cluster names at least one peer and every peer it names is a cluster this
/// service holds whose own configuration names it back. A cluster that names a
/// peer which does not name it is still waiting for that other side to be
/// configured, and a configuration naming no peer at all (a witness Region
/// alone) has no peering yet either.
///
/// The walk is over the cluster's own peers, capped at
/// [`MAX_MULTI_REGION_PEERS`] by every operation that writes the member
/// ([`validate_multi_region_peers`]), so its cost is bounded by a constant.
pub fn multi_region_setup_complete(
    cluster: &Cluster,
    clusters: &BTreeMap<String, Cluster>,
) -> bool {
    let peers = cluster.multi_region_peers();
    !peers.is_empty()
        && peers.iter().all(|peer_arn| {
            peer_cluster(clusters, peer_arn).is_some_and(|peer| peer.names_peer(&cluster.arn))
        })
}

/// The status the clock owes a cluster whose creation or reconfiguration is
/// finishing: the cluster is usable ([`STATUS_ACTIVE`]) unless it is a
/// multi-Region cluster whose peering is not established on both sides, which
/// waits in [`STATUS_PENDING_SETUP`] until it is.
///
/// Shared by every operation that writes a cluster's configuration, so the
/// status a created cluster settles into and the one a reconfigured cluster
/// settles into are decided by one rule.
pub fn settled_cluster_status(
    cluster: &Cluster,
    clusters: &BTreeMap<String, Cluster>,
) -> &'static str {
    if cluster.is_multi_region() && !multi_region_setup_complete(cluster, clusters) {
        STATUS_PENDING_SETUP
    } else {
        STATUS_ACTIVE
    }
}

/// The modelled `ValidationException` for a `nextToken` this service never issued,
/// as [`decode_pagination_cursor`] reads it.
///
/// Aurora DSQL states this failure with the FIXED sentence
/// [`INVALID_PAGINATION_TOKEN_MESSAGE`], which belongs to the failure rather than to
/// the request: a cursor is opaque, so there is nothing about the value to report,
/// and the remedy is the same whatever was sent. The sentence is carried twice — in
/// `message` and in the single `fieldList` entry — and the entry names the cursor by
/// the name the caller spelled it with on the wire ([`NEXT_TOKEN_FIELD`]). The
/// function takes no argument, so no value of the refused request can reach either.
///
/// Shared by every paginated reader, so one fabricated cursor is refused the same
/// way whichever list operation received it — and with the modelled shape's
/// `@required` `reason` filled, not the synthetic `message`-only one.
pub fn unissued_pagination_token() -> ValidationException {
    ValidationException {
        message: INVALID_PAGINATION_TOKEN_MESSAGE.to_string(),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: NEXT_TOKEN_FIELD.to_string(),
            message: INVALID_PAGINATION_TOKEN_MESSAGE.to_string(),
        }]),
    }
}

/// The sentence Aurora DSQL refuses a pagination token it did not issue with. Fixed
/// text in full: it interpolates nothing, so the same sentence states the failure for
/// every rejected cursor of every paginated reader.
pub const INVALID_PAGINATION_TOKEN_MESSAGE: &str =
    "Invalid pagination token. Request a new token and try again.";

/// The name the paginated operations' cursor carries ON THE WIRE: the `@httpQuery`
/// parameter the `nextToken` member is bound to (`smithy_query(query="operation",
/// operation="ListClusters,ListStreams")`: `nextToken`, `httpQuery: "next-token"`).
///
/// A `fieldList` entry names the field the CALLER sent, so for a member carried in a
/// query parameter that is the parameter's name rather than the Smithy member name.
pub const NEXT_TOKEN_FIELD: &str = "next-token";

/// The name a cluster's and a stream's status member carries on the wire
/// (`smithy_query(query="operation",
/// operation="CreateCluster,UpdateCluster,CreateStream")`: `status`), and so the
/// key a recorded call keeps the status it answered under
/// ([`RecordedCall::answering`]).
pub const STATUS_MEMBER: &str = "status";

/// The name the policy operations' answer carries on the wire
/// (`smithy_query(query="operation",
/// operation="PutClusterPolicy,DeleteClusterPolicy")`: a `@required`
/// `policyVersion`), and so the key a recorded call keeps that answer under
/// ([`RecordedCall::answering`]).
pub const POLICY_VERSION_MEMBER: &str = "policyVersion";

/// The name a retired resource's ARN member carries on the wire
/// (`smithy_query(query="operation", operation="DeleteCluster,DeleteStream")`: a
/// `@required` `arn` on each), and so the key a recorded call keeps that answer
/// under ([`RecordedCall::answering`]).
pub const ARN_MEMBER: &str = "arn";

/// The name a cluster's deletion-protection member carries on the wire
/// (`smithy_query(query="operation", operation="CreateCluster")`: a `@required`
/// `deletionProtectionEnabled`), and so the key a recorded call keeps that
/// answer under ([`RecordedCall::answering`]).
pub const DELETION_PROTECTION_ENABLED_MEMBER: &str = "deletionProtectionEnabled";

/// The name a cluster's encryption member carries on the wire
/// (`smithy_query(query="operation", operation="CreateCluster")`:
/// `encryptionDetails`), and so the key a recorded call keeps that answer under
/// ([`RecordedCall::answering`]).
pub const ENCRYPTION_DETAILS_MEMBER: &str = "encryptionDetails";

/// The name a cluster's multi-Region member carries on the wire
/// (`smithy_query(query="operation", operation="CreateCluster")`:
/// `multiRegionProperties`), and so the key a recorded call keeps that answer
/// under ([`RecordedCall::answering`]).
pub const MULTI_REGION_PROPERTIES_MEMBER: &str = "multiRegionProperties";

/// The name a cluster's advanced-monitoring member carries on the wire
/// (`smithy_query(query="operation", operation="CreateCluster")`: `metricGroups`),
/// and so the key a recorded call keeps that answer under
/// ([`RecordedCall::answering`], through [`encode_metric_groups`]).
pub const METRIC_GROUPS_MEMBER: &str = "metricGroups";

/// The name a retired resource's creation-time member carries on the wire
/// (`smithy_query(query="operation", operation="DeleteCluster,DeleteStream")`: a
/// `@required` `creationTime` on each), and so the key a recorded call keeps
/// that answer under ([`RecordedCall::answering`], through [`encode_instant`]).
pub const CREATION_TIME_MEMBER: &str = "creationTime";

/// How a [`RecordedCall`] keeps an instant it answered: whole nanoseconds since
/// the Unix epoch.
///
/// A recorded answer is a string, and a retry has to report the very same
/// timestamp the original call did, so the encoding is exact rather than
/// rounded to a second or to a float's precision. An instant before the epoch
/// cannot arise here — every stored instant is a `SystemTime::now()` this
/// process read — and encodes as `0`.
pub fn encode_instant(at: SystemTime) -> String {
    at.duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default()
        .to_string()
}

/// Read back an instant [`encode_instant`] wrote, or `None` for a string it did
/// not write.
pub fn decode_instant(recorded: &str) -> Option<SystemTime> {
    let nanos: u128 = recorded.parse().ok()?;
    let seconds = u64::try_from(nanos / 1_000_000_000).ok()?;
    let subsec = (nanos % 1_000_000_000) as u32;
    UNIX_EPOCH.checked_add(Duration::new(seconds, subsec))
}

/// What a served delete answered, as its `clientToken` ledger keeps it.
///
/// A delete's answer cannot be re-derived when its retry arrives: the resource
/// the call retired is by then on its way out or gone, and neither its current
/// status nor its absence is what that request answered. So the three members
/// every delete of this service answers beside the retired resource's own
/// identifier — [`ARN_MEMBER`], [`STATUS_MEMBER`] and [`CREATION_TIME_MEMBER`]
/// — are kept on the record, and this is the one codec that writes and reads
/// them: [`Self::into_record`] on the call that serves the delete,
/// [`Self::from_record`] on its retry. Each delete handler keeps only the shape
/// of its own output, which names the resource (and, for a nested one, its
/// parent) its own way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedDelete {
    /// The resource that call retired, as [`RecordedCall::resource`] names it.
    pub resource: String,
    /// The ARN the call answered.
    pub arn: String,
    /// The status the call answered.
    pub status: String,
    /// The creation time the call answered.
    pub creation_time: SystemTime,
}

impl RecordedDelete {
    /// This answer as the ledger stores it. Every member is kept, because the
    /// resource this record describes is on its way out of the store.
    pub fn into_record(self) -> RecordedCall {
        RecordedCall::new(&self.resource)
            .answering(STATUS_MEMBER, &self.status)
            .answering(ARN_MEMBER, &self.arn)
            .answering(CREATION_TIME_MEMBER, &encode_instant(self.creation_time))
    }

    /// The answer `recorded` holds, or `None` when it is missing any member a
    /// served delete records — a record a handler has to refuse over rather
    /// than serve an empty member from, since a delete always records all three.
    pub fn from_record(recorded: &RecordedCall) -> Option<Self> {
        Some(Self {
            resource: recorded.resource.clone(),
            arn: recorded.answered(ARN_MEMBER)?.to_string(),
            status: recorded.answered(STATUS_MEMBER)?.to_string(),
            creation_time: decode_instant(recorded.answered(CREATION_TIME_MEMBER)?)?,
        })
    }
}

/// What a served policy write — an attach or a removal — answered, as its
/// `clientToken` ledger keeps it.
///
/// A policy write's answer cannot be re-derived when its retry arrives: a later
/// write supersedes the version the call issued and a removal drops it
/// altogether, and neither the version attached by then nor the absence of one
/// is what that request answered. So the one member both policy writers answer
/// ([`POLICY_VERSION_MEMBER`]) is kept on the record, and this is the one codec
/// that writes and reads it: [`Self::into_record`] on the call that serves the
/// write, [`Self::from_record`] on its retry. Each handler keeps only the shape
/// of its own output and lifts the missing-member refusal into its own error
/// channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedPolicyWrite {
    /// The policy version the call answered — the one it issued, for an attach,
    /// or the one it removed, for a removal.
    pub policy_version: String,
}

impl RecordedPolicyWrite {
    /// This answer as the ledger stores it, against the cluster `resource` whose
    /// policy the call wrote.
    pub fn into_record(self, resource: &str) -> RecordedCall {
        RecordedCall::new(resource).answering(POLICY_VERSION_MEMBER, &self.policy_version)
    }

    /// The answer `recorded` holds, or `None` when it does not hold the member a
    /// served policy write records — a record a handler has to refuse over
    /// rather than answer an empty version from, since a served policy write
    /// always records the version it answered.
    pub fn from_record(recorded: &RecordedCall) -> Option<Self> {
        Some(Self {
            policy_version: recorded.answered(POLICY_VERSION_MEMBER)?.to_string(),
        })
    }
}

/// What a served call whose only un-derivable answer is a status answered, as
/// its `clientToken` ledger keeps it.
///
/// A status is the one member of such a call's output the store no longer
/// determines when its retry arrives: the clock settles the resource on from the
/// status the call reported, and a later call moves it again, so neither the
/// status the resource carries by then nor its absence is what that request
/// answered. So the one member these calls answer ([`STATUS_MEMBER`]) is kept on
/// the record, and this is the one codec that writes and reads it:
/// [`Self::into_record`] on the call that serves the request,
/// [`Self::from_record`] on its retry. Each handler keeps only the shape of its
/// own output — which names the resource (and, for a nested one, its parent) its
/// own way — and lifts the missing-member refusal into its own error channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedStatus {
    /// The status the call answered.
    pub status: String,
}

impl RecordedStatus {
    /// This answer as the ledger stores it, against the `resource` the call
    /// acted on.
    pub fn into_record(self, resource: &str) -> RecordedCall {
        RecordedCall::new(resource).answering(STATUS_MEMBER, &self.status)
    }

    /// The answer `recorded` holds, or `None` when it does not hold the member
    /// such a call records — a record a handler has to refuse over rather than
    /// answer the live resource's status from, since a served call always
    /// records the status it answered.
    pub fn from_record(recorded: &RecordedCall) -> Option<Self> {
        Some(Self {
            status: recorded.answered(STATUS_MEMBER)?.to_string(),
        })
    }
}

/// What a served `CreateCluster` answered that the cluster's own record no
/// longer determines once something else has run.
///
/// A minted cluster's `identifier`, `arn`, `creationTime` and `endpoint` are
/// fixed for its whole life, so a retry renders those from the stored record.
/// The members here are not: the clock moves `status` on, and `UpdateCluster`
/// writes `deletionProtectionEnabled`, `encryptionDetails`,
/// `multiRegionProperties` and `metricGroups` again. Each is therefore kept with the token and
/// served from the record, as [`RecordedCall`] requires of a value a later call
/// can supersede, so the retry answers the result of the original request.
#[derive(Debug, Clone)]
pub struct RecordedClusterCreation {
    /// The `status` the call answered.
    pub status: String,
    /// The `deletionProtectionEnabled` the call answered.
    pub deletion_protection_enabled: bool,
    /// The `encryptionDetails` the call answered, if it answered any.
    pub encryption_details: Option<EncryptionDetails>,
    /// The `multiRegionProperties` the call answered, if it answered any.
    pub multi_region_properties: Option<MultiRegionProperties>,
    /// The `metricGroups` the call answered.
    pub metric_groups: std::collections::BTreeSet<String>,
}

impl RecordedClusterCreation {
    /// This answer as the ledger stores it, against the cluster `resource` the
    /// call minted.
    pub fn into_record(self, resource: &str) -> RecordedCall {
        RecordedCall::new(resource)
            .answering(STATUS_MEMBER, &self.status)
            .answering(
                DELETION_PROTECTION_ENABLED_MEMBER,
                &self.deletion_protection_enabled.to_string(),
            )
            .answering(
                ENCRYPTION_DETAILS_MEMBER,
                &encode_encryption_details(self.encryption_details.as_ref()),
            )
            .answering(
                MULTI_REGION_PROPERTIES_MEMBER,
                &encode_multi_region(self.multi_region_properties.as_ref()),
            )
            .answering(
                METRIC_GROUPS_MEMBER,
                &encode_metric_groups(&self.metric_groups),
            )
    }

    /// The answer `recorded` holds, or `None` when it is missing any member a
    /// served create records — a record a handler has to refuse over rather than
    /// serve a re-derived member from, since a served create records every one.
    pub fn from_record(recorded: &RecordedCall) -> Option<Self> {
        Some(Self {
            status: recorded.answered(STATUS_MEMBER)?.to_string(),
            deletion_protection_enabled: recorded
                .answered(DELETION_PROTECTION_ENABLED_MEMBER)?
                .parse()
                .ok()?,
            encryption_details: decode_encryption_details(
                recorded.answered(ENCRYPTION_DETAILS_MEMBER)?,
            )?,
            multi_region_properties: decode_multi_region(
                recorded.answered(MULTI_REGION_PROPERTIES_MEMBER)?,
            )?,
            metric_groups: decode_metric_groups(recorded.answered(METRIC_GROUPS_MEMBER)?)?,
        })
    }
}

/// How a [`RecordedCall`] keeps an `EncryptionDetails` it answered: the
/// structure's members as a JSON object, and JSON `null` for a call that
/// answered none.
///
/// A recorded answer is a string, so a structured member is encoded rather than
/// flattened: `null` is what distinguishes "this call answered no
/// `encryptionDetails`" from "no `encryptionDetails` was ever recorded", which
/// is the case [`RecordedCall::answered`] reserves `None` for.
fn encode_encryption_details(details: Option<&EncryptionDetails>) -> String {
    match details {
        None => ::serde_json::Value::Null.to_string(),
        Some(details) => ::serde_json::json!({
            "encryptionType": details.encryption_type,
            "kmsKeyArn": details.kms_key_arn,
            "encryptionStatus": details.encryption_status,
        })
        .to_string(),
    }
}

/// Read back the `encryptionDetails` [`encode_encryption_details`] wrote (the
/// outer `Option` is the decoding, the inner one the answer), or `None` for a
/// string it did not write.
fn decode_encryption_details(recorded: &str) -> Option<Option<EncryptionDetails>> {
    let encoded: ::serde_json::Value = ::serde_json::from_str(recorded).ok()?;
    if encoded.is_null() {
        return Some(None);
    }
    let object = encoded.as_object()?;
    Some(Some(EncryptionDetails {
        encryption_type: object.get("encryptionType")?.as_str()?.to_string(),
        kms_key_arn: decoded_text(object.get("kmsKeyArn"))?,
        encryption_status: object.get("encryptionStatus")?.as_str()?.to_string(),
    }))
}

/// How a [`RecordedCall`] keeps a `MultiRegionProperties` it answered, on the
/// same terms as [`encode_encryption_details`].
fn encode_multi_region(properties: Option<&MultiRegionProperties>) -> String {
    match properties {
        None => ::serde_json::Value::Null.to_string(),
        Some(properties) => ::serde_json::json!({
            "witnessRegion": properties.witness_region,
            "clusters": properties.clusters,
        })
        .to_string(),
    }
}

/// Read back the `multiRegionProperties` [`encode_multi_region`] wrote, on the
/// same terms as [`decode_encryption_details`]. The peer list keeps the order it
/// was answered in, which is the order the request carried.
fn decode_multi_region(recorded: &str) -> Option<Option<MultiRegionProperties>> {
    let encoded: ::serde_json::Value = ::serde_json::from_str(recorded).ok()?;
    if encoded.is_null() {
        return Some(None);
    }
    let object = encoded.as_object()?;
    let clusters = match object.get("clusters") {
        None | Some(::serde_json::Value::Null) => None,
        Some(::serde_json::Value::Array(peers)) => Some(
            peers
                .iter()
                .map(|peer| peer.as_str().map(str::to_string))
                .collect::<Option<Vec<String>>>()?,
        ),
        Some(_) => return None,
    };
    Some(Some(MultiRegionProperties {
        witness_region: decoded_text(object.get("witnessRegion"))?,
        clusters,
    }))
}

/// One optional string member of an encoded answer: absent or JSON `null` is a
/// member the call answered nothing for, a string is its value, and anything
/// else is not an encoding this module wrote.
fn decoded_text(member: Option<&::serde_json::Value>) -> Option<Option<String>> {
    match member {
        None | Some(::serde_json::Value::Null) => Some(None),
        Some(text) => Some(Some(text.as_str()?.to_string())),
    }
}

/// The wire error code the ARN-addressed tagging surface answers a `resourceArn` it
/// resolves no resource of this service for with.
///
/// NOT a shape this service's model declares — the model lists
/// `ResourceNotFoundException` (404) on the tagging operations — but the code the
/// service answers on that surface, as the behavioural corpus records it: the
/// operations bound to `/tags/{resourceArn}` are fronted by a tagging layer that
/// states its own refusal, while every operation addressed by an IDENTIFIER answers
/// the modelled `ResourceNotFoundException` naming the resource
/// ([`resource_not_found`]). The status and the code are what a client classifies on,
/// so they follow the recording rather than the model.
pub const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";

/// The status [`TAGGING_NOT_FOUND_CODE`] is served on — the same 404 the model gives
/// `ResourceNotFoundException`, a not-found being a 404 whichever code names it.
pub const TAGGING_NOT_FOUND_STATUS: u16 = 404;

/// The sentence [`TAGGING_NOT_FOUND_CODE`] carries. Fixed text in full: that surface
/// reports neither the ARN it refused nor any identifier or remediation, so one
/// sentence states the failure for every resource.
pub const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The refusal the tagging surface answers an ARN it resolves no resource for with,
/// in whichever tagging operation's error channel: HTTP [`TAGGING_NOT_FOUND_STATUS`]
/// with [`TAGGING_NOT_FOUND_CODE`] and [`TAGGING_NOT_FOUND_MESSAGE`], and no member
/// besides the sentence.
///
/// Built here rather than per handler, so the three operations bound to
/// `/tags/{resourceArn}` cannot disagree about the one refusal they share.
pub fn tagging_resource_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::not_found(TAGGING_NOT_FOUND_MESSAGE)
}

/// The modelled `ResourceNotFoundException` for a cluster this service does not
/// hold: [`resource_not_found`] over the ARN the addressed identifier has in the
/// request's own region.
pub fn cluster_not_found(region: &str, identifier: &str) -> ResourceNotFoundException {
    resource_not_found(
        &cluster_arn(region, identifier),
        identifier,
        RESOURCE_TYPE_CLUSTER,
    )
}

/// The modelled `ResourceNotFoundException` for a stream a cluster does not hold:
/// [`resource_not_found`] over the ARN a stream of that identifier would have had
/// under THAT cluster.
///
/// `cluster_region` is the region the cluster this call resolved LIVES in
/// ([`resource_region`]), never the region the request happens to be signed for: a
/// stream ARN nests under its cluster's (`cluster/{cluster}/stream/{stream}`), so
/// the only region a stream of a stored cluster could ever have carried is that
/// cluster's. The absent-cluster sibling ([`cluster_not_found`]) is the one case
/// that takes the request's region, because there is no stored resource to read one
/// from.
pub fn stream_not_found(
    cluster_region: &str,
    cluster_identifier: &str,
    stream_identifier: &str,
) -> ResourceNotFoundException {
    resource_not_found(
        &stream_arn(cluster_region, cluster_identifier, stream_identifier),
        stream_identifier,
        RESOURCE_TYPE_STREAM,
    )
}

/// The modelled `ResourceNotFoundException` for a cluster that has no
/// resource-based policy attached.
///
/// A policy is not an ARN-addressed resource of its own — the call addresses it
/// by the cluster's identifier — so this is the sentence AWS words that case
/// with, built from the identifier the request carried.
pub fn cluster_policy_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "Aurora DSQL can't find the policy for the resource {identifier}. \
             Verify that the policy exists."
        ),
        resource_id: identifier.to_string(),
        resource_type: RESOURCE_TYPE_POLICY.to_string(),
    }
}

/// The sentence every refusal of a call whose `expectedPolicyVersion` does not
/// name the version currently attached is worded with, BUILT FROM THE REQUEST: it
/// names the version the caller claimed and the resource it claimed it for, which
/// is how a caller learns which version was not found.
///
/// One sentence for the whole class, whatever status the refusal carries: a claim
/// about a policy that is not attached at all is a `ResourceNotFoundException`
/// ([`cluster_policy_version_not_found`]) and a claim about a version that is no
/// longer the current one is a `ConflictException`
/// ([`cluster_policy_version_conflict`]), so no handler words this case inline.
pub fn cluster_policy_version_message(identifier: &str, expected_version: &str) -> String {
    format!(
        "Aurora DSQL can't find policy version {expected_version} for resource {identifier}. \
         Verify that the policy exists and that the version is correct."
    )
}

/// The modelled `ResourceNotFoundException` for a call that named a policy
/// version the cluster does not hold (including the version of a policy that is
/// already gone).
pub fn cluster_policy_version_not_found(
    identifier: &str,
    expected_version: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: cluster_policy_version_message(identifier, expected_version),
        resource_id: identifier.to_string(),
        resource_type: RESOURCE_TYPE_POLICY.to_string(),
    }
}

/// The modelled `ConflictException` for a call whose `expectedPolicyVersion`
/// names a version other than the one currently attached: the optimistic-
/// concurrency refusal, worded with the same request-derived sentence as the
/// sibling 404 ([`cluster_policy_version_message`]) so a client reading the text
/// can tell a version conflict from any other conflict.
pub fn cluster_policy_version_conflict(
    identifier: &str,
    expected_version: &str,
) -> ConflictException {
    ConflictException {
        message: cluster_policy_version_message(identifier, expected_version),
        resource_id: Some(identifier.to_string()),
        resource_type: Some(RESOURCE_TYPE_POLICY.to_string()),
    }
}

