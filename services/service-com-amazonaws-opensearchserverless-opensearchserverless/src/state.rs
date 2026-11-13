// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-opensearchserverless-opensearchserverless service.
//!
//! This holds the resources this slice serves — the OpenSearch Serverless
//! **collection**, the **collection group**, the **security policy**, the **data access
//! policy**, the **lifecycle policy**, the **security configuration** and the managed
//! interface **VPC endpoint** — plus everything
//! more than one handler of
//! those resources needs: the server-derived fields (the minted ids, the ARNs, the
//! endpoints derived from a collection id, the epoch-millisecond timestamps), the
//! clock-owed `CREATING -> ACTIVE` transition a just-created collection settles through,
//! the opaque page token a listing hands out, and the `clientToken` replay index the
//! `@idempotent` creates read. Beside the resources sits the ACCOUNT's own settings — the
//! OCU capacity limits `GetAccountSettings` reports ([`AccountSettingsRecord`]).
//!
//! `src/ops/mod.rs` is generated, so a helper more than one operation reaches lives here
//! rather than in a second module under `src/ops/`.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AccessPolicyDetail, AccessPolicySummary, AccountSettingsDetail, CapacityDetails, CapacityLimits,
    CollectionDetail,
    CollectionErrorDetail, CollectionGroupCapacityLimits, CollectionGroupDetail,
    CollectionGroupErrorDetail, CollectionGroupStorageConfiguration, CollectionGroupSummary,
    CollectionSummary,
    CreateCollectionDetail, CreateCollectionGroupDetail, CreateVpcEndpointDetail, CurrentCapacity,
    DeleteCollectionDetail, DeleteVpcEndpointDetail, EffectiveLifecyclePolicyDetail, FipsEndpoints,
    IamFederationConfigOptions,
    IamIdentityCenterConfigOptions, LifecyclePolicyDetail, LifecyclePolicyErrorDetail,
    LifecyclePolicySummary, SamlConfigOptions, SecurityConfigDetail, SecurityConfigSummary, SecurityPolicyDetail,
    SecurityPolicySummary, Tag, UpdateCollectionDetail, UpdateCollectionGroupDetail,
    UpdateIamIdentityCenterConfigOptions, UpdateVpcEndpointDetail, VectorOptions, VpcEndpointDetail,
    VpcEndpointErrorDetail, VpcEndpointSummary,
};

/// Which address a collection's published endpoints name, so every one of them is an
/// address this emulator answers the OpenSearch REST surface on.
pub mod announced_endpoints;
/// The listener one collection's data plane is published on, which is the address a stock
/// OpenSearch client is configured with.
pub mod collection_plane;
/// Who may act on a collection's data: the `data` access policies, and the decision one of
/// them gives for a principal, a resource and a permission.
pub mod data_access;
/// The account's VPCs and subnets, which a managed VPC endpoint is placed in: facts this
/// service RESOLVES against Amazon EC2 rather than holds.
pub mod ec2_network;
/// Whether a collection's endpoints are reachable at all: the `network` security policies
/// that admit traffic to them.
pub mod network_access;

pub use announced_endpoints::AnnouncedEndpoints;
pub use collection_plane::CollectionPlane;

/// This service's ARN namespace, from the model's `aws.api#service` trait
/// (`smithy_query(query="service_info")` -> `awsService.arnNamespace`). Also its SigV4
/// signing name and the host label its collection endpoints carry.
pub const ARN_NAMESPACE: &str = "aoss";

/// The host label the FIPS endpoints carry, from the model's own endpoint rule set
/// (`smithy_query(query="service_info")` -> `endpointResolution.urlTemplates`, which lists
/// `https://aoss-fips.{Region}.{PartitionResult#dnsSuffix}` beside the standard one).
pub const FIPS_ARN_NAMESPACE: &str = "aoss-fips";

/// The DNS suffix an `aws`-partition endpoint carries.
pub const DNS_SUFFIX: &str = "amazonaws.com";

/// The path an OpenSearch Dashboards endpoint adds to the collection endpoint.
pub const DASHBOARDS_PATH: &str = "/_dashboards";

/// The region an ARN and an endpoint carry when the request is unsigned, so no SigV4
/// credential scope names one. `engine::emulator_arn`'s own doc fixes this default.
pub const DEFAULT_REGION: &str = "us-east-1";

// ---------------------------------------------------------------------------
// Modelled enum values this slice reports. The wire strings come from the model
// (`smithy_query(query="shape", shape="CollectionStatus")` and friends); the
// `*_VALUES` catalogs in `crate::ops::types` are what a handler validates a
// caller-supplied value against.
// ---------------------------------------------------------------------------

/// `CollectionStatus$CREATING` — "Creating collection resource".
pub const COLLECTION_CREATING: &str = "CREATING";
/// `CollectionStatus$ACTIVE` — "Collection resource is ready to use".
pub const COLLECTION_ACTIVE: &str = "ACTIVE";
/// `CollectionStatus$UPDATING` — "Updating collection resource".
pub const COLLECTION_UPDATING: &str = "UPDATING";
/// `CollectionStatus$DELETING` — "Deleting collection resource".
pub const COLLECTION_DELETING: &str = "DELETING";
/// `CollectionStatus$FAILED` — "Collection resource create or delete failed".
pub const COLLECTION_FAILED: &str = "FAILED";
/// `CollectionStatus$UPDATE_FAILED` — "Collection resource update failed".
pub const COLLECTION_UPDATE_FAILED: &str = "UPDATE_FAILED";

/// `DeletionProtection$ENABLED`: "When deletion protection is `ENABLED`, the collection
/// cannot be deleted" (the shape's own documentation).
pub const DELETION_PROTECTION_ENABLED: &str = "ENABLED";
/// `DeletionProtection$DISABLED`; the value a collection carries when the create request
/// names none, so a collection is deletable unless the caller asked otherwise.
pub const DELETION_PROTECTION_DISABLED: &str = "DISABLED";

/// `StandbyReplicas$ENABLED`, the default for a collection whose create request names
/// none: AWS's "Creating and managing collections" documents standby replicas as enabled
/// by default.
pub const STANDBY_REPLICAS_ENABLED: &str = "ENABLED";

/// The `CollectionType` a create request defaults to. From `CreateCollection`'s own
/// documentation of `type`, AWS's default collection type is `TIMESERIES`.
pub const DEFAULT_COLLECTION_TYPE: &str = "TIMESERIES";

/// The `kmsKeyArn` a collection reports when it is encrypted with an AWS-owned key
/// rather than a customer managed key — the case `EncryptionConfig$aWSOwnedKey` selects.
/// OpenSearch Serverless reports the literal `auto` there, because no customer key ARN
/// exists to name.
pub const AWS_OWNED_KEY: &str = "auto";

/// The statuses from which a collection may be deleted, from `ConflictException`'s own
/// documentation: "When deleting a resource, thrown when the resource is not in the
/// ACTIVE, FAILED, or UPDATE_FAILED state."
pub const DELETABLE_STATUSES: [&str; 3] =
    [COLLECTION_ACTIVE, COLLECTION_FAILED, COLLECTION_UPDATE_FAILED];

/// The statuses from which a collection may be UPDATED: the one it is "ready to use" in
/// and the one an earlier update left it in. A collection whose last update failed is
/// updatable again — that is how a caller retries the write that failed — while the
/// non-terminal statuses a collection is on its way through (`CREATING`, `UPDATING`,
/// `DELETING`) and the terminal `FAILED` of a create that never produced a collection are
/// not states an update may start from.
///
/// The check and the refusal that explains it are read off this one list
/// ([`collection_not_updatable_message`]), so they can never disagree.
pub const UPDATABLE_STATUSES: [&str; 2] = [COLLECTION_ACTIVE, COLLECTION_UPDATE_FAILED];

/// `VpcEndpointStatus$PENDING` — "Pending VPCEndpoint resource", the status a managed
/// interface endpoint is reported in while it is brought up.
pub const VPC_ENDPOINT_PENDING: &str = "PENDING";

/// `VpcEndpointStatus$ACTIVE` — "VPCEndpoint resource is ready to use", the status a
/// `PENDING` endpoint settles into.
pub const VPC_ENDPOINT_ACTIVE: &str = "ACTIVE";

/// `VpcEndpointStatus$DELETING` — "Deleting VPCEndpoint resource", the status an endpoint
/// whose deletion has been accepted is reported in until its PURGE falls due.
pub const VPC_ENDPOINT_DELETING: &str = "DELETING";

/// `VpcEndpointStatus$FAILED` — "VPCEndpoint resource create or delete failed". Nothing in
/// this slice fails an endpoint, so no stored record carries it; it is named here because
/// it is one of the statuses an endpoint may be DELETED from.
pub const VPC_ENDPOINT_FAILED: &str = "FAILED";

/// The statuses a managed interface endpoint may be deleted from: the states
/// `ConflictException`'s own documentation lists ("When deleting a resource, thrown when
/// the resource is not in the ACTIVE, FAILED, or UPDATE_FAILED state") that
/// `VpcEndpointStatus` actually models — it declares `PENDING`, `DELETING`, `ACTIVE` and
/// `FAILED`, and no `UPDATE_FAILED`.
pub const VPC_ENDPOINT_DELETABLE_STATUSES: [&str; 2] = [VPC_ENDPOINT_ACTIVE, VPC_ENDPOINT_FAILED];

/// The statuses a managed interface endpoint may be UPDATED from: the SETTLED ones, which
/// for `VpcEndpointStatus` are the same pair a deletion is accepted in
/// ([`VPC_ENDPOINT_DELETABLE_STATUSES`]). The other two it models, `PENDING` and
/// `DELETING`, are the non-terminal statuses an endpoint is on its way through — an
/// endpoint still being brought up, or already going away, is the state conflict
/// `ConflictException` describes.
pub const VPC_ENDPOINT_UPDATABLE_STATUSES: [&str; 2] = VPC_ENDPOINT_DELETABLE_STATUSES;

/// The most security groups an endpoint may carry, from `SecurityGroupIds`' own
/// `@length(min: 1, max: 5)` (`smithy_query(query="shape", shape="SecurityGroupIds")`).
///
/// The boundary validator holds ONE request's list to it; the stored list an update ADDS
/// to is held to it in the handler, so no sequence of accepted requests leaves an endpoint
/// carrying more groups than one request could ask for.
pub const MAX_SECURITY_GROUP_IDS: usize = 5;

/// The fewest subnets an endpoint may carry, from `SubnetIds`' own `@length(min: 1)`
/// (`smithy_query(query="shape", shape="SubnetIds")`): the endpoint is reached THROUGH its
/// subnets, and `subnetIds` is `@required` on the create, so an update that would remove
/// every one of them is refused.
pub const MIN_SUBNET_IDS: usize = 1;

/// The most subnets an endpoint may carry, and the most one request may name.
///
/// `SubnetIds` carries `@length(min: 1)` and NO maximum, so nothing in the model bounds the
/// list a request names or the list a succession of updates grows: this is what keeps both
/// finite, which is why it exists at all. The COUNT comes from how an interface endpoint
/// reaches a service: AWS PrivateLink's "Access an AWS service using an interface VPC
/// endpoint" has the caller select one subnet per Availability Zone, and the largest number
/// of Availability Zones an AWS Region has is six (`us-east-1`). It is therefore a bound
/// this emulator chooses, not one the model publishes, and a refusal against it is worded
/// as the validation layer words a length violation.
pub const MAX_SUBNET_IDS: usize = 6;

/// `ServerlessGeneration$CLASSIC`, the generation a collection group carries when its
/// create request does not name one.
///
/// `generation` is OPTIONAL on `CreateCollectionGroupRequest` while every detail shape
/// reports one, so a stored group always carries a generation. `ServerlessGeneration`
/// models exactly `CLASSIC` and `NEXTGEN`, and `NEXTGEN` is the newer generation a caller
/// opts into by naming it, so a request that names none describes a `CLASSIC` group. The
/// VALUE is a choice this emulator makes; the fact that one is always reported is the
/// model's.
pub const SERVERLESS_GENERATION_CLASSIC: &str = "CLASSIC";

/// `ServerlessGeneration$NEXTGEN`, the generation a caller opts into by naming it. It is
/// the generation that runs on the next-generation (v2) storage stack, so it is what
/// [`CollectionGroupRecord::storage_configuration`] reports `v2Enabled` from.
pub const SERVERLESS_GENERATION_NEXTGEN: &str = "NEXTGEN";

// The policy type values the per-type account-wide statistics counters are keyed by:
// `AccessPolicyStats$DataPolicyCount`, `SecurityPolicyStats$EncryptionPolicyCount` and
// `$NetworkPolicyCount`, and `LifecyclePolicyStats$RetentionPolicyCount` each count one
// type, so each names the type it counts rather than repeating the wire string.
// `SecurityConfigStats$SamlConfigCount` is keyed by no type — it is documented as "the
// number of security configurations in the current account", so it counts the whole class
// ([`ServiceState::security_config_count`]).

/// `AccessPolicyType$data` — the only value that enum carries
/// (`smithy_query(query="shape", shape="AccessPolicyType")`).
pub const ACCESS_POLICY_DATA: &str = "data";

/// `SecurityPolicyType$encryption` — "Encryption at rest policy type".
pub const SECURITY_POLICY_ENCRYPTION: &str = "encryption";

/// `SecurityPolicyType$network` — "Network policy type".
pub const SECURITY_POLICY_NETWORK: &str = "network";

// `LifecyclePolicyStats$RetentionPolicyCount`'s own type value is
// [`LIFECYCLE_POLICY_RETENTION`], declared with the retention-rule validation it keys.

/// `AutoscalingStatus$NO_ACTION`, the status a capacity report carries while no scaling
/// action is in progress — the other two members are `ACTION_SCALING_UP` and
/// `ACTION_SCALING_DOWN` (`smithy_query(query="shape", shape="AutoscalingStatus")`).
pub const AUTOSCALING_NO_ACTION: &str = "NO_ACTION";

/// The `errorCode` a `CollectionErrorDetail` carries for an id or name that names no
/// collection, from `CollectionErrorDetail$errorCode`'s own documentation: "The error
/// code for the request. For example, `NOT_FOUND`."
pub const ERROR_CODE_NOT_FOUND: &str = "NOT_FOUND";

/// The `errorMessage` that goes with [`ERROR_CODE_NOT_FOUND`], from
/// `CollectionErrorDetail$errorMessage`'s own documentation: "A description of the error.
/// For example, `The specified Collection is not found.`"
pub const ERROR_MESSAGE_NOT_FOUND: &str = "The specified Collection is not found.";

/// The `errorMessage` a `VpcEndpointErrorDetail` carries for an id that names no managed
/// interface endpoint. `VpcEndpointErrorDetail$errorMessage` is documented only as "An
/// error message describing the reason for the failure", so the wording follows the one
/// this service's other batch readers document for the same cause
/// ([`ERROR_MESSAGE_NOT_FOUND`], [`ERROR_MESSAGE_GROUP_NOT_FOUND`]) with the resource this
/// one reads named instead — spelled `VpcEndpoint`, ONE word, the way this service names
/// the resource kind in every refusal about it ([`vpc_endpoint_not_found_message`],
/// [`vpc_endpoint_absent_conflict_message`]) and in the model's own shape names
/// (`VpcEndpointDetail`, `VpcEndpointErrorDetail`).
pub const ERROR_MESSAGE_VPC_ENDPOINT_NOT_FOUND: &str = "The specified VpcEndpoint is not found.";

/// The `errorMessage` a `LifecyclePolicyErrorDetail` carries for an identifier that names
/// no lifecycle policy, from `LifecyclePolicyErrorDetail$errorMessage`'s own
/// documentation: "A description of the error. For example, `The specified Lifecycle Policy
/// is not found`."
pub const ERROR_MESSAGE_LIFECYCLE_POLICY_NOT_FOUND: &str =
    "The specified Lifecycle Policy is not found.";

/// How long after the call that starts it a collection's clock-owed transition falls due:
/// `CREATING -> ACTIVE` after a create, `UPDATING -> ACTIVE` after an update, and the
/// PURGE of a `DELETING` collection after a delete.
///
/// A span this emulator chooses, not a duration the service documents: a collection is
/// returned in a non-terminal status, so it settles on its own a moment later with nobody
/// calling. It only has to be long enough that a caller acting straight after the call
/// still observes the transient status and the state errors that go with it, and short
/// enough to poll through.
const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The account's collection quota in one Region, from AWS's "Quotas for Amazon OpenSearch
/// Serverless": 50 collections per account (adjustable on request, so this is the DEFAULT
/// quota). It bounds the store as well as the API.
pub const MAX_COLLECTIONS: usize = 50;

/// The account's quota for security policies of ONE type in one Region, from the same
/// quota page: 20 encryption policies and 20 network policies per account.
pub const MAX_SECURITY_POLICIES_PER_TYPE: usize = 20;

/// The most tags one resource carries, from `Tags`' own `@length(min: 0, max: 50)` — the
/// bound `TagResource` is refused past, since that write can take a resource over a
/// maximum no single request's `tags` list breaks.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most indexes this emulator stores in one collection.
///
/// Emulator resource guard, NOT a quota AWS publishes: the OpenSearch Serverless quota
/// page lists no per-collection index limit, and `CreateIndex` adds one caller-named entry
/// per request to a collection that may live forever, so this bound is what keeps that
/// store finite. It is answered with the modelled `ServiceQuotaExceededException`'s
/// stand-in for this operation — `CreateIndex` declares no quota error, so the refusal is
/// its `ValidationException` — and it is set far above any realistic client workflow.
pub const MAX_INDEXES_PER_COLLECTION: usize = 100;

/// The account's quota for security configurations of ONE type in one Region: 50
/// (adjustable on request, so this is the DEFAULT quota), which is what the real service's
/// own refusal names — "Your request exceeds the limit for saml policies in this AWS
/// account. The limit is 50." — and the same order as the collection quota above, not the
/// 20 the encryption/network policy rows carry. Counted per `SecurityConfigType`, as the
/// policy quota above is counted per `SecurityPolicyType`.
pub const MAX_SECURITY_CONFIGS_PER_TYPE: usize = 50;

/// The `sessionTimeout` a SAML security configuration carries when the create request
/// names none, from `SamlConfigOptions$sessionTimeout`'s own documentation: "The session
/// timeout, in minutes. Default is 60 minutes".
pub const DEFAULT_SAML_SESSION_TIMEOUT: i32 = 60;

/// The `userAttribute` an IAM Identity Center configuration carries when the create
/// request names none, from `CreateIamIdentityCenterConfigOptions$userAttribute`'s own
/// documentation: "Defaults to `UserId`".
pub const DEFAULT_IDENTITY_CENTER_USER_ATTRIBUTE: &str = "UserId";

/// The `groupAttribute` an IAM Identity Center configuration carries when the create
/// request names none, from `CreateIamIdentityCenterConfigOptions$groupAttribute`'s own
/// documentation: "Defaults to `GroupId`".
pub const DEFAULT_IDENTITY_CENTER_GROUP_ATTRIBUTE: &str = "GroupId";

/// The account's data-access-policy quota in one Region, from the same "Quotas for Amazon
/// OpenSearch Serverless" page: 100 data access policies per account (adjustable on
/// request, so this is the DEFAULT quota). `AccessPolicyType` has the single value `data`,
/// so this bounds the whole map.
pub const MAX_ACCESS_POLICIES: usize = 100;

/// The most lifecycle policies this emulator stores for one account in one Region.
///
/// Emulator resource guard, NOT a quota AWS publishes: the "Quotas for Amazon OpenSearch
/// Serverless" page lists rows for collections, security policies and data access policies
/// but none for data lifecycle policies, so this bound exists only to keep the store
/// finite. It is answered with the modelled `ServiceQuotaExceededException` because that is
/// the error `CreateLifecyclePolicy` declares for "more resources than the service allows",
/// and it is set at the data-access-policy figure above, the other per-account policy cap
/// this service carries. `LifecyclePolicyType` has the single value `retention`, so this
/// bounds the whole map.
pub const MAX_LIFECYCLE_POLICIES: usize = MAX_ACCESS_POLICIES;

/// The most collection groups this emulator stores for one account in one Region.
///
/// Emulator resource guard, NOT a quota AWS publishes: the OpenSearch Serverless quota
/// page lists no collection-group limit, so this bound exists only to keep the store
/// finite. It is answered with the modelled `ServiceQuotaExceededException` because that
/// is the error `CreateCollectionGroup` declares for "more resources than the service
/// allows", and it is set at the collection quota above so a group per collection still
/// fits.
pub const MAX_COLLECTION_GROUPS: usize = MAX_COLLECTIONS;

/// The most managed interface VPC endpoints this emulator stores for one account in one
/// Region.
///
/// An emulator bound rather than a figure this code can cite from the model: nothing in
/// `CreateVpcEndpoint`'s contract fixes a count, so this exists to keep the store finite.
/// It is answered with the modelled `ServiceQuotaExceededException` because that is the
/// error `CreateVpcEndpoint` declares for "more resources than the service allows", and it
/// is set at the collection quota above, so an endpoint per collection still fits.
pub const MAX_VPC_ENDPOINTS: usize = MAX_COLLECTIONS;

/// The `maxIndexingCapacityInOCU` an account reports before anything calls
/// `UpdateAccountSettings`, from AWS's "Managing capacity limits for Amazon OpenSearch
/// Serverless" — the page `CapacityLimits`' own documentation links — which gives the
/// default maximum capacity as 10 OCUs for indexing and 10 OCUs for search.
pub const DEFAULT_MAX_INDEXING_CAPACITY_IN_OCU: i32 = 10;

/// The `maxSearchCapacityInOCU` default; see [`DEFAULT_MAX_INDEXING_CAPACITY_IN_OCU`].
pub const DEFAULT_MAX_SEARCH_CAPACITY_IN_OCU: i32 = 10;

/// The account's non-adjustable maximum capacity, in OCUs, for ONE capacity type.
///
/// `CapacityLimits`' members carry only a `@range(min: 2)`, so the model states no ceiling;
/// the live service refuses a request above this figure with its own quota message ("your
/// maximum indexing capacity service quota limit of 1700 OCUs"), which is where the value
/// comes from. The quota is stated PER capacity type — indexing and search are counted and
/// refused separately — and neither the model nor the refusal distinguishes the two
/// figures, so the same ceiling bounds both.
pub const MAX_ACCOUNT_CAPACITY_IN_OCU: i32 = 1700;

/// The `CollectionGroupCapacityLimits$maxIndexingCapacityInOCU` a COLLECTION GROUP
/// carries when its create request named no indexing ceiling of its own.
///
/// A group-level ceiling, distinct from the ACCOUNT's
/// [`DEFAULT_MAX_INDEXING_CAPACITY_IN_OCU`] above: the model declares no default for
/// either `CollectionGroupCapacityLimits` member, and the wire recordings this emulator
/// replays show the live API answering a create that named no `capacityLimits` with a
/// defaulted ceiling of 96 OCUs for indexing and 96 for search. The VALUE comes from that
/// recorded default; what the model fixes is that the member is reported at all, since
/// every collection-group detail shape carries `capacityLimits`.
pub const DEFAULT_COLLECTION_GROUP_MAX_INDEXING_CAPACITY_IN_OCU: f32 = 96.0;

/// The group-level `maxSearchCapacityInOCU` default; see
/// [`DEFAULT_COLLECTION_GROUP_MAX_INDEXING_CAPACITY_IN_OCU`].
pub const DEFAULT_COLLECTION_GROUP_MAX_SEARCH_CAPACITY_IN_OCU: f32 = 96.0;

/// The page bound a listing applies when the caller names none, from each listing's own
/// `maxResults` documentation: "The maximum number of results to return. Default is 20."
/// (`ListCollectionsRequest`), "The default is 20." (`ListSecurityConfigsRequest`).
pub const DEFAULT_PAGE_SIZE: i32 = 20;

/// The page bound the LIFECYCLE-POLICY listing applies when the caller names none, which
/// its own `maxResults` documentation puts lower than the listings above:
/// `ListLifecyclePoliciesRequest$maxResults` reads "An optional parameter that specifies
/// the maximum number of results to return. You can use use `nextToken` to get the next
/// page of results. The default is 10."
pub const DEFAULT_LIFECYCLE_POLICY_PAGE_SIZE: i32 = 10;

/// The largest page a listing serves: the maximum of `maxResults`' own
/// `@range(min: 1, max: 100)`.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The most `resource` filter entries a listing evaluates: the modelled maximum of
/// `ListSecurityPoliciesRequest$resource`'s own `@length(min: 1, max: 1000)`. Enforced
/// HERE as well as at the boundary, so the work a filter drives is bounded by this code
/// rather than by what the validator ahead of it happens to check.
pub const MAX_RESOURCE_FILTER_ENTRIES: usize = 1000;

/// The longest resource pattern a filter entry, or a stored policy's own `Resource`
/// entry, may be, in BYTES.
///
/// EMULATOR RESOURCE GUARD, not an AWS limit: `Resource` carries no `@length`
/// (`smithy_query(query="shape", shape="Resource")`), so without this the length of a
/// pattern — a factor in the cost of every comparison below — is the caller's to choose.
/// 1024 is far past any real resource name: the longest form is
/// `index/<collection>/<index>`, whose collection half is a `CollectionName` of at most 32
/// characters.
pub const MAX_RESOURCE_PATTERN_LENGTH: usize = 1024;

/// The most `Resource` entries one stored policy document contributes to a listing's
/// filtering.
///
/// EMULATOR RESOURCE GUARD, not an AWS limit: a `PolicyDocument` is up to 20480
/// characters, which is room for thousands of short `Resource` entries, and that count
/// multiplies the comparisons a filter drives. A real encryption or network policy names a
/// handful, so a document past this contributes its first [`MAX_POLICY_RESOURCES`] entries
/// and no more.
pub const MAX_POLICY_RESOURCES: usize = 200;

/// The most resource-pattern matching WORK one listing request performs, in bytes
/// scanned.
///
/// EMULATOR RESOURCE GUARD, not an AWS limit, and the one that bounds the PRODUCT the
/// factors above each bound a side of: records of a type, times filter entries, times the
/// resources a document names, times the cost of one comparison. The budget counts WORK
/// rather than comparisons, because one comparison of two wildcard-heavy patterns is not
/// one of two plain names: see [`resource_overlap_work`]. 100 million byte-scans is around
/// a tenth of a second, and far past any real listing (a thousand filter entries against
/// twenty policies naming a handful of short resources each is under ten million); a
/// request that would exceed it is refused with the modelled `ValidationException` instead
/// of running on, in the spirit of [`MAX_DOCUMENT_DEPTH`].
pub const MAX_RESOURCE_FILTER_WORK: usize = 100_000_000;

/// The most `clientToken` replay entries this emulator keeps.
///
/// Emulator resource guard, NOT an AWS quota: `ClientToken` is caller-supplied and up to
/// 512 characters, and a create/delete loop frees its resource slot while leaving one
/// token behind, so the resource caps above do not bound this map. It is held at this cap
/// by oldest-first eviction in
/// [`ComAmazonawsOpensearchserverlessOpensearchserverlessState::remember_token`], and an entry is dropped as soon as the record it
/// names is deleted. Evicting the oldest token only costs a retry of a create the caller
/// made long ago that very-much-later answer, which then conflicts on the name instead of
/// replaying — the same answer a token whose record was deleted already gets. Set well
/// above the combined resource caps so a realistic retry is always still replayable.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 1_024;

/// The number of characters a minted `CollectionId` carries.
///
/// `CollectionId` is `@length(min: 3, max: 40)` with `@pattern("^[a-z0-9]{3,40}$")`, a bound
/// far wider than the ONE width AWS mints inside it, so the modelled pattern does not settle
/// this: what does is the RECORDED response of a collection create, whose id is 20 lowercase
/// alphanumeric characters — the width that is also the host label of the collection endpoint
/// derived from it (`https://<id>.<region>.aoss.amazonaws.com`). A client, a provider or a
/// test that checks the form of an id (or of an endpoint built from one) against AWS's own
/// takes this width and no other.
///
/// It sits inside the modelled bound and keeps `36^20` inside a `u128`, so a single draw
/// renders a whole id.
const ID_LEN: u32 = 20;

/// The characters a `CollectionId` is built from: exactly the pattern's class.
const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The radix [`render_id`] renders a draw in — the size of [`ID_ALPHABET`].
const ID_BASE: u128 = ID_ALPHABET.len() as u128;

/// The number of distinct ids [`ComAmazonawsOpensearchserverlessOpensearchserverlessState::mint_collection_id`] can render: `36^20`.
const ID_SPACE: u128 = ID_BASE.pow(ID_LEN);

/// How many draws one mint takes before it keeps the last one.
///
/// A fresh draw lands on an id already stored only by chance out of [`ID_SPACE`], so the
/// first draw is almost always the answer; re-drawing on a collision is what keeps the ids
/// of two live resources distinct, and the bound keeps a mint off an unbounded loop.
const ID_MINT_ATTEMPTS: usize = 8;

/// The number of characters one minted `PolicyVersion` carries.
///
/// `PolicyVersion` is `@length(min: 20, max: 36)` with
/// `@pattern("^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$")`, a
/// base64-SHAPED token whose bound admits five widths (20, 24, 28, 32 and 36 characters), so
/// the model does not settle this either: what does is the RECORDED response of a policy
/// read, whose version token is 20 characters — the shortest the bound admits, a whole number
/// of four-character groups, so unpadded.
const POLICY_VERSION_LEN: usize = 20;

/// The characters one minted `PolicyVersion` is drawn from: the ALPHANUMERIC half of the
/// class the pattern admits, which is the class AWS's own version tokens are drawn from.
///
/// The pattern also admits `+` and `/`, and an encoder using the standard base64 alphabet
/// puts them in a token: a policy version is an opaque identifier a caller round-trips into
/// `Update*Policy` / `UpdateSecurityConfig`, logs, stores in configuration and places in
/// paths and URLs, so a token holding a path delimiter breaks assumptions no AWS-issued one
/// breaks.
const POLICY_VERSION_ALPHABET: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// The prefix a `VpcEndpointId` carries, from the shape's own
/// `@pattern("^vpce-[0-9a-z]*$")`.
const VPC_ENDPOINT_ID_PREFIX: &str = "vpce-";

/// The number of hexadecimal characters a minted `VpcEndpointId` carries after
/// [`VPC_ENDPOINT_ID_PREFIX`]: 17, the width an interface VPC endpoint id carries in AWS
/// (`vpce-` followed by a 17-character hexadecimal identifier). It sits inside
/// `VpcEndpointId`'s `@length(min: 1, max: 255)`, and hexadecimal is inside the
/// `[0-9a-z]` class its pattern admits.
const VPC_ENDPOINT_ID_HEX_LEN: usize = 17;

/// The `errorMessage` a `CollectionGroupErrorDetail` carries for an id or name that names
/// no collection group, from `CollectionGroupErrorDetail$errorMessage`'s own
/// documentation: "A description of the error. For example, `The specified Collection
/// Group is not found.`"
pub const ERROR_MESSAGE_GROUP_NOT_FOUND: &str = "The specified Collection Group is not found.";

/// `arn:aws:aoss:<region>:<account>:collection/<id>`.
pub fn collection_arn(region: &str, collection_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("collection/{collection_id}"),
    )
}

/// `arn:aws:aoss:<region>:<account>:collection-group/<id>`.
///
/// The resource half is the `CollectionGroup` resource's own ARN template from the model
/// (`smithy_query(query="shape", shape="CollectionGroup")` -> `arn.template`:
/// `collection-group/{id}`).
pub fn collection_group_arn(region: &str, collection_group_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("collection-group/{collection_group_id}"),
    )
}

/// The collection-specific endpoint index, search and data-upload requests go to as AWS
/// publishes it: `https://<id>.<region>.aoss.amazonaws.com`. Derived from the collection id
/// alone, so every reader of a record reports the same endpoint.
///
/// What this emulator ANNOUNCES for a collection is
/// [`AnnouncedEndpoints::of_collection`], which answers this form only for a caller that
/// resolves AWS's own hosts on an emulator served on no port at all — every other caller is
/// announced an address this emulator answers on.
pub fn collection_endpoint(collection_id: &str, region: &str) -> String {
    format!("https://{collection_id}.{region}.{ARN_NAMESPACE}.{DNS_SUFFIX}")
}

/// The OpenSearch Dashboards endpoint: [`collection_endpoint`] plus
/// [`DASHBOARDS_PATH`].
pub fn dashboard_endpoint(collection_id: &str, region: &str) -> String {
    format!("{}{DASHBOARDS_PATH}", collection_endpoint(collection_id, region))
}

/// The FIPS-compliant collection endpoint: [`collection_endpoint`] with the
/// [`FIPS_ARN_NAMESPACE`] host label the model's FIPS endpoint template uses.
pub fn fips_collection_endpoint(collection_id: &str, region: &str) -> String {
    format!("https://{collection_id}.{region}.{FIPS_ARN_NAMESPACE}.{DNS_SUFFIX}")
}

/// The FIPS-compliant Dashboards endpoint; see [`fips_collection_endpoint`].
pub fn fips_dashboard_endpoint(collection_id: &str, region: &str) -> String {
    format!(
        "{}{DASHBOARDS_PATH}",
        fips_collection_endpoint(collection_id, region)
    )
}

/// The HOST of a published endpoint — the value a client puts in the `Host` header of the
/// request it signs, which is what a request addressed to a collection's data plane is
/// resolved by.
///
/// A published endpoint is a URL (`https://<host>[/<path>]`), and the host of one is the
/// authority `service_common::data_plane_addressing::addressed_host` reads — the ONE
/// reading of that in this repository, called rather than re-implemented. Any `:port`
/// stays part of the host, since that is what the client addressed.
pub fn endpoint_host(endpoint: &str) -> String {
    ::service_common::data_plane_addressing::addressed_host(endpoint, None).unwrap_or_default()
}

/// Whether `endpoint` addresses a WHOLE host — a URL that is an authority and nothing more
/// (`https://<host>[:<port>]`, with or without a trailing `/`), so what it names is reached
/// by its address alone.
///
/// A published endpoint carrying a path beyond that names its resource IN the path, on an
/// authority it shares with whatever else answers there; the two are told apart here rather
/// than by each reader.
pub fn addresses_a_whole_host(endpoint: &str) -> bool {
    let after_scheme = endpoint
        .split_once("://")
        .map_or(endpoint, |(_scheme, rest)| rest);
    matches!(after_scheme.split_once('/'), None | Some((_, "")))
}

/// A fresh document id, for a write that named none — the id OpenSearch mints for a
/// `POST /<index>/_doc` and for a `_bulk` action that carries no `_id`.
///
/// Drawn from [`random_bytes`], exactly as the identifiers above are, so the id carries
/// no structure a caller can read and no two writes take the same one. Rendered in the
/// URL-safe base64 alphabet OpenSearch's own auto-generated ids are drawn from, so it is
/// a value that travels in a URI path unescaped.
pub fn mint_document_id() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    /// The length of an OpenSearch auto-generated document id.
    const ID_CHARS: usize = 20;
    random_bytes()
        .iter()
        .take(ID_CHARS)
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect()
}

/// Now, as the epoch milliseconds `createdDate` / `lastModifiedDate` are modelled with
/// (`Long`, documented as "The Epoch time when the collection was created").
pub fn now_epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// 32 fresh bytes behind ONE minted identifier or version token.
///
/// The seed is what the standard library offers without reaching for another crate:
/// `RandomState`, which std seeds randomly once per process and varies per instance, plus
/// the clock and a per-process mint counter, so two values minted in the same instant — or
/// by two engines in the same process — still differ. SHA-256 spreads the seed so none of
/// its structure shows in the value. A `CollectionId` and a `PolicyVersion` are
/// identifiers, not secrets: this is a uniqueness and opacity source, and nothing drawn
/// here is used as a credential.
fn random_bytes() -> [u8; 32] {
    use ::sha2::Digest as _;
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher as _, Hasher as _};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// How many values this process has minted, so the seed never repeats within it.
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let mut digest = ::sha2::Sha256::new();
    for _ in 0..2 {
        digest.update(RandomState::new().build_hasher().finish().to_le_bytes());
    }
    digest.update(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    digest.update(MINTED.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    digest.finalize().into()
}

/// One draw out of [`ID_SPACE`], the value [`render_id`] renders an id from.
fn draw_id_value() -> u128 {
    let bytes = random_bytes();
    let mut draw = [0_u8; 16];
    draw.copy_from_slice(&bytes[..16]);
    u128::from_be_bytes(draw) % ID_SPACE
}

/// Render `draw` as [`ID_LEN`] characters of [`ID_ALPHABET`] — the `^[a-z0-9]{3,40}$`
/// shape a `CollectionId` and a `CollectionGroupId` both carry.
///
/// Every position comes out of the draw, so each one ranges over the whole alphabet
/// instead of filling with whatever character the radix conversion ran out on.
fn render_id(draw: u128) -> String {
    let mut remaining = draw % ID_SPACE;
    let mut id = Vec::with_capacity(ID_LEN as usize);
    for _ in 0..ID_LEN {
        let digit = usize::try_from(remaining % ID_BASE).unwrap_or(0);
        id.push(ID_ALPHABET[digit]);
        remaining /= ID_BASE;
    }
    String::from_utf8(id).unwrap_or_default()
}

/// The number of byte values one character of [`POLICY_VERSION_ALPHABET`] is drawn from: the
/// largest whole multiple of the alphabet's size that fits in a byte.
///
/// A byte at or above it is DISCARDED rather than folded into the alphabet, because 256 is not
/// a multiple of 62: folding every byte would make the first eight characters of the alphabet
/// likelier than the rest, which is structure a caller could read out of enough tokens.
const POLICY_VERSION_DRAW_CEILING: usize = 256 - 256 % POLICY_VERSION_ALPHABET.len();

/// Mint a fresh `PolicyVersion`: [`POLICY_VERSION_LEN`] characters drawn uniformly from
/// [`POLICY_VERSION_ALPHABET`], one per drawn byte below
/// [`POLICY_VERSION_DRAW_CEILING`].
///
/// The token is drawn rather than derived, so it is opaque — it carries neither the write
/// instant nor a counter a caller could read a write order out of — and a write never
/// mints the version an earlier write did, which is what makes it usable as the
/// optimistic-concurrency precondition `Update*Policy` and `UpdateSecurityConfig` check.
///
/// The loop draws another 32 bytes only while the token is short of its width, which a draw
/// discarding under 4% of its bytes reaches with the first batch except by accident.
pub fn mint_policy_version() -> String {
    let mut token = String::with_capacity(POLICY_VERSION_LEN);
    while token.len() < POLICY_VERSION_LEN {
        for byte in random_bytes() {
            if token.len() == POLICY_VERSION_LEN {
                break;
            }
            let draw = usize::from(byte);
            if draw >= POLICY_VERSION_DRAW_CEILING {
                continue;
            }
            token.push(char::from(
                POLICY_VERSION_ALPHABET[draw % POLICY_VERSION_ALPHABET.len()],
            ));
        }
    }
    token
}

/// The hexadecimal digits a drawn `VpcEndpointId` is built from.
const HEX_DIGITS: &[u8] = b"0123456789abcdef";

/// Draw a `VpcEndpointId`: [`VPC_ENDPOINT_ID_PREFIX`] followed by
/// [`VPC_ENDPOINT_ID_HEX_LEN`] hexadecimal characters.
///
/// Every character after the prefix comes out of one drawn byte (see [`random_bytes`]), so
/// the id carries no structure a caller can read — no counter and no clock — and two
/// endpoints, in this store or in another process, never carry the same one.
fn draw_vpc_endpoint_id() -> String {
    let mut id = String::with_capacity(VPC_ENDPOINT_ID_PREFIX.len() + VPC_ENDPOINT_ID_HEX_LEN);
    id.push_str(VPC_ENDPOINT_ID_PREFIX);
    for byte in random_bytes().iter().take(VPC_ENDPOINT_ID_HEX_LEN) {
        id.push(char::from(HEX_DIGITS[usize::from(byte % 16)]));
    }
    id
}

/// The region this request is signed for, which is the region its collections' ARNs and
/// endpoints carry. Falls back to [`DEFAULT_REGION`] for an unsigned request, which
/// carries no credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A status change the clock owes a resource.
///
/// `CollectionStatus` models `CREATING` beside `ACTIVE`, so a collection is reported in
/// it while the resources it describes are brought up, and reaches `ACTIVE` on its own a
/// moment later with nobody calling. `VpcEndpointStatus` models `PENDING` beside `ACTIVE`
/// for the same span of a managed interface endpoint. The due instant is STORED and
/// applied when a later call observes the record; no handler ever sleeps for it.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    /// The instant from which [`status`](Self::status) is the collection's real status.
    pub due_at: Instant,
    /// The status the collection settles into.
    pub status: &'static str,
}

/// One index stored inside a collection, keyed by its `IndexName`.
///
/// An index belongs to exactly one collection (`CreateIndex` addresses it by the
/// collection's `id` plus the index name), so it is held ON the collection record: a
/// purged collection takes its indexes with it and no second map can outlive it.
///
/// The control plane reports the schema (`GetIndex` answers `indexSchema`) and the
/// collection's own data plane reports the documents, so ONE record carries both: an index
/// created through either plane is the index the other one reads.
#[derive(Debug, Clone, Default)]
pub struct IndexRecord {
    /// `IndexSchema`, held as the document the create carried — `GetIndex` reports it
    /// back. `None` for an index created without one, which the model allows
    /// (`indexSchema` is not `@required`).
    pub index_schema: Option<::aws_smithy_types::Document>,
    /// The epoch-millisecond instant the index was created, which the data plane reports
    /// as the index setting `index.creation_date`.
    pub created_date: i64,
    /// The documents this index holds, by document id — what the collection's OpenSearch
    /// REST data plane writes, reads and searches. Bounded by
    /// [`MAX_DOCUMENTS_PER_INDEX`].
    pub documents: BTreeMap<String, DocumentRecord>,
    /// The sequence number the next write to this index takes, which is how OpenSearch
    /// numbers the writes of one index (`_seq_no`). Monotonic per index and never reused,
    /// so a client can order two writes it made.
    pub next_seq_no: i64,
}

impl IndexRecord {
    /// A fresh index carrying `index_schema`, created at `created_date` and holding no
    /// document yet — the one place this crate builds the record, so an index created
    /// through the control plane and one created through the data plane start alike.
    pub fn new(index_schema: Option<::aws_smithy_types::Document>, created_date: i64) -> Self {
        Self {
            index_schema,
            created_date,
            documents: BTreeMap::new(),
            next_seq_no: 0,
        }
    }

    /// The sequence number the next write of this index takes, advancing the counter.
    ///
    /// A DELETE is a write of the id it addresses, so it takes a sequence number too —
    /// which is what lets a client order a delete against the write before it.
    pub fn take_seq_no(&mut self) -> i64 {
        let seq_no = self.next_seq_no;
        self.next_seq_no = self.next_seq_no.saturating_add(1);
        seq_no
    }

    /// Store `source` under `id`, as one write of this index: the version the id is now at
    /// and the sequence number this write took, which is what the answer reports.
    ///
    /// A write to an id the index already holds replaces that document and advances its
    /// version, the way OpenSearch versions a document per id; the sequence number comes
    /// from the index and advances on every write.
    pub fn write_document(
        &mut self,
        id: &str,
        source: ::aws_smithy_types::Document,
    ) -> DocumentRecord {
        let version = self
            .documents
            .get(id)
            .map_or(1, |held| held.version.saturating_add(1));
        let seq_no = self.take_seq_no();
        let written = DocumentRecord {
            source,
            version,
            seq_no,
        };
        self.documents.insert(id.to_string(), written.clone());
        written
    }
}

/// The most documents ONE index of a collection holds.
///
/// EMULATOR RESOURCE GUARD, not an AWS limit: the data plane adds one caller-supplied
/// document per write to an index that may live as long as its collection, and nothing
/// reclaims a document until its index or collection is removed — so this cap, with
/// [`MAX_DOCUMENTS_PER_INDEX`]'s companion [`MAX_DOCUMENT_SOURCE_NODES`] on the size of
/// one document and [`MAX_INDEXES_PER_COLLECTION`] on how many indexes a collection
/// holds, is what keeps the store finite. A write past it is refused rather than stored.
pub const MAX_DOCUMENTS_PER_INDEX: usize = 1_000;

/// The most values ONE stored document source may carry; see [`MAX_DOCUMENTS_PER_INDEX`]
/// for why the size of one document is bounded, and [`document_within_bounds`] for the
/// walk that enforces it.
pub const MAX_DOCUMENT_SOURCE_NODES: usize = 1_000;

/// One document an index holds, as its data plane stores and reports it.
#[derive(Debug, Clone)]
pub struct DocumentRecord {
    /// The document body the write carried, held PARSED so a read answers the document
    /// rather than the caller's own formatting.
    pub source: ::aws_smithy_types::Document,
    /// `_version`: how many times this id has been written, starting at 1.
    pub version: i64,
    /// `_seq_no`: this write's place in the index's own write order.
    pub seq_no: i64,
}

/// One stored collection. Every field a reader reports is held here; nothing is
/// recomputed from a request.
#[derive(Debug, Clone)]
pub struct CollectionRecord {
    /// The server-minted `CollectionId`, which is also this record's key.
    pub id: String,
    pub name: String,
    /// The `CollectionStatus` wire value, settled through [`PendingStatus`].
    pub status: String,
    /// `CollectionType`; named `kind` because `type` is a Rust keyword.
    pub kind: String,
    pub description: Option<String>,
    pub arn: String,
    pub kms_key_arn: String,
    pub standby_replicas: String,
    pub deletion_protection: String,
    pub vector_options: Option<VectorOptions>,
    pub collection_group_name: Option<String>,
    pub collection_endpoint: String,
    pub dashboard_endpoint: String,
    pub fips_collection_endpoint: String,
    pub fips_dashboard_endpoint: String,
    pub created_date: i64,
    pub last_modified_date: i64,
    /// The tags the create request carried. `CollectionDetail` models no `tags` member,
    /// so they are reached through the tagging operations (still stubs) rather than
    /// echoed on a read.
    pub tags: Vec<Tag>,
    /// The indexes this collection holds, by `IndexName`.
    pub indexes: BTreeMap<String, IndexRecord>,
    /// The transition the clock owes this collection, while one is outstanding.
    pub pending_status: Option<PendingStatus>,
    /// When this collection's accepted deletion is due to be PURGED — the instant from
    /// which it stops being a collection any read reports at all. Set by
    /// [`Self::begin_deletion`], which is also what moves it to `DELETING`.
    pub purge_at: Option<Instant>,
}

impl CollectionRecord {
    /// Whether this collection publishes `host` as an address whose WHOLE authority is
    /// this collection: its collection endpoint "used to submit index, search, and data
    /// upload requests" (`CollectionDetail$collectionEndpoint`) or the FIPS endpoint beside
    /// it, where the address names the collection and the path is the client's — which is
    /// also the host of each Dashboards endpoint, since a Dashboards endpoint is its
    /// collection endpoint plus a path.
    ///
    /// Read by the front door's host-style addressing
    /// ([`crate::service::collection_rest`]), so only an address of that shape is compared:
    /// a published endpoint that names the collection in its PATH
    /// ([`announced_endpoints`]) is resolved by that path, because its authority is the
    /// emulator's own front door, which every service of this process shares.
    pub fn publishes_host(&self, host: &str) -> bool {
        [&self.collection_endpoint, &self.fips_collection_endpoint]
            .into_iter()
            .filter(|published| addresses_a_whole_host(published))
            .any(|published| endpoint_host(published) == host)
    }

    /// Report `transient` from now, and owe `settled` from [`SETTLE_AFTER`] onwards.
    pub fn begin_transition(&mut self, transient: &str, settled: &'static str) {
        self.status = transient.to_string();
        self.pending_status = Some(PendingStatus {
            due_at: Instant::now() + SETTLE_AFTER,
            status: settled,
        });
    }

    /// Materialise the transition the clock has already made due, if any.
    ///
    /// This is the one write a reporting operation may make: the change was owed at `now`
    /// whether or not anybody called, so applying it does not change what a later caller
    /// sees — it *is* what a later caller sees.
    pub fn settle(&mut self, now: Instant) {
        if let Some(pending) = self.pending_status.take_if(|pending| now >= pending.due_at) {
            self.status = pending.status.to_string();
        }
    }

    /// Accept this collection's deletion: report it `DELETING` from now, and owe its PURGE
    /// from [`SETTLE_AFTER`] onwards.
    ///
    /// `DeleteCollection` answers `deleteCollectionDetail` with status `DELETING`, and the
    /// collection stays readable in that status until the purge, so a caller polling the
    /// deletion observes it rather than seeing the resource vanish in one step.
    pub fn begin_deletion(&mut self, deleted_at: i64) {
        self.status = COLLECTION_DELETING.to_string();
        // A delete supersedes whatever transition was outstanding: the only thing still
        // owed on this collection is the purge.
        self.pending_status = None;
        self.purge_at = Some(Instant::now() + SETTLE_AFTER);
        self.last_modified_date = deleted_at;
    }

    /// Whether this collection's deletion has been accepted, so the only thing still owed
    /// on it is the purge.
    ///
    /// Read off `purge_at`, the ONE record of an accepted deletion:
    /// [`Self::begin_deletion`] is what sets it, and it sets the `DELETING` status with it.
    pub fn is_deleting(&self) -> bool {
        self.purge_at.is_some()
    }

    /// Whether the clock has already made this collection's purge due at `now`.
    pub fn is_purge_due(&self, now: Instant) -> bool {
        self.purge_at.is_some_and(|due_at| now >= due_at)
    }

    /// Accept an update that only rewrites what this record REMEMBERS about the collection
    /// — its `description` and its `deletionProtection` setting.
    ///
    /// Nothing the collection RUNS changes, so the service has nothing to bring up or tear
    /// down for it: the write lands inside the call, the collection keeps the settled
    /// status it already had, and only `lastModifiedDate` moves. That is what leaves the
    /// collection usable by the operations that require a settled status — `DeleteCollection`
    /// above all — on the very next call.
    pub fn apply_metadata_update(&mut self, updated_at: i64) {
        self.last_modified_date = updated_at;
    }

    /// Accept an update that RECONFIGURES the collection itself — its vector-search
    /// acceleration, the one member of an update request that changes what the collection
    /// runs rather than what this record remembers about it: report it `UPDATING`
    /// ("Updating collection resource") from now, settling back to `ACTIVE` once the clock
    /// makes that due. `CollectionStatus` models `UPDATING` beside `ACTIVE` for exactly
    /// this span.
    pub fn begin_update(&mut self, updated_at: i64) {
        self.begin_transition(COLLECTION_UPDATING, COLLECTION_ACTIVE);
        self.last_modified_date = updated_at;
    }

    /// The `CreateCollectionDetail` body for this record — the one place this crate
    /// renders that shape.
    pub fn create_detail(&self) -> CreateCollectionDetail {
        CreateCollectionDetail {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            r#type: Some(self.kind.clone()),
            description: self.description.clone(),
            arn: Some(self.arn.clone()),
            kms_key_arn: Some(self.kms_key_arn.clone()),
            standby_replicas: Some(self.standby_replicas.clone()),
            deletion_protection: Some(self.deletion_protection.clone()),
            vector_options: self.vector_options.clone(),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
            collection_group_name: self.collection_group_name.clone(),
        }
    }

    /// The `CollectionDetail` body for this record — the one place this crate renders
    /// that shape, so `BatchGetCollection` cannot report a record differently from the
    /// create that stored it.
    pub fn detail(&self) -> CollectionDetail {
        CollectionDetail {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            r#type: Some(self.kind.clone()),
            description: self.description.clone(),
            arn: Some(self.arn.clone()),
            kms_key_arn: Some(self.kms_key_arn.clone()),
            standby_replicas: Some(self.standby_replicas.clone()),
            deletion_protection: Some(self.deletion_protection.clone()),
            vector_options: self.vector_options.clone(),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
            collection_endpoint: Some(self.collection_endpoint.clone()),
            dashboard_endpoint: Some(self.dashboard_endpoint.clone()),
            fips_endpoints: Some(FipsEndpoints {
                collection_endpoint: Some(self.fips_collection_endpoint.clone()),
                dashboard_endpoint: Some(self.fips_dashboard_endpoint.clone()),
            }),
            // `failureCode` / `failureMessage` describe a collection whose create or
            // update failed. Nothing in this slice fails a collection, so a stored record
            // carries neither.
            failure_code: None,
            failure_message: None,
            collection_group_name: self.collection_group_name.clone(),
        }
    }

    /// The `CollectionSummary` body for this record — the shape `ListCollections` pages
    /// over.
    pub fn summary(&self) -> CollectionSummary {
        CollectionSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            arn: Some(self.arn.clone()),
            kms_key_arn: Some(self.kms_key_arn.clone()),
            collection_group_name: self.collection_group_name.clone(),
        }
    }

    /// The `DeleteCollectionDetail` body for this record, as the delete that accepted it
    /// reports it.
    pub fn delete_detail(&self) -> DeleteCollectionDetail {
        DeleteCollectionDetail {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            deletion_protection: Some(self.deletion_protection.clone()),
        }
    }

    /// The `UpdateCollectionDetail` body for this record — the one place this crate
    /// renders that shape. The shape models no endpoints and no `standbyReplicas`, so it
    /// reports the members the model declares and nothing else.
    pub fn update_detail(&self) -> UpdateCollectionDetail {
        UpdateCollectionDetail {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            r#type: Some(self.kind.clone()),
            description: self.description.clone(),
            vector_options: self.vector_options.clone(),
            arn: Some(self.arn.clone()),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
            deletion_protection: Some(self.deletion_protection.clone()),
        }
    }
}

/// One stored collection group. Every field a reader reports is held here, except the
/// two a reader DERIVES from the collections beside it (`numberOfCollections` and the
/// current capacity); see [`CollectionGroupRecord::detail`].
#[derive(Debug, Clone)]
pub struct CollectionGroupRecord {
    /// The server-minted `CollectionGroupId`, which is also this record's key.
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The `StandbyReplicas` wire value; `@required` on the create request, so every
    /// stored group carries one.
    pub standby_replicas: String,
    pub description: Option<String>,
    /// The `ServerlessGeneration` wire value, defaulted by
    /// [`SERVERLESS_GENERATION_CLASSIC`] when the create request named none.
    pub generation: String,
    /// The group's EFFECTIVE capacity limits: every member the create (or a later
    /// update) named, plus the service default for each ceiling it did not — see
    /// [`effective_capacity_limits`]. Not an `Option`, because a group always HAS
    /// effective limits: the ones it was asked for, or the defaults it starts at.
    pub capacity_limits: CollectionGroupCapacityLimits,
    /// The tags the create request carried. `CreateCollectionGroupDetail` and
    /// `CollectionGroupDetail` both model `tags`, so unlike a collection a group reports
    /// its own tags on a read.
    pub tags: Vec<Tag>,
    pub created_date: i64,
    /// `lastModifiedDate`, which `UpdateCollectionGroupDetail` reports; equal to
    /// [`created_date`](Self::created_date) until a write moves it.
    pub last_modified_date: i64,
}

impl CollectionGroupRecord {
    /// The `CreateCollectionGroupDetail` body for this record — the one place this crate
    /// renders that shape.
    pub fn create_detail(&self) -> CreateCollectionGroupDetail {
        CreateCollectionGroupDetail {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            standby_replicas: Some(self.standby_replicas.clone()),
            description: self.description.clone(),
            tags: Some(self.tags.clone()),
            created_date: Some(self.created_date),
            capacity_limits: Some(self.capacity_limits.clone()),
            generation: Some(self.generation.clone()),
            storage_configuration: Some(self.storage_configuration()),
        }
    }

    /// The `CollectionGroupDetail` body for this record, with the
    /// `numberOfCollections` its reader counted from the collections beside it, so a read
    /// cannot report a group differently from the create that stored it.
    pub fn detail(&self, number_of_collections: i32) -> CollectionGroupDetail {
        CollectionGroupDetail {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            standby_replicas: Some(self.standby_replicas.clone()),
            description: self.description.clone(),
            tags: Some(self.tags.clone()),
            created_date: Some(self.created_date),
            capacity_limits: Some(self.capacity_limits.clone()),
            current_capacity: self.current_capacity(),
            number_of_collections: Some(number_of_collections),
            generation: Some(self.generation.clone()),
            storage_configuration: Some(self.storage_configuration()),
        }
    }

    /// The `CollectionGroupSummary` body for this record — the listing's row for one
    /// group, with the `numberOfCollections` its reader counted from the collections
    /// beside it exactly as [`Self::detail`] does, so a listing cannot report a group
    /// differently from a read of the same group.
    ///
    /// `CollectionGroupSummary` models only `id`, `arn`, `name`, `numberOfCollections`,
    /// `createdDate`, `capacityLimits` and `generation`
    /// (`smithy_query(query="shape", shape="CollectionGroupSummary")`), so the members
    /// only a detail carries — `standbyReplicas`, `description`, `tags`,
    /// `currentCapacity`, `storageConfiguration` — are not in it.
    pub fn summary(&self, number_of_collections: i32) -> CollectionGroupSummary {
        CollectionGroupSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            number_of_collections: Some(number_of_collections),
            created_date: Some(self.created_date),
            capacity_limits: Some(self.capacity_limits.clone()),
            generation: Some(self.generation.clone()),
        }
    }

    /// The group's `storageConfiguration`, computed from the generation it stores.
    ///
    /// `v2Enabled` reports whether the group's data is held on the next-generation (v2)
    /// storage stack, which is what [`SERVERLESS_GENERATION_NEXTGEN`] selects: a group's
    /// `generation` is the only setting it carries that chooses a storage stack, so a
    /// `NEXTGEN` group is `v2Enabled: true` and a `CLASSIC` one `false`. Reported on every
    /// detail, so a client can read the group's storage mode off either the create or a
    /// later read.
    ///
    /// Only the `CLASSIC` -> `false` half is backed by a recording; the `NEXTGEN` -> `true`
    /// half is INFERRED from what the two generations are, and a later recording of a
    /// `NEXTGEN` group is what would confirm or correct it.
    fn storage_configuration(&self) -> CollectionGroupStorageConfiguration {
        CollectionGroupStorageConfiguration {
            v2_enabled: Some(self.generation == SERVERLESS_GENERATION_NEXTGEN),
        }
    }

    /// The `UpdateCollectionGroupDetail` body for this record — the one place this crate
    /// renders that shape, so an update cannot report a group differently from the read
    /// that follows it.
    pub fn update_detail(&self) -> UpdateCollectionGroupDetail {
        UpdateCollectionGroupDetail {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            capacity_limits: Some(self.capacity_limits.clone()),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
            generation: Some(self.generation.clone()),
        }
    }

    /// Apply the members an `UpdateCollectionGroup` request named, leaving a member it did
    /// NOT name at its stored value: both are optional on a request whose `id` already
    /// identifies the group, and neither is documented as clearing what is stored — the
    /// same partial-write rule [`AccountSettingsRecord::apply`] follows.
    ///
    /// `capacityLimits` is replaced WHOLE rather than merged member by member: the request
    /// carries `CollectionGroupCapacityLimits` as one structure ("Updated capacity limits
    /// for the collection group"), so the limits the request states are the limits the
    /// group has — passed through [`effective_capacity_limits`], so an update that states
    /// no ceiling leaves the group with the service default rather than with none, exactly
    /// as a create does. Every write stamps `lastModifiedDate`.
    pub fn apply_update(
        &mut self,
        description: Option<String>,
        capacity_limits: Option<CollectionGroupCapacityLimits>,
        written_at: i64,
    ) {
        if description.is_some() {
            self.description = description;
        }
        if capacity_limits.is_some() {
            self.capacity_limits = effective_capacity_limits(capacity_limits.as_ref());
        }
        self.last_modified_date = written_at;
    }

    /// The group's current search and indexing capacity.
    ///
    /// Nothing in this emulator runs OCUs, so the only capacity a group demonstrably
    /// holds is the FLOOR its own stored limits name — `minSearchCapacityInOCU` and
    /// `minIndexingCapacityInOCU` — and it holds exactly that while no autoscaling action
    /// is in progress ([`AUTOSCALING_NO_ACTION`]). A group whose limits name no floor
    /// reports no current capacity rather than a number nothing measured — and the
    /// defaults [`effective_capacity_limits`] fills in are CEILINGS, so a group created
    /// without capacity limits names no floor.
    fn current_capacity(&self) -> Option<CurrentCapacity> {
        let limits = &self.capacity_limits;
        let at_floor = |floor: Option<f32>| {
            floor.map(|capacity_in_ocu| CapacityDetails {
                capacity_in_ocu: Some(capacity_in_ocu),
                autoscaling_status: Some(AUTOSCALING_NO_ACTION.to_string()),
            })
        };
        let search = at_floor(limits.min_search_capacity_in_ocu);
        let indexing = at_floor(limits.min_indexing_capacity_in_ocu);
        if search.is_none() && indexing.is_none() {
            return None;
        }
        Some(CurrentCapacity { search, indexing })
    }
}

/// The EFFECTIVE capacity limits of a collection group whose create (or update) request
/// named `requested`.
///
/// Every collection-group detail shape models `capacityLimits`, and the live API reports
/// it on a group whose create named none, so a group's limits are never absent: each
/// member the request named is kept as given, and each CEILING it did not name falls back
/// to the service default ([`DEFAULT_COLLECTION_GROUP_MAX_INDEXING_CAPACITY_IN_OCU`],
/// [`DEFAULT_COLLECTION_GROUP_MAX_SEARCH_CAPACITY_IN_OCU`]). A FLOOR stays absent when
/// the request named none: the recorded default names neither `min*` member, and a floor
/// nobody asked for would otherwise claim capacity this emulator never reserved.
///
/// The default is a CEILING the service fills in, not a bound a caller is held to, so a
/// floor named above it is stored as given rather than refused: only the halves one
/// request named itself are checked against each other
/// ([`validate_capacity_limits`]).
pub fn effective_capacity_limits(
    requested: Option<&CollectionGroupCapacityLimits>,
) -> CollectionGroupCapacityLimits {
    CollectionGroupCapacityLimits {
        max_indexing_capacity_in_ocu: requested
            .and_then(|limits| limits.max_indexing_capacity_in_ocu)
            .or(Some(DEFAULT_COLLECTION_GROUP_MAX_INDEXING_CAPACITY_IN_OCU)),
        max_search_capacity_in_ocu: requested
            .and_then(|limits| limits.max_search_capacity_in_ocu)
            .or(Some(DEFAULT_COLLECTION_GROUP_MAX_SEARCH_CAPACITY_IN_OCU)),
        min_indexing_capacity_in_ocu: requested
            .and_then(|limits| limits.min_indexing_capacity_in_ocu),
        min_search_capacity_in_ocu: requested.and_then(|limits| limits.min_search_capacity_in_ocu),
    }
}

/// An OCU figure as the refusals that name one render it.
///
/// Both the collection-group capacity members and the account-wide totals computed from
/// them are modelled `float` (`smithy_query(query="shape",
/// shape="CollectionGroupCapacityLimits")`), and a float is rendered with a fractional
/// part, so a whole number of OCUs reads `32.0` rather than `32`.
pub fn rendered_ocu(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

/// [`rendered_ocu`] for a figure computed ACROSS members — a request's ceiling plus the
/// capacity already committed — carried as `f64` so a whole number of OCUs past a `f32`'s
/// exact range is still rendered digit for digit.
pub fn rendered_ocu_total(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

/// Refuse `limits` whose floor exceeds its own ceiling, naming the two members and their
/// values.
///
/// `CollectionGroupCapacityLimits` bounds each member on its own (`max*` is
/// `@range(min: 1)`, `min*` is `@range(min: 0)`), which the boundary validator applies
/// before a handler runs; what it cannot express is the relation BETWEEN the pair, so
/// that is checked here. `Ok(())` for limits that name only one half of a pair: a floor
/// with no ceiling, or a ceiling with no floor, contradicts nothing.
pub fn validate_capacity_limits(limits: &CollectionGroupCapacityLimits) -> Result<(), String> {
    for (floor_member, floor, ceiling_member, ceiling) in [
        (
            "minIndexingCapacityInOCU",
            limits.min_indexing_capacity_in_ocu,
            "maxIndexingCapacityInOCU",
            limits.max_indexing_capacity_in_ocu,
        ),
        (
            "minSearchCapacityInOCU",
            limits.min_search_capacity_in_ocu,
            "maxSearchCapacityInOCU",
            limits.max_search_capacity_in_ocu,
        ),
    ] {
        if let (Some(floor), Some(ceiling)) = (floor, ceiling) {
            if floor > ceiling {
                return Err(format!(
                    "Invalid capacity range: {floor_member} ({}) exceeds {ceiling_member} ({})",
                    rendered_ocu(floor),
                    rendered_ocu(ceiling)
                ));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The account's own settings
// ---------------------------------------------------------------------------

/// The account-level settings `GetAccountSettings` reports and `UpdateAccountSettings`
/// writes: the maximum capacity, in OCUs, this account's collections may scale to.
///
/// Held as plain `i32`s rather than as the modelled `CapacityLimits` (whose members are
/// both optional) because the account always HAS both limits: one it was set to, or the
/// default it starts at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountSettingsRecord {
    /// `CapacityLimits$maxIndexingCapacityInOCU`.
    pub max_indexing_capacity_in_ocu: i32,
    /// `CapacityLimits$maxSearchCapacityInOCU`.
    pub max_search_capacity_in_ocu: i32,
}

impl Default for AccountSettingsRecord {
    /// The documented defaults, so a `GetAccountSettings` on an account nobody has
    /// configured reports the limits AWS starts one at instead of nothing.
    fn default() -> Self {
        Self {
            max_indexing_capacity_in_ocu: DEFAULT_MAX_INDEXING_CAPACITY_IN_OCU,
            max_search_capacity_in_ocu: DEFAULT_MAX_SEARCH_CAPACITY_IN_OCU,
        }
    }
}

impl AccountSettingsRecord {
    /// The `AccountSettingsDetail` body for these settings — the one place this crate
    /// renders that shape, so an update cannot report settings a later
    /// `GetAccountSettings` would not.
    pub fn detail(&self) -> AccountSettingsDetail {
        AccountSettingsDetail {
            capacity_limits: Some(CapacityLimits {
                max_indexing_capacity_in_ocu: Some(self.max_indexing_capacity_in_ocu),
                max_search_capacity_in_ocu: Some(self.max_search_capacity_in_ocu),
            }),
        }
    }

    /// Apply the limits an `UpdateAccountSettings` request named, leaving a limit the
    /// request did NOT name at its stored value: both `CapacityLimits` members are
    /// optional, so a request may raise or lower one limit without restating the other.
    ///
    /// Answers whether the stored settings CHANGED, so the caller can report what really
    /// happened: a request naming neither member, or naming only values the account
    /// already holds, leaves this record exactly as it was.
    pub fn apply(&mut self, limits: &CapacityLimits) -> bool {
        let before = *self;
        if let Some(value) = limits.max_indexing_capacity_in_ocu {
            self.max_indexing_capacity_in_ocu = value;
        }
        if let Some(value) = limits.max_search_capacity_in_ocu {
            self.max_search_capacity_in_ocu = value;
        }
        *self != before
    }

    /// The limit this record holds for `kind`.
    pub fn limit(&self, kind: CapacityKind) -> i32 {
        match kind {
            CapacityKind::Indexing => self.max_indexing_capacity_in_ocu,
            CapacityKind::Search => self.max_search_capacity_in_ocu,
        }
    }
}

/// The capacity an account-wide limit, and the quota that bounds it, is about.
///
/// `CapacityLimits` carries one maximum per type and the service's quota is stated per
/// type, so every figure a capacity refusal names — the ceiling asked for, the capacity the
/// collection groups already account for, the quota itself — is read off this one pair
/// rather than spelled twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityKind {
    /// `CapacityLimits$maxIndexingCapacityInOCU`.
    Indexing,
    /// `CapacityLimits$maxSearchCapacityInOCU`.
    Search,
}

impl CapacityKind {
    /// Both capacity types, in the order a refusal checks them.
    pub const ALL: [Self; 2] = [Self::Indexing, Self::Search];

    /// How a quota refusal words this capacity.
    pub fn word(self) -> &'static str {
        match self {
            Self::Indexing => "indexing",
            Self::Search => "search",
        }
    }

    /// The `CapacityLimits` member this capacity is named by, or `None` when the request
    /// named none.
    pub fn requested(self, limits: &CapacityLimits) -> Option<i32> {
        match self {
            Self::Indexing => limits.max_indexing_capacity_in_ocu,
            Self::Search => limits.max_search_capacity_in_ocu,
        }
    }

    /// The `CollectionGroupCapacityLimits` ceiling a collection group commits for this
    /// capacity.
    pub fn group_ceiling(self, limits: &CollectionGroupCapacityLimits) -> Option<f32> {
        match self {
            Self::Indexing => limits.max_indexing_capacity_in_ocu,
            Self::Search => limits.max_search_capacity_in_ocu,
        }
    }
}

/// The members ONE write to a stored policy replaces, as `UpdateAccessPolicy`,
/// `UpdateSecurityPolicy` and `UpdateLifecyclePolicy` carry them.
///
/// All three operations model `description` and `policy` as OPTIONAL on a request whose
/// `type`/`name` already identify the policy, and none is documented as clearing the
/// stored value, so a member the request did not name keeps what is stored — the same
/// partial-write rule [`AccountSettingsRecord::apply`] follows. The other two members are
/// server-derived: every write mints a fresh `policyVersion`
/// (see [`mint_policy_version`])
/// and stamps `lastModifiedDate`.
#[derive(Debug, Clone)]
pub struct PolicyWrite {
    /// The `description` the request named, or `None` when it named none.
    pub description: Option<String>,
    /// The PARSED `policy` document the request named, or `None` when it named none.
    pub policy: Option<::aws_smithy_types::Document>,
    /// The freshly minted `PolicyVersion` this write stores.
    pub policy_version: String,
    /// The epoch-millisecond instant of the write, which becomes `lastModifiedDate`.
    pub written_at: i64,
}

impl PolicyWrite {
    /// Apply this write to one stored policy's mutable members: the ONE place the merge
    /// rule lives, so a data access policy, a security policy and a lifecycle policy
    /// cannot drift apart.
    fn apply(
        self,
        description: &mut Option<String>,
        policy: &mut ::aws_smithy_types::Document,
        policy_version: &mut String,
        last_modified_date: &mut i64,
    ) {
        if self.description.is_some() {
            *description = self.description;
        }
        if let Some(document) = self.policy {
            *policy = document;
        }
        *policy_version = self.policy_version;
        *last_modified_date = self.written_at;
    }
}

/// One stored security policy, keyed by its `(type, name)` pair.
#[derive(Debug, Clone)]
pub struct SecurityPolicyRecord {
    /// The `SecurityPolicyType` wire value; named `kind` because `type` is a keyword.
    pub kind: String,
    pub name: String,
    /// The `PolicyVersion` token, re-minted on every write; see
    /// [`mint_policy_version`].
    pub policy_version: String,
    pub description: Option<String>,
    /// The policy document, held PARSED so a read answers "the JSON policy document
    /// without any whitespaces" (`SecurityPolicyDetail$policy`) rather than the caller's
    /// own formatting.
    pub policy: ::aws_smithy_types::Document,
    pub created_date: i64,
    pub last_modified_date: i64,
}

impl SecurityPolicyRecord {
    /// The `SecurityPolicyDetail` body for this record — the one place this crate renders
    /// that shape.
    pub fn detail(&self) -> SecurityPolicyDetail {
        SecurityPolicyDetail {
            r#type: Some(self.kind.clone()),
            name: Some(self.name.clone()),
            policy_version: Some(self.policy_version.clone()),
            description: self.description.clone(),
            policy: Some(self.policy.clone()),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
        }
    }

    /// The `SecurityPolicySummary` body for this record — the shape `ListSecurityPolicies`
    /// pages over. It is `SecurityPolicyDetail` without the policy document itself, so a
    /// listing and a read cannot disagree about the members they share.
    pub fn summary(&self) -> SecurityPolicySummary {
        SecurityPolicySummary {
            r#type: Some(self.kind.clone()),
            name: Some(self.name.clone()),
            policy_version: Some(self.policy_version.clone()),
            description: self.description.clone(),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
        }
    }

    /// What a listing needs of this record, COPIED OUT of the store: the summary it
    /// reports and the resources its rules name. Taking this copy is the whole of a
    /// listing's work under the state lock, so the pattern matching a `resource` filter
    /// drives runs outside it and one caller's filter cannot block another request.
    ///
    /// Bounded by construction: a type holds at most
    /// [`MAX_SECURITY_POLICIES_PER_TYPE`] records and a document contributes at most
    /// [`MAX_POLICY_RESOURCES`] entries.
    pub fn listing_entry(&self) -> SecurityPolicyListingEntry {
        SecurityPolicyListingEntry {
            summary: self.summary(),
            name: self.name.clone(),
            resources: policy_document_resources(&self.policy),
        }
    }

    /// Apply `write` to this record; see [`PolicyWrite`] for the rule it follows.
    pub fn apply(&mut self, write: PolicyWrite) {
        write.apply(
            &mut self.description,
            &mut self.policy,
            &mut self.policy_version,
            &mut self.last_modified_date,
        );
    }
}

/// The `Resource` entries a policy document's rules name, in document order.
///
/// What the model cannot express: `PolicyDocument` is text with a character class, so the
/// resources a policy applies to live in its CONTENT. AWS's "Network access for Amazon
/// OpenSearch Serverless" and "Encryption at rest for Amazon OpenSearch Serverless"
/// document that content as a `Rules` list whose every rule carries a `ResourceType` and
/// the `Resource` entries it applies to — an encryption policy being one such object and a
/// network policy a LIST of them — which is why both forms are walked here. The resource
/// entry already carries its own type (`collection/<name>`, `index/<collection>/<index>`),
/// so a rule's `ResourceType` adds nothing to what a resource filter can select on. A
/// RETENTION lifecycle policy's document is in that same form — "Amazon OpenSearch
/// Serverless data lifecycle policies" writes it as a `Rules` list whose every rule carries
/// a `ResourceType` and the `Resource` entries it applies to, the form
/// [`validate_retention_policy`] checks — so the resources its rules name are read here too.
///
/// Nothing is validated here: a document that is not in that form simply names no
/// resources, because `CreateSecurityPolicy` accepts any JSON document a caller signs.
/// Each of the three levels is read by [`policy_document_entries`], the one place this
/// module decides what a document's list-valued position holds, so the resources a
/// listing's `resource` filter matches against are exactly the ones the rules an
/// encryption lookup covers a collection by are drawn from.
///
/// At most [`MAX_POLICY_RESOURCES`] entries are collected, and each is at most
/// [`MAX_RESOURCE_PATTERN_LENGTH`] bytes, so neither the length of this list nor the cost
/// of one comparison is chosen by whoever wrote the document.
pub fn policy_document_resources(document: &::aws_smithy_types::Document) -> Vec<String> {
    use ::aws_smithy_types::Document;

    let mut resources: Vec<String> = Vec::new();
    for statement in policy_document_entries(Some(document)) {
        let Document::Object(members) = statement else {
            continue;
        };
        for rule in policy_document_entries(members.get("Rules")) {
            let Document::Object(rule) = rule else {
                continue;
            };
            for named in policy_document_entries(rule.get("Resource")) {
                if resources.len() >= MAX_POLICY_RESOURCES {
                    return resources;
                }
                let Document::String(named) = named else {
                    continue;
                };
                // A resource entry past the pattern bound is dropped rather than matched:
                // its length is whoever wrote the document's to choose, and it is a factor
                // in every comparison's cost.
                if named.len() <= MAX_RESOURCE_PATTERN_LENGTH {
                    resources.push(named.clone());
                }
            }
        }
    }
    resources
}

/// The entries a policy document, or one of its list-valued members, holds — the list
/// spelling and the single-value spelling read alike.
///
/// What the model cannot express: `PolicyDocument` is text with a character class, so a
/// stored document's shape is whatever the caller wrote. AWS's "Encryption at rest for
/// Amazon OpenSearch Serverless" and "Network access for Amazon OpenSearch Serverless"
/// write a policy as one rule-carrying object or a LIST of them, a statement's `Rules` as
/// a list of rules and a rule's `Resource` as a list of resource patterns; a document that
/// writes a single value where such a list belongs holds the one-element list, which is
/// what every reader of that position in this module sees.
///
/// An absent member holds nothing.
fn policy_document_entries(
    member: Option<&::aws_smithy_types::Document>,
) -> Vec<&::aws_smithy_types::Document> {
    use ::aws_smithy_types::Document;

    match member {
        Some(Document::Array(entries)) => entries.iter().collect(),
        Some(single) => vec![single],
        None => Vec::new(),
    }
}

/// One stored policy as a listing sees it: the summary it reports, the `PolicyName` that
/// is its place in the listing's order, and the resources its rules name. Built under the
/// state lock ([`SecurityPolicyRecord::listing_entry`],
/// [`AccessPolicyRecord::listing_entry`]) and used after it is released.
///
/// Generic over the summary shape because a policy listing is ONE rule whatever policy
/// resource it enumerates: the policies of the one `type` the request names, ordered by
/// `PolicyName`, narrowed by the `resource` filter and cut into pages. Only the modelled
/// summary differs between them.
#[derive(Debug, Clone)]
pub struct PolicyListingEntry<Summary> {
    /// The modelled summary the listing reports for this policy.
    pub summary: Summary,
    /// The `PolicyName`, which is this listing's page cursor.
    pub name: String,
    /// The resources the policy's rules name; see [`policy_document_resources`].
    pub resources: Vec<String>,
}

/// What `ListSecurityPolicies` pages over.
pub type SecurityPolicyListingEntry = PolicyListingEntry<SecurityPolicySummary>;

/// What `ListAccessPolicies` pages over.
pub type AccessPolicyListingEntry = PolicyListingEntry<AccessPolicySummary>;

/// What `ListLifecyclePolicies` pages over.
pub type LifecyclePolicyListingEntry = PolicyListingEntry<LifecyclePolicySummary>;

/// A policy listing is of ONE `type` (the member is required), inside which the
/// `PolicyName` is unique — so the name is this entry's place in that listing's order.
impl<Summary> Paged for PolicyListingEntry<Summary> {
    fn page_cursor(&self) -> &str {
        &self.name
    }
}

/// The entries of `candidates` that apply to at least one resource `filter` names — what a
/// listing's `resource` filter selects — or the `ValidationException` message for a filter
/// this emulator will not evaluate.
///
/// A filter entry and a policy resource meet when the resource sets they denote OVERLAP;
/// see [`resource_patterns_overlap`]. An entry whose policy names no resource at all (its
/// rules carry none, or its document is not in the documented rule form) applies to
/// nothing a filter can name, so a filtered listing leaves it out while an unfiltered one
/// still reports it.
///
/// Every factor of the work is bounded BEFORE it runs: the filter's length and each
/// entry's length are refused past [`MAX_RESOURCE_FILTER_ENTRIES`] and
/// [`MAX_RESOURCE_PATTERN_LENGTH`], a document contributes at most
/// [`MAX_POLICY_RESOURCES`] resources of at most that same length, and the PRODUCT — the
/// work this one request performs, charged per comparison by
/// [`resource_overlap_work`] — is capped by [`MAX_RESOURCE_FILTER_WORK`], which trips into
/// the same refusal rather than running on.
pub fn resource_filtered<Summary>(
    candidates: Vec<PolicyListingEntry<Summary>>,
    filter: &[String],
) -> Result<Vec<PolicyListingEntry<Summary>>, String> {
    if filter.len() > MAX_RESOURCE_FILTER_ENTRIES {
        return Err(format!(
            "resource must name at most {MAX_RESOURCE_FILTER_ENTRIES} resources, got {}",
            filter.len()
        ));
    }
    if let Some(entry) = filter
        .iter()
        .find(|entry| entry.len() > MAX_RESOURCE_PATTERN_LENGTH)
    {
        return Err(format!(
            "a resource entry must be at most {MAX_RESOURCE_PATTERN_LENGTH} bytes, got {}",
            entry.len()
        ));
    }

    let mut budget = MAX_RESOURCE_FILTER_WORK;
    let mut kept = Vec::new();
    for candidate in candidates {
        let mut applies = false;
        for wanted in filter {
            for resource in &candidate.resources {
                // Charged BEFORE the comparison, so the budget bounds work done rather
                // than work already spent.
                let Some(left) = budget.checked_sub(resource_overlap_work(resource, wanted))
                else {
                    return Err(format!(
                        "resource names more, or more wildcarded, resources than this \
                         listing matches ({MAX_RESOURCE_FILTER_WORK} bytes of matching); \
                         name fewer resources"
                    ));
                };
                budget = left;
                if resource_patterns_overlap(resource, wanted) {
                    applies = true;
                    break;
                }
            }
            if applies {
                break;
            }
        }
        if applies {
            kept.push(candidate);
        }
    }
    Ok(kept)
}

/// An upper bound, in bytes scanned, on what [`resource_patterns_overlap`] does for one
/// pair — the unit [`MAX_RESOURCE_FILTER_WORK`] is spent in.
///
/// [`resource_pattern_matches`] walks one pattern's literal runs over the other string, so
/// its cost is at most the number of those runs times that string's length, and the
/// overlap test does it in both directions. Two plain names therefore cost their own
/// length, while a pattern full of wildcards costs what it really takes.
fn resource_overlap_work(left: &str, right: &str) -> usize {
    let runs = |pattern: &str| pattern.matches('*').count() + 1;
    let span = left.len() + right.len();
    (runs(left) + runs(right)).saturating_mul(span)
}

/// True when `left` matches `right` read as a plain resource name, or `right` matches
/// `left` read as one.
///
/// Both sides are resource patterns in the form AWS's policy and filter documentation
/// uses: a resource name in which `*` stands for any run of characters ("you can use a
/// wildcard"), and nothing else is special. A filter entry is itself such a pattern, so
/// the test is symmetric — an exact filter meets a wildcard policy resource, and a
/// wildcard filter meets an exact one.
///
/// That is exactly "the resource sets the two denote overlap" whenever at most one side
/// carries a wildcard, and whenever both wildcards are trailing ones, which is the form
/// AWS documents ("you can use a wildcard at the end"). Two patterns wildcarded in the
/// MIDDLE can denote a common resource without either matching the other as a literal
/// (`*-prod` and `logs-*` both denote `logs-prod`); this answers false for that pair
/// rather than solving pattern-against-pattern intersection.
pub fn resource_patterns_overlap(left: &str, right: &str) -> bool {
    resource_pattern_matches(left, right) || resource_pattern_matches(right, left)
}

/// True when the resource `name` is one `pattern` denotes.
///
/// `*` is the only metacharacter, so this is a single left-to-right walk of the pattern's
/// literal runs over `name`: the first run must start it, the last must end it, and each
/// run between must occur after the one before. It compiles nothing and allocates nothing,
/// so the cost of one comparison is linear in the two strings rather than in a matcher
/// built from caller text.
///
/// Not `services/common`'s `iam_authorization::wildcard_match`, which implements IAM's
/// pattern language: that one also treats `?` as a single-character wildcard, and a `?` in
/// an OpenSearch Serverless resource name is a literal character, so reusing it would make
/// this listing select policies whose rules name a different resource.
fn resource_pattern_matches(pattern: &str, name: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == name;
    }
    let mut runs = pattern.split('*').peekable();
    let first = runs.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    while let Some(run) = runs.next() {
        if runs.peek().is_none() {
            // The pattern's last run must END `name`; an empty one is a trailing `*`,
            // which the rest of `name` satisfies whatever it is.
            return run.is_empty() || rest.ends_with(run);
        }
        match rest.find(run) {
            // `find` answers a byte offset at a character boundary.
            Some(at) => rest = &rest[at + run.len()..],
            None => return false,
        }
    }
    true
}

/// The `SecurityConfigId` the configuration `name` carries under `kind`:
/// `<type>/<account-id>/<name>`, the form AWS's own security-configuration ids take (the
/// `GetSecurityConfig` CLI example addresses one as `saml/123456789012/myprovider`). It is
/// derived from the type, the account and the name alone, so every reader of a record
/// reports the same id, and it stays inside `SecurityConfigId`'s `@length(max: 100)`
/// because `SecurityConfigType` is at most 17 characters and `ConfigName` at most 32.
pub fn security_config_id(kind: &str, name: &str) -> String {
    format!("{kind}/{}/{name}", ::engine::emulator_account_id())
}

/// One stored security configuration, keyed by its [`security_config_id`].
///
/// The identity-provider options are held as the shapes a READ reports
/// (`IamIdentityCenterConfigOptions` rather than the create request's
/// `CreateIamIdentityCenterConfigOptions`), with every documented default already applied,
/// so nothing is recomputed from a request when the record is reported.
#[derive(Debug, Clone)]
pub struct SecurityConfigRecord {
    /// The `SecurityConfigId`, which is also this record's key.
    pub id: String,
    /// The `ConfigName` the create request carried. Not modelled on either reported shape,
    /// but it is half of this record's identity: one name may be used once per type.
    pub name: String,
    /// The `SecurityConfigType` wire value; named `kind` because `type` is a Rust keyword.
    pub kind: String,
    /// The `PolicyVersion` token `configVersion` carries, re-minted on every write; see
    /// [`mint_policy_version`].
    pub config_version: String,
    pub description: Option<String>,
    /// Set exactly for a `saml` configuration, which is the type whose options are
    /// required.
    pub saml_options: Option<SamlConfigOptions>,
    /// Set exactly for an `iamidentitycenter` configuration.
    pub iam_identity_center_options: Option<IamIdentityCenterConfigOptions>,
    /// Set exactly for an `iamfederation` configuration.
    pub iam_federation_options: Option<IamFederationConfigOptions>,
    pub created_date: i64,
    pub last_modified_date: i64,
}

impl SecurityConfigRecord {
    /// The `SecurityConfigDetail` body for this record — the one place this crate renders
    /// that shape, so a create cannot report a record differently from a later read.
    pub fn detail(&self) -> SecurityConfigDetail {
        SecurityConfigDetail {
            id: Some(self.id.clone()),
            r#type: Some(self.kind.clone()),
            config_version: Some(self.config_version.clone()),
            description: self.description.clone(),
            saml_options: self.saml_options.clone(),
            iam_identity_center_options: self.iam_identity_center_options.clone(),
            iam_federation_options: self.iam_federation_options.clone(),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
        }
    }

    /// The `SecurityConfigSummary` body for this record — the shape `ListSecurityConfigs`
    /// pages over. It is `SecurityConfigDetail` without the identity-provider options.
    pub fn summary(&self) -> SecurityConfigSummary {
        SecurityConfigSummary {
            id: Some(self.id.clone()),
            r#type: Some(self.kind.clone()),
            config_version: Some(self.config_version.clone()),
            description: self.description.clone(),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
        }
    }

    /// Apply `write` to this record; see [`SecurityConfigWrite`] for the rule it follows.
    pub fn apply(&mut self, write: SecurityConfigWrite) {
        if write.description.is_some() {
            self.description = write.description;
        }
        // `samlOptions` and `iamFederationOptions` arrive as the WHOLE options structure a
        // read reports, so the options the request states are the options the
        // configuration has.
        if let Some(options) = write.saml_options {
            self.saml_options = Some(options);
        }
        if let Some(options) = write.iam_federation_options {
            self.iam_federation_options = Some(options);
        }
        // `iamIdentityCenterOptionsUpdates` is a different shape from the stored
        // `IamIdentityCenterConfigOptions`: it carries only the two attributes, so it
        // PATCHES them and leaves the instance the configuration is integrated with (and
        // the application members) alone — there is nothing in the request to restate them
        // from.
        if let Some(updates) = write.iam_identity_center_updates {
            if let Some(options) = self.iam_identity_center_options.as_mut() {
                if updates.user_attribute.is_some() {
                    options.user_attribute = updates.user_attribute;
                }
                if updates.group_attribute.is_some() {
                    options.group_attribute = updates.group_attribute;
                }
            }
        }
        self.config_version = write.config_version;
        self.last_modified_date = write.written_at;
    }
}

/// The members ONE write to a stored security configuration replaces, as
/// `UpdateSecurityConfig` carries them.
///
/// Every member a request may name is OPTIONAL on a request whose `id` already identifies
/// the configuration, and none is documented as clearing what is stored, so a member the
/// request did not name keeps its stored value — the same partial-write rule
/// [`PolicyWrite`] follows. The other two members are server-derived: every write mints a
/// fresh `configVersion`
/// (see [`mint_policy_version`])
/// and stamps `lastModifiedDate`.
#[derive(Debug, Clone)]
pub struct SecurityConfigWrite {
    /// The `description` the request named, or `None` when it named none.
    pub description: Option<String>,
    /// The `samlOptions` the request named, with the documented `sessionTimeout` default
    /// already applied; `None` when the request named none.
    pub saml_options: Option<SamlConfigOptions>,
    /// The `iamIdentityCenterOptionsUpdates` the request named, which patch the stored
    /// options rather than replacing them.
    pub iam_identity_center_updates: Option<UpdateIamIdentityCenterConfigOptions>,
    /// The `iamFederationOptions` the request named, or `None` when it named none.
    pub iam_federation_options: Option<IamFederationConfigOptions>,
    /// The freshly minted `PolicyVersion` this write stores as `configVersion`.
    pub config_version: String,
    /// The epoch-millisecond instant of the write, which becomes `lastModifiedDate`.
    pub written_at: i64,
}

/// One stored data access policy, keyed by its `(type, name)` pair.
///
/// Held beside [`SecurityPolicyRecord`] rather than sharing it: the two are different
/// resources with their own modelled detail shape, their own `type` value list and their
/// own quota, and `GetAccessPolicy` must not be able to read a security policy (or the
/// other way round) just because the names coincide.
#[derive(Debug, Clone)]
pub struct AccessPolicyRecord {
    /// The `AccessPolicyType` wire value; named `kind` because `type` is a keyword.
    pub kind: String,
    pub name: String,
    /// The `PolicyVersion` token, re-minted on every write; see
    /// [`mint_policy_version`].
    pub policy_version: String,
    pub description: Option<String>,
    /// The policy document, held PARSED so a read answers "the JSON policy document
    /// without any whitespaces" (`AccessPolicyDetail$policy`) rather than the caller's own
    /// formatting.
    pub policy: ::aws_smithy_types::Document,
    pub created_date: i64,
    pub last_modified_date: i64,
}

impl AccessPolicyRecord {
    /// The `AccessPolicyDetail` body for this record — the one place this crate renders
    /// that shape, so a create cannot report a record differently from a later read.
    pub fn detail(&self) -> AccessPolicyDetail {
        AccessPolicyDetail {
            r#type: Some(self.kind.clone()),
            name: Some(self.name.clone()),
            policy_version: Some(self.policy_version.clone()),
            description: self.description.clone(),
            policy: Some(self.policy.clone()),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
        }
    }

    /// The `AccessPolicySummary` body for this record — the shape `ListAccessPolicies`
    /// pages over. It is `AccessPolicyDetail` without the policy document itself, so a
    /// listing and a read cannot disagree about the members they share.
    pub fn summary(&self) -> AccessPolicySummary {
        AccessPolicySummary {
            r#type: Some(self.kind.clone()),
            name: Some(self.name.clone()),
            policy_version: Some(self.policy_version.clone()),
            description: self.description.clone(),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
        }
    }

    /// What a listing needs of this record, COPIED OUT of the store, on the same terms as
    /// [`SecurityPolicyRecord::listing_entry`]: taking the copy is the whole of the
    /// listing's work under the state lock, so the `resource` filter's pattern matching
    /// runs outside it.
    ///
    /// Bounded by construction: the store holds at most [`MAX_ACCESS_POLICIES`] records and
    /// a document contributes at most [`MAX_POLICY_RESOURCES`] entries.
    pub fn listing_entry(&self) -> AccessPolicyListingEntry {
        AccessPolicyListingEntry {
            summary: self.summary(),
            name: self.name.clone(),
            resources: policy_document_resources(&self.policy),
        }
    }

    /// Apply `write` to this record; see [`PolicyWrite`] for the rule it follows.
    pub fn apply(&mut self, write: PolicyWrite) {
        write.apply(
            &mut self.description,
            &mut self.policy,
            &mut self.policy_version,
            &mut self.last_modified_date,
        );
    }
}

/// The `LifecyclePolicyType` whose documents carry retention rules: `retention`, the one
/// value the model's own `LifecyclePolicyType` lists
/// (`smithy_query(query="shape", shape="LifecyclePolicyType")`). The document syntax
/// [`validate_retention_policy`] checks is this type's, so the check is keyed on it.
pub const LIFECYCLE_POLICY_RETENTION: &str = "retention";

/// The shortest `MinIndexRetention` a retention rule may name, in hours.
///
/// From AWS's "Amazon OpenSearch Serverless data lifecycle policies", which documents a
/// rule's minimum retention period as a number of hours or days with "a minimum of 24
/// hours" — so a rule asking for less names a window the service does not serve.
pub const MIN_INDEX_RETENTION_HOURS: u64 = 24;

/// The longest `MinIndexRetention` a retention rule may name, in hours: the same page's
/// maximum of 3650 days (ten years), expressed in the hours a window is compared in.
pub const MAX_INDEX_RETENTION_HOURS: u64 = 3650 * 24;

/// The `MinIndexRetention` window `value` names, in hours, or why it names none.
///
/// AWS's data-lifecycle-policy syntax writes the window as a count followed by a unit —
/// `24h` for hours or `7d` for days — so anything else is not a retention period at all.
fn retention_hours(value: &str) -> Result<u64, String> {
    let (count, unit_hours) = match value.as_bytes().last() {
        Some(b'h') => (&value[..value.len() - 1], 1),
        Some(b'd') => (&value[..value.len() - 1], 24),
        _ => {
            return Err(format!(
                "{value:?} must be a count of hours or days, as in \"24h\" or \"7d\""
            ))
        }
    };
    // `str::parse` accepts a leading `+`, which the documented syntax does not: the count
    // is a run of digits and nothing else.
    if count.is_empty() || !count.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!(
            "{value:?} must be a count of hours or days, as in \"24h\" or \"7d\""
        ));
    }
    let count: u64 = count
        .parse()
        .map_err(|_| format!("{value:?} names more hours than a retention window can hold"))?;
    count
        .checked_mul(unit_hours)
        .ok_or_else(|| format!("{value:?} names more hours than a retention window can hold"))
}

/// Check a `retention` lifecycle policy document against the rule syntax AWS documents,
/// answering the `ValidationException` message for the first thing wrong with it.
///
/// What the model cannot express: `PolicyDocument` is text with a character class, so the
/// boundary validator says nothing about the document's CONTENT. AWS's "Amazon OpenSearch
/// Serverless data lifecycle policies" documents that content — a `Rules` list, each rule
/// naming `ResourceType`, the `Resource` entries it applies to, and either a
/// `MinIndexRetention` window or `NoMinIndexRetention` for unlimited retention, never both
/// — and this service's own model confirms the pair a read reports them as
/// (`EffectiveLifecyclePolicyDetail$retentionPeriod`, "the minimum number of index
/// retention in days or hours", beside `$noMinRetentionPeriod`, "`true` if the minimum
/// number of days or hours is not set") and the single `ResourceType` value, `index`.
pub fn validate_retention_policy(document: &serde_json::Value) -> Result<(), String> {
    let Some(rules) = document.get("Rules") else {
        return Err("policy must carry a Rules list".to_string());
    };
    let Some(rules) = rules.as_array() else {
        return Err("policy.Rules must be a list of rules".to_string());
    };
    if rules.is_empty() {
        return Err("policy.Rules must name at least one rule".to_string());
    }
    for (index, rule) in rules.iter().enumerate() {
        let at = format!("policy.Rules[{index}]");
        if !rule.is_object() {
            return Err(format!("{at} must be an object"));
        }

        // `ResourceType`'s own value list in this service's model is the single value
        // `index` (`smithy_query(query="shape", shape="ResourceType")`).
        let resource_type = rule.get("ResourceType").and_then(serde_json::Value::as_str);
        match resource_type {
            Some(resource_type)
                if crate::ops::types::RESOURCE_TYPE_VALUES.contains(&resource_type) => {}
            Some(resource_type) => {
                return Err(format!(
                    "{at}.ResourceType must be one of [{}], got {resource_type:?}",
                    crate::ops::types::RESOURCE_TYPE_VALUES.join(", ")
                ))
            }
            None => return Err(format!("{at}.ResourceType must name a resource type")),
        }

        let Some(resources) = rule.get("Resource").and_then(serde_json::Value::as_array) else {
            return Err(format!("{at}.Resource must be a list of resource names"));
        };
        if resources.is_empty() {
            return Err(format!("{at}.Resource must name at least one resource"));
        }
        for resource in resources {
            match resource.as_str() {
                Some(resource) if !resource.is_empty() => {}
                _ => {
                    return Err(format!(
                        "{at}.Resource entries must be non-empty resource names"
                    ))
                }
            }
        }

        let no_minimum = match rule.get("NoMinIndexRetention") {
            None => false,
            Some(value) => value.as_bool().ok_or_else(|| {
                format!("{at}.NoMinIndexRetention must be true or false")
            })?,
        };
        let minimum = rule.get("MinIndexRetention");
        match (minimum, no_minimum) {
            (Some(_), true) => {
                return Err(format!(
                    "{at} must name either MinIndexRetention or NoMinIndexRetention, not both"
                ))
            }
            (None, false) => {
                return Err(format!(
                    "{at} must name either MinIndexRetention or NoMinIndexRetention"
                ))
            }
            (None, true) => {}
            (Some(minimum), false) => {
                let Some(minimum) = minimum.as_str() else {
                    return Err(format!(
                        "{at}.MinIndexRetention must be a retention window, as in \"24h\""
                    ));
                };
                let hours = retention_hours(minimum)
                    .map_err(|why| format!("{at}.MinIndexRetention {why}"))?;
                if !(MIN_INDEX_RETENTION_HOURS..=MAX_INDEX_RETENTION_HOURS).contains(&hours) {
                    return Err(format!(
                        "{at}.MinIndexRetention must be between {MIN_INDEX_RETENTION_HOURS}h \
                         and {MAX_INDEX_RETENTION_HOURS}h, got {minimum:?}"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// One stored lifecycle policy, keyed by its `(type, name)` pair.
///
/// Held beside [`AccessPolicyRecord`] and [`SecurityPolicyRecord`] rather than sharing
/// either: a lifecycle policy is its own resource with its own detail shape, its own `type`
/// value list and its own reader, so `BatchGetLifecyclePolicy` must not be able to read a
/// security or data access policy just because the names coincide.
#[derive(Debug, Clone)]
pub struct LifecyclePolicyRecord {
    /// The `LifecyclePolicyType` wire value; named `kind` because `type` is a keyword.
    pub kind: String,
    pub name: String,
    /// The `PolicyVersion` token, re-minted on every write; see
    /// [`mint_policy_version`].
    pub policy_version: String,
    pub description: Option<String>,
    /// The policy document, held PARSED so a read answers "the JSON policy document
    /// without any whitespaces" (`LifecyclePolicyDetail$policy`) rather than the caller's
    /// own formatting.
    pub policy: ::aws_smithy_types::Document,
    pub created_date: i64,
    pub last_modified_date: i64,
}

impl LifecyclePolicyRecord {
    /// The `LifecyclePolicyDetail` body for this record — the one place this crate renders
    /// that shape, so a create cannot report a record differently from a later read.
    pub fn detail(&self) -> LifecyclePolicyDetail {
        LifecyclePolicyDetail {
            r#type: Some(self.kind.clone()),
            name: Some(self.name.clone()),
            policy_version: Some(self.policy_version.clone()),
            description: self.description.clone(),
            policy: Some(self.policy.clone()),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
        }
    }

    /// The `LifecyclePolicySummary` body for this record — the shape
    /// `ListLifecyclePolicies` pages over. It is `LifecyclePolicyDetail` without the policy
    /// document itself, so a listing and a read cannot disagree about the members they
    /// share.
    pub fn summary(&self) -> LifecyclePolicySummary {
        LifecyclePolicySummary {
            r#type: Some(self.kind.clone()),
            name: Some(self.name.clone()),
            policy_version: Some(self.policy_version.clone()),
            description: self.description.clone(),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
        }
    }

    /// What a listing needs of this record, COPIED OUT of the store; see
    /// [`SecurityPolicyRecord::listing_entry`] for why the copy is taken under the lock and
    /// what bounds it.
    pub fn listing_entry(&self) -> LifecyclePolicyListingEntry {
        LifecyclePolicyListingEntry {
            summary: self.summary(),
            name: self.name.clone(),
            resources: policy_document_resources(&self.policy),
        }
    }

    /// The retention rules this policy's document carries, COPIED OUT of the store; see
    /// [`RetentionRule`] for why the copy is taken under the lock and what bounds it.
    ///
    /// The document is read at the same three levels, through the same
    /// [`policy_document_entries`], as [`policy_document_resources`] reads it at, so the
    /// rules an effective-policy read resolves against are carried by exactly the resources
    /// a listing's `resource` filter matches a policy by. A position that does not hold what
    /// the documented rule form puts there contributes no rule rather than an error:
    /// [`validate_retention_policy`] is what refuses such a document at the write.
    pub fn retention_rules(&self) -> Vec<RetentionRule> {
        use ::aws_smithy_types::Document;

        let mut rules: Vec<RetentionRule> = Vec::new();
        let mut patterns = 0usize;
        for statement in policy_document_entries(Some(&self.policy)) {
            let Document::Object(members) = statement else {
                continue;
            };
            for rule in policy_document_entries(members.get("Rules")) {
                let Document::Object(rule) = rule else {
                    continue;
                };
                let Some(Document::String(resource_type)) = rule.get("ResourceType") else {
                    continue;
                };
                let mut resource_patterns: Vec<String> = Vec::new();
                for named in policy_document_entries(rule.get("Resource")) {
                    if patterns >= MAX_POLICY_RESOURCES {
                        return rules;
                    }
                    let Document::String(named) = named else {
                        continue;
                    };
                    // A pattern past the length bound is dropped rather than matched, on the
                    // same terms as [`policy_document_resources`] drops it.
                    if named.len() <= MAX_RESOURCE_PATTERN_LENGTH {
                        patterns += 1;
                        resource_patterns.push(named.clone());
                    }
                }
                rules.push(RetentionRule {
                    policy_type: self.kind.clone(),
                    policy_name: self.name.clone(),
                    resource_type: resource_type.clone(),
                    resource_patterns,
                    retention_period: match rule.get("MinIndexRetention") {
                        Some(Document::String(window)) => Some(window.clone()),
                        // No window named is no minimum retention: the `NoMinIndexRetention`
                        // spelling of the same rule.
                        _ => None,
                    },
                });
            }
        }
        rules
    }

    /// Apply `write` to this record; see [`PolicyWrite`] for the rule it follows.
    pub fn apply(&mut self, write: PolicyWrite) {
        write.apply(
            &mut self.description,
            &mut self.policy,
            &mut self.policy_version,
            &mut self.last_modified_date,
        );
    }
}

/// One retention rule of one stored lifecycle policy, COPIED OUT of the store on the same
/// terms as [`LifecyclePolicyRecord::listing_entry`]: taking the copy is the whole of an
/// effective-policy read's work under the state lock, so the resource-pattern matching that
/// read drives runs outside it.
///
/// Bounded by construction: the store holds at most [`MAX_LIFECYCLE_POLICIES`] records, one
/// document contributes at most [`MAX_POLICY_RESOURCES`] patterns and each of those is at
/// most [`MAX_RESOURCE_PATTERN_LENGTH`] bytes.
#[derive(Debug, Clone)]
pub struct RetentionRule {
    /// The `LifecyclePolicyType` of the policy that carries the rule, so a read of one type
    /// never resolves against another type's rules.
    pub policy_type: String,
    /// The `PolicyName` of the policy that carries the rule — what an effective read reports
    /// as `policyName`.
    pub policy_name: String,
    /// The rule's own `ResourceType`, which is the kind of resource it applies to.
    pub resource_type: String,
    /// The rule's `Resource` entries, each a resource pattern in the form
    /// [`resource_pattern_matches`] reads.
    pub resource_patterns: Vec<String>,
    /// The `MinIndexRetention` window the rule names, or `None` when it names no minimum at
    /// all (`NoMinIndexRetention`) — the two spellings [`validate_retention_policy`]
    /// accepts.
    pub retention_period: Option<String>,
}

/// The `ResourceType` the resource name `resource` addresses: the segment before its first
/// `/`, the prefix AWS's resource names and the `Resource` patterns of a policy rule are
/// both written with (`index/<collection>/<index>`). `None` when that segment is not one of
/// the model's own `ResourceType` values.
pub fn resource_type_of(resource: &str) -> Option<&'static str> {
    let addressed = resource.split('/').next().unwrap_or_default();
    crate::ops::types::RESOURCE_TYPE_VALUES
        .iter()
        .copied()
        .find(|value| *value == addressed)
}

/// How specific a resource pattern is, as an order: the literal prefix it pins before its
/// first `*`, and then whether it pins the whole name (no wildcard at all).
///
/// A pattern that matches a given resource name pins a prefix of that name, so the longer
/// the pinned prefix the narrower the set of resources the pattern denotes; a wildcard-free
/// pattern pins the name itself, which is narrower still than a wildcarded pattern pinning
/// the same characters (`index/sales/orders` against `index/sales/orders*`).
fn pattern_specificity(pattern: &str) -> (usize, bool) {
    match pattern.split_once('*') {
        Some((pinned, _)) => (pinned.len(), false),
        None => (pattern.len(), true),
    }
}

/// The rule of `rules` that decides the effective lifecycle policy of the `resource` named
/// under the policy type `kind`, or `None` when none of them applies to it.
///
/// AWS's "Amazon OpenSearch Serverless data lifecycle policies" documents the precedence
/// that resolves several applicable rules: the most specific matching resource pattern
/// decides. [`pattern_specificity`] is that order, and rules that tie in it are resolved by
/// the order the store holds them in — policy type and name, then document order — so the
/// answer is a property of the stored policies rather than of the walk.
///
/// `budget` bounds the matching work, charged per comparison by [`resource_overlap_work`]
/// exactly as a listing's `resource` filter is: a read whose identifiers and stored patterns
/// together exceed it is refused with the message answered here rather than running on.
pub fn effective_retention_rule<'rules>(
    rules: &'rules [RetentionRule],
    kind: &str,
    resource: &str,
    budget: &mut usize,
) -> Result<Option<&'rules RetentionRule>, String> {
    let addressed = resource_type_of(resource);
    let mut chosen: Option<(&RetentionRule, (usize, bool))> = None;
    for rule in rules
        .iter()
        .filter(|rule| rule.policy_type == kind && Some(rule.resource_type.as_str()) == addressed)
    {
        for pattern in &rule.resource_patterns {
            // Charged BEFORE the comparison, so the budget bounds work done rather than
            // work already spent.
            let Some(left) = budget.checked_sub(resource_overlap_work(pattern, resource)) else {
                return Err(format!(
                    "resourceIdentifiers names more, or more wildcarded, resources than this \
                     read matches ({MAX_RESOURCE_FILTER_WORK} bytes of matching); name fewer \
                     resources"
                ));
            };
            *budget = left;
            // The identifier names ONE resource rather than a pattern, so this is the
            // one-directional match — a stored pattern against that name — not the
            // symmetric overlap a listing's filter entry takes part in.
            if !resource_pattern_matches(pattern, resource) {
                continue;
            }
            let specificity = pattern_specificity(pattern);
            if chosen.is_none_or(|(_, held)| specificity > held) {
                chosen = Some((rule, specificity));
            }
        }
    }
    Ok(chosen.map(|(rule, _)| rule))
}

/// The `EffectiveLifecyclePolicyDetail` for the identifier `(kind, resource)`, answered from
/// `rule` — the rule [`effective_retention_rule`] chose, or `None` when no stored policy of
/// that type applies to the resource.
///
/// The one place this crate renders that shape. A rule naming a `MinIndexRetention` window
/// reports it as `retentionPeriod` beside a `noMinRetentionPeriod` of `false`; a rule naming
/// no minimum, and a resource no rule applies to, report `noMinRetentionPeriod` `true` and
/// no window — the member's own documentation, "that will return as `true` if the minimum
/// number of days or hours is not set to a index resource".
pub fn effective_lifecycle_policy_detail(
    kind: &str,
    resource: &str,
    rule: Option<&RetentionRule>,
) -> EffectiveLifecyclePolicyDetail {
    EffectiveLifecyclePolicyDetail {
        r#type: Some(kind.to_string()),
        resource: Some(resource.to_string()),
        policy_name: rule.map(|rule| rule.policy_name.clone()),
        // The applying rule's `ResourceType` when one applies, and otherwise the kind of
        // resource the name itself addresses: the detail describes that resource either way.
        resource_type: rule
            .map(|rule| rule.resource_type.clone())
            .or_else(|| resource_type_of(resource).map(str::to_string)),
        retention_period: rule.and_then(|rule| rule.retention_period.clone()),
        no_min_retention_period: Some(rule.is_none_or(|rule| rule.retention_period.is_none())),
    }
}

/// A `LifecyclePolicyErrorDetail` for an `identifiers` entry that names no stored lifecycle
/// policy.
pub fn lifecycle_policy_error_for(kind: &str, name: &str) -> LifecyclePolicyErrorDetail {
    LifecyclePolicyErrorDetail {
        r#type: Some(kind.to_string()),
        name: Some(name.to_string()),
        error_message: Some(ERROR_MESSAGE_LIFECYCLE_POLICY_NOT_FOUND.to_string()),
        error_code: Some(ERROR_CODE_NOT_FOUND.to_string()),
    }
}

/// A `CollectionErrorDetail` for an `ids` entry that names no stored collection.
pub fn collection_error_for_id(id: &str) -> CollectionErrorDetail {
    CollectionErrorDetail {
        id: Some(id.to_string()),
        name: None,
        error_message: Some(ERROR_MESSAGE_NOT_FOUND.to_string()),
        error_code: Some(ERROR_CODE_NOT_FOUND.to_string()),
    }
}

/// A `CollectionErrorDetail` for a `names` entry that names no stored collection.
pub fn collection_error_for_name(name: &str) -> CollectionErrorDetail {
    CollectionErrorDetail {
        id: None,
        name: Some(name.to_string()),
        error_message: Some(ERROR_MESSAGE_NOT_FOUND.to_string()),
        error_code: Some(ERROR_CODE_NOT_FOUND.to_string()),
    }
}

/// The `SecurityPolicyType` whose policies say how a collection is encrypted
/// (`smithy_query(query="shape", shape="SecurityPolicyType")` -> `encryption`).
pub const SECURITY_POLICY_TYPE_ENCRYPTION: &str = "encryption";

/// The `resourceType` a quota refusal names for a TAGGED resource: the collections this
/// slice tags are `collection` resources (the segment their ARN carries,
/// `…:collection/<id>`).
pub const TAGGED_RESOURCE_TYPE: &str = "collection";

/// The `resourceType` a quota refusal names for a managed interface endpoint: the resource
/// `CreateVpcEndpoint` creates, spelled as this service's other refusals spell theirs (the
/// resource noun in lower case, hyphenated).
pub const VPC_ENDPOINT_RESOURCE_TYPE: &str = "vpc-endpoint";

/// The `resourceType` a quota refusal names for the ACCOUNT's own settings: the resource
/// `UpdateAccountSettings` writes, spelled the way the resource types above are.
pub const ACCOUNT_SETTINGS_RESOURCE_TYPE: &str = "account-settings";

/// The `ResourceType` a security-policy rule carries for a rule about collections, and the
/// prefix its `Resource` patterns are written with: AWS's security-policy documentation
/// spells those patterns `collection/<name>`.
pub const POLICY_COLLECTION_RESOURCE_TYPE: &str = "collection";

/// The key an encryption policy assigns to the collection it covers.
#[derive(Debug, Clone)]
pub struct PolicyEncryption {
    /// The customer managed key the policy names, or `None` when it assigns the
    /// AWS-owned key — the case [`AWS_OWNED_KEY`] is reported for.
    pub kms_key_arn: Option<String>,
}

/// The statement of `policy` whose `Rules` cover `resource`, if one does.
///
/// A policy document is either one statement object or a list of them (a network policy is
/// written as a list; an encryption policy as a single object), so both spellings are
/// walked — by [`policy_document_entries`], the same reader a listing's resource filter
/// goes through. The statement is answered rather than a bare `bool` because the key the
/// policy assigns sits on the SAME statement as the rule that matched.
fn covering_statement<'a>(
    policy: &'a ::aws_smithy_types::Document,
    resource: &str,
) -> Option<&'a ::aws_smithy_types::Document> {
    policy_document_entries(Some(policy))
        .into_iter()
        .find(|statement| statement_covers(statement, resource))
}

/// Whether one policy statement's `Rules` cover `resource`.
fn statement_covers(statement: &::aws_smithy_types::Document, resource: &str) -> bool {
    use ::aws_smithy_types::Document;

    let Document::Object(members) = statement else {
        return false;
    };
    policy_document_entries(members.get("Rules"))
        .into_iter()
        .any(|rule| rule_covers(rule, resource))
}

/// Whether one `Rules` entry applies to `resource`: a rule about collections, naming a
/// `Resource` pattern that matches.
///
/// AWS's security-policy documentation writes such a pattern as `collection/<name>`, with
/// a wildcard allowed to match a prefix (`collection/log*`, `collection/*`); the match
/// itself is [`resource_pattern_matches`], the one `*` walk this module has, and the
/// document positions it reads the patterns out of are [`policy_document_entries`], the
/// one shape reader, so a policy's rule and a listing's filter can neither read the same
/// pattern differently nor find different patterns in the same document.
fn rule_covers(rule: &::aws_smithy_types::Document, resource: &str) -> bool {
    use ::aws_smithy_types::Document;

    let Document::Object(members) = rule else {
        return false;
    };
    let about_collections = matches!(
        members.get("ResourceType"),
        Some(Document::String(kind)) if kind == POLICY_COLLECTION_RESOURCE_TYPE
    );
    if !about_collections {
        return false;
    }
    policy_document_entries(members.get("Resource"))
        .into_iter()
        .any(|pattern| match pattern {
            Document::String(pattern) => resource_pattern_matches(pattern, resource),
            _ => false,
        })
}

/// A list of statuses as the sentence fragment `ConflictException`'s own documentation
/// states them in — `'ACTIVE', 'FAILED', or 'UPDATE_FAILED'` — so a refusal's text and the
/// check it explains are read off one list and can never disagree, whichever resource's
/// list it is ([`DELETABLE_STATUSES`], [`VPC_ENDPOINT_DELETABLE_STATUSES`]).
pub fn statuses_phrase(statuses: &[&str]) -> String {
    joined_with_or(statuses.iter().map(|status| format!("'{status}'")))
}

/// The same list as [`statuses_phrase`] renders, with the status names UNQUOTED — the way
/// a refusal that names them inline in its own sentence states them
/// ([`collection_not_updatable_message`]).
pub fn plain_statuses_phrase(statuses: &[&str]) -> String {
    joined_with_or(statuses.iter().map(|status| (*status).to_string()))
}

/// `parts` as one sentence fragment: the last two joined by `or`, every earlier one
/// comma-separated, which is the form `ConflictException`'s own documentation states a
/// list of statuses in.
fn joined_with_or(parts: impl IntoIterator<Item = String>) -> String {
    let parts: Vec<String> = parts.into_iter().collect();
    match parts.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, [only])) => format!("{only} or {last}"),
        Some((last, rest)) => format!("{}, or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// How this service words a collection that is not there, from its id.
pub fn collection_not_found_message(id: &str) -> String {
    format!("Collection with ID {id} is not found")
}

/// How this service words a collection that is not in a status it may be updated from,
/// from the collection ID the request addressed and the statuses an update IS accepted in
/// ([`UPDATABLE_STATUSES`]).
///
/// The statuses are read off the same list the precondition checks, so the sentence always
/// names every status from which the update would have been accepted.
pub fn collection_not_updatable_message(id: &str) -> String {
    format!(
        "Collection Id {id} cannot be updated because it is not in {} Status",
        plain_statuses_phrase(&UPDATABLE_STATUSES)
    )
}

/// How this service words a POLICY resource that is not there, from its own name and type.
///
/// This service calls every named, typed resource of its security plane a `Policy` in a
/// refusal — the data access policy, the security policy, the lifecycle policy, and the
/// security CONFIGURATION a write addresses ([`security_config_write_not_found_message`]) —
/// so the one sentence a client keys on is built here rather than in each handler.
pub fn policy_not_found_message(name: &str, kind: &str) -> String {
    format!("Policy with name {name} and type {kind} is not found")
}

/// How this service words a create whose `(type, name)` pair is already taken: the
/// counterpart of [`policy_not_found_message`], and one wording for every policy resource
/// a create addresses by that pair.
pub fn policy_already_exists_message(name: &str, kind: &str) -> String {
    format!("Policy with name {name} and type {kind} already exists")
}

/// The type and the name a `SecurityConfigId` encodes, as `(kind, name)`.
///
/// An id of the documented `<type>/<account-id>/<name>` form ([`security_config_id`])
/// carries the type in its FIRST segment and the name in its LAST one, so both are read
/// back off the id a request addressed the configuration by. `SecurityConfigId` declares no
/// `@pattern`, so an id carrying no separator at all encodes neither and answers `None`.
pub fn parse_security_config_id(id: &str) -> Option<(&str, &str)> {
    let (kind, rest) = id.split_once('/')?;
    let name = rest.rsplit_once('/').map_or(rest, |(_, name)| name);
    if kind.is_empty() || name.is_empty() {
        return None;
    }
    Some((kind, name))
}

/// How the WRITE paths word a security configuration that is not there: as the `Policy`
/// this service calls it, named by the type and the name its `id` encodes
/// ([`parse_security_config_id`]). An id encoding neither is named as the id it is, since
/// there is no name or type in it to render.
///
/// `UpdateSecurityConfig` and `DeleteSecurityConfig` address a configuration the way a
/// caller addresses the other policy resources — by what it IS rather than by its id — and
/// the READ path keeps the id-addressed wording ([`security_config_not_found_message`]).
pub fn security_config_write_not_found_message(id: &str) -> String {
    match parse_security_config_id(id) {
        Some((kind, name)) => policy_not_found_message(name, kind),
        None => security_config_not_found_message(id),
    }
}

/// How this service words a managed interface endpoint that is not there, from the ID the
/// request addressed it by: the kind spelled `VpcEndpoint`, the id SINGLE-QUOTED, and a
/// trailing period.
///
/// The id is what a caller deleting several endpoints tells one refusal from another by, so
/// it is rendered rather than left out. This is the wording of the operations that model
/// `ResourceNotFoundException`; one that models none words the same cause as a conflict
/// ([`vpc_endpoint_absent_conflict_message`]).
pub fn vpc_endpoint_not_found_message(id: &str) -> String {
    format!("VpcEndpoint with ID '{id}' cannot be found.")
}

/// How an operation that models NO not-found error words a managed interface endpoint that
/// is not there: as the state conflict it reports instead, naming the same id.
pub fn vpc_endpoint_absent_conflict_message(id: &str) -> String {
    format!("The VpcEndpoint with Id '{id}' is not exist.")
}

/// How this service words a security configuration that is not there, from the `id` the
/// request addressed it by.
///
/// The wording of the READ path, which addresses the configuration by its id and so names
/// the id back. The WRITE paths name the configuration as the policy it is
/// ([`security_config_write_not_found_message`]).
pub fn security_config_not_found_message(id: &str) -> String {
    format!("Security config with id: {id} does not exist")
}

/// How this service words a collection whose `deletionProtection` is `ENABLED`, from the
/// collection's own NAME.
///
/// `DeleteCollection` addresses the collection by id, but the refusal names the collection
/// the way a caller does when creating it; the name is read from the stored record.
pub fn deletion_protection_message(name: &str) -> String {
    format!(
        "Collection {name} cannot be deleted because deletion protection is enabled. Disable \
         deletion protection before deleting the collection."
    )
}

/// How this service words a collection group that still holds collections, from the group
/// id the request addressed.
pub fn collection_group_not_empty_message(id: &str) -> String {
    format!("CollectionGroup with ID {id}, has collections associated and cannot be deleted")
}

/// How this service words a create whose collection has no encryption setting at all —
/// neither an `encryptionConfig` in the request nor an `encryption` security policy
/// covering the name — from the `name` the request carried.
pub fn missing_encryption_policy_message(name: &str) -> String {
    format!(
        "No matching security policy of encryption type found for collection name: {name}. \
         Please create security policy of encryption type for this collection."
    )
}

/// How every optimistic-lock refusal in this service is worded: the version the request
/// carried is not the stored one, so the resource has been written since the caller read
/// it.
///
/// Entirely fixed text, and ONE text for all four policy/configuration update operations,
/// since the condition a client recovers from is the same — refetch the resource, rebuild
/// the request — whichever resource moved on.
pub const STALE_POLICY_VERSION_MESSAGE: &str =
    "Policy version specified in the request refers to an older version and policy has since \
     changed";

/// How a policy document whose member is present but EMPTY is refused, from the JSON path
/// of that member.
///
/// What the model cannot express: `PolicyDocument` is text with a character class, so the
/// boundary validator says nothing about the document's content. AWS's "Data access
/// policies for Amazon OpenSearch Serverless" documents a statement's `Principal` as a
/// required list of principals, and the live service refuses a document that breaks a
/// list's minimum with this one sentence, naming the offending member's JSON path.
pub fn policy_json_invalid_message(violations: &[String]) -> String {
    format!("Policy json is invalid, error: [{}]", violations.join(", "))
}

/// The violation clause [`policy_json_invalid_message`] carries for the member at `path`
/// being an empty array.
pub fn empty_array_violation(path: &str) -> String {
    format!("{path}: there must be a minimum of 1 items in the array")
}

/// The statements of a DATA ACCESS policy document whose `Principal` list is present and
/// empty, as the JSON paths the refusal names them by (`$[<statement index>].Principal`).
///
/// A statement that grants nothing to nobody is not a policy the service stores: the
/// `Principal` list is required to name at least one principal. The document is either one
/// statement object or a list of them — the two spellings [`policy_document_entries`]
/// reads alike — and the index a path carries is the statement's position in that list, so
/// a single-statement document reports `$[0]`.
pub fn empty_principal_paths(document: &::aws_smithy_types::Document) -> Vec<String> {
    use ::aws_smithy_types::Document;

    let mut paths: Vec<String> = Vec::new();
    for (index, statement) in policy_document_entries(Some(document))
        .into_iter()
        .enumerate()
    {
        let Document::Object(members) = statement else {
            continue;
        };
        if let Some(Document::Array(principals)) = members.get("Principal") {
            if principals.is_empty() {
                paths.push(format!("$[{index}].Principal"));
            }
        }
    }
    paths
}

/// Refuse a data access policy document that breaks a list minimum, answering the
/// `ValidationException` message for it. `Ok(())` for a document that breaks none.
///
/// `CreateAccessPolicy` and `UpdateAccessPolicy` both store one, so the check lives here
/// and the two cannot disagree about which documents are stored.
pub fn validate_access_policy_document(
    document: &::aws_smithy_types::Document,
) -> Result<(), String> {
    let violations: Vec<String> = empty_principal_paths(document)
        .iter()
        .map(|path| empty_array_violation(path))
        .collect();
    if violations.is_empty() {
        return Ok(());
    }
    Err(policy_json_invalid_message(&violations))
}

/// How the index operations word a collection id that names no collection. `CreateIndex`,
/// `GetIndex`, `UpdateIndex` and `DeleteIndex` address the COLLECTION by id, so the
/// refusal names that collection.
pub fn index_collection_not_found_message(id: &str) -> String {
    format!("The Collection with Id '{id}' does not exist.")
}

/// How the index operations word an index that is not there: the fixed text below plus the
/// `indexName` the request carried, and nothing else. The collection id does NOT appear —
/// an `id` that names no collection is the DIFFERENT refusal worded by
/// [`index_collection_not_found_message`], and that one keeps naming the collection.
///
/// `GetIndex`, `UpdateIndex` and `DeleteIndex` each address an existing index this way, so
/// the wording of that one refusal is read off one place and the three cannot disagree
/// about it.
pub fn index_not_found_message(index_name: &str) -> String {
    format!("Index not found: {index_name}")
}

/// How the index operations word an `indexSchema` past the bounds this emulator stores a
/// caller-supplied schema under ([`MAX_DOCUMENT_DEPTH`], [`MAX_INDEX_SCHEMA_NODES`]).
///
/// `CreateIndex` and `UpdateIndex` each store one, so the refusal that states the bound
/// lives beside the bound rather than in each handler.
pub fn index_schema_bounds_message() -> String {
    format!(
        "indexSchema must not nest deeper than {MAX_DOCUMENT_DEPTH} levels or carry more than \
         {MAX_INDEX_SCHEMA_NODES} values"
    )
}

/// How this service words a collection group that is not there, from the ID a request
/// addressed it by: the kind spelled `CollectionGroup` as one word, the id SINGLE-QUOTED,
/// and a trailing period — the same spelling of the kind that
/// [`collection_group_not_empty_message`] uses.
///
/// The wording of the operations that model `ResourceNotFoundException` for a
/// `CollectionGroupId`. A request that names the group by NAME is answered with
/// [`collection_group_name_not_found_message`].
pub fn collection_group_not_found_message(id: &str) -> String {
    format!("CollectionGroup with ID '{id}' cannot be found.")
}

/// How this service words a collection group that is not there, from its NAME — the way a
/// request that names the group it wants its collection in addresses one. The
/// id-addressed wording is [`collection_group_not_found_message`].
pub fn collection_group_name_not_found_message(name: &str) -> String {
    format!("Collection group with name {name} is not found")
}

/// Hold an `IndexName` to the rule `CreateIndexRequest$indexName` documents: "Index names
/// must be lowercase and can't begin with underscores (_) or hyphens (-)".
///
/// `IndexName` carries no `@length` and no `@pattern`, so the boundary validator narrows
/// nothing here; an empty name is refused too, since an index has to be named to be
/// addressable at all. The message names the member and the constraint it failed, the way
/// the validation layer words one.
pub fn validate_index_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("indexName must not be empty".to_string());
    }
    if name.starts_with('_') || name.starts_with('-') {
        return Err(format!(
            "Value '{name}' at 'indexName' failed to satisfy constraint: Member must not begin \
             with an underscore (_) or a hyphen (-)"
        ));
    }
    if name.chars().any(char::is_uppercase) {
        return Err(format!(
            "Value '{name}' at 'indexName' failed to satisfy constraint: Member must be lowercase"
        ));
    }
    Ok(())
}

/// The first element a request member names more than once, if one does.
///
/// Several members carry a SET rather than a sequence — a value named twice describes no one
/// set — and the model states it in prose on each of them: a tag set is a map keyed by tag
/// key (`TagResourceRequest$tags` and `UntagResourceRequest$tagKeys` each document "All tag
/// keys in the request must be unique", and the create requests' `tags` is documented as "an
/// arbitrary SET of tags (key–value pairs)"), and an endpoint's subnets and security groups
/// are the ids it holds, which [`apply_id_change`] keeps as a set however many updates
/// change them. None of those list shapes carries `@uniqueItems`
/// (`smithy_query(query="shape", shape="Tags,TagKeys,SubnetIds,SecurityGroupIds")`), so the
/// boundary validator admits a repeat and the rule is enforced by the handler that takes the
/// member. Elements are compared as sent, since both a tag key and a resource id are
/// case-sensitive.
pub fn first_repeated_element<'elements>(
    elements: impl IntoIterator<Item = &'elements str>,
) -> Option<String> {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for element in elements {
        if !seen.insert(element) {
            return Some(element.to_string());
        }
    }
    None
}

/// How this service words a member that names one element of a SET twice: the clause the
/// boundary validator renders for a repeated element of a list, at the member that carried
/// it, so a repeat refused by a handler reads exactly like one refused at the boundary.
///
/// The sentence itself comes from the framework's own renderer rather than being spelled
/// here, so this refusal cannot drift from the ones `src/service.rs` words.
pub fn repeated_element_message(member: &str, element: &str) -> String {
    use ::aws_smithy_schema_server::constraints::{envelope, Violation, ViolationKind};

    let violation = Violation {
        path: member.to_string(),
        kind: ViolationKind::UniqueItems,
        value: Some(element.to_string()),
        sensitive: false,
    };
    // One violation always renders; the bare clause is the same sentence without the
    // envelope's count prefix.
    envelope(::std::slice::from_ref(&violation)).unwrap_or_else(|| violation.message())
}

/// The kind of resource a `resourceArn` addresses — the tag-bearing resources this slice
/// holds.
///
/// A resource ARN's last colon-separated field is `<resource kind>/<id>`, which is the
/// template each resource carries in the model (`collection/{id}` and
/// `collection-group/{id}`), so the kind and the id are read out of the ARN itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArnResourceKind {
    /// `…:collection/<CollectionId>`.
    Collection,
    /// `…:collection-group/<CollectionGroupId>`.
    CollectionGroup,
}

impl ArnResourceKind {
    /// Every kind an ARN may address, in the order [`parse_resource_arn`] tries them.
    const ALL: [Self; 2] = [Self::CollectionGroup, Self::Collection];

    /// How a refusal NAMES this kind, before the id of the resource it was looking for.
    pub fn label(self) -> &'static str {
        match self {
            Self::Collection => "Collection",
            Self::CollectionGroup => "Collection Group",
        }
    }

    /// The `resourceType` a quota refusal names for this kind, which is also the resource
    /// TYPE this kind's ARN template carries (`collection/{id}`, `collection-group/{id}`),
    /// so an ARN is read for this kind by that one token.
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::Collection => TAGGED_RESOURCE_TYPE,
            Self::CollectionGroup => "collection-group",
        }
    }
}

/// Whether `value` is an ARN at all — the only question the SYNTAX of a `resourceArn` asks.
///
/// Read through the workspace's ONE ARN parse, so this service cannot disagree with another
/// about what an ARN is: six colon-separated fields with the literal `arn` first and a
/// partition, a vendor and a resource that each name something, the Region and the account
/// empty for a partition-global or account-less resource. A value outside that form
/// addresses no resource of any service, so nothing can be looked up under it; which
/// SERVICE's resource an ARN names and whether that resource exists are separate questions,
/// answered by [`parse_resource_arn`] and the store.
pub fn is_arn(value: &str) -> bool {
    ::service_common::arn::parse(value).is_some()
}

/// The resource a `resourceArn` addresses: its kind and the id the ARN carries, or `None`
/// for a value whose resource half names neither of this service's addressable resources.
///
/// An ARN addresses a resource here only when its VENDOR field names THIS service
/// ([`ARN_NAMESPACE`]) and its resource field names one of the resource types this
/// service's resources declare in their ARN templates, with a non-empty id — both read
/// through the shared ARN parse, so an ARN of another service, whose resource half may read
/// like one of these, names nothing here.
pub fn parse_resource_arn(arn: &str) -> Option<(ArnResourceKind, &str)> {
    ::service_common::arn::parse_of(arn, ARN_NAMESPACE)?;
    ArnResourceKind::ALL.into_iter().find_map(|kind| {
        ::service_common::arn::resource_id_of_type(arn, kind.resource_type())
            .filter(|id| !id.is_empty())
            .map(|id| (kind, id))
    })
}

/// How this service words a RESOURCE ARN that names no resource this account holds: by the
/// KIND of resource the ARN addresses and the id it carries, and by the ARN itself when its
/// resource half names no addressable resource, since there is no id in it to name.
pub fn resource_not_found_message(arn: &str) -> String {
    match parse_resource_arn(arn) {
        Some((kind, id)) => format!("{} ID {id} is not found", kind.label()),
        None => format!("Resource {arn} is not found"),
    }
}

/// How this service words a `resourceArn` that is no ARN at all ([`is_arn`]): the value the
/// request carried, verbatim, called invalid.
pub fn invalid_resource_arn_message(value: &str) -> String {
    format!("Resource ARN: {value} is invalid")
}

/// Why the TAGGING plane cannot address the `resourceArn` a request carried.
///
/// The two conditions are not interchangeable: a value that is no ARN is malformed input —
/// the caller bug `ValidationException` describes, which no retry and no wait can turn into
/// a resource — while an ARN that names no live resource is the state condition
/// `ResourceNotFoundException` describes, which a client may poll or read as already
/// deleted. Each variant carries the sentence its refusal renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceArnRefusal {
    /// The value is no ARN; refused with `ValidationException`.
    Invalid(String),
    /// The ARN addresses no resource this account holds, or one whose deletion has been
    /// accepted; refused with `ResourceNotFoundException`.
    NotFound(String),
}

/// The customer managed key a policy statement assigns, if it names one. A statement that
/// selects the AWS-owned key (`AWSOwnedKey: true`) names no key ARN.
fn statement_kms_key(statement: &::aws_smithy_types::Document) -> Option<String> {
    use ::aws_smithy_types::Document;

    let Document::Object(members) = statement else {
        return None;
    };
    match members.get("KmsARN") {
        Some(Document::String(arn)) if !arn.is_empty() => Some(arn.clone()),
        _ => None,
    }
}

/// A `CollectionGroupErrorDetail` for an `ids` entry that names no stored collection
/// group.
pub fn collection_group_error_for_id(id: &str) -> CollectionGroupErrorDetail {
    CollectionGroupErrorDetail {
        id: Some(id.to_string()),
        name: None,
        error_message: Some(ERROR_MESSAGE_GROUP_NOT_FOUND.to_string()),
        error_code: Some(ERROR_CODE_NOT_FOUND.to_string()),
    }
}

/// A `CollectionGroupErrorDetail` for a `names` entry that names no stored collection
/// group.
pub fn collection_group_error_for_name(name: &str) -> CollectionGroupErrorDetail {
    CollectionGroupErrorDetail {
        id: None,
        name: Some(name.to_string()),
        error_message: Some(ERROR_MESSAGE_GROUP_NOT_FOUND.to_string()),
        error_code: Some(ERROR_CODE_NOT_FOUND.to_string()),
    }
}

/// A `VpcEndpointErrorDetail` for an `ids` entry that names no stored VPC endpoint.
pub fn vpc_endpoint_error_for_id(id: &str) -> VpcEndpointErrorDetail {
    VpcEndpointErrorDetail {
        id: Some(id.to_string()),
        error_message: Some(ERROR_MESSAGE_VPC_ENDPOINT_NOT_FOUND.to_string()),
        error_code: Some(ERROR_CODE_NOT_FOUND.to_string()),
    }
}

/// Why a batch read's `ids`/`names` pair selects nothing servable, if it does not select
/// exactly one of the two.
///
/// Both of this service's id-or-name batch readers take the same pair of selectors, and
/// `ids` and `names` carry the same sentence on BOTH request shapes — "You can't provide
/// names and IDs in the same request"
/// (`smithy_query(query="doc", shape="BatchGetCollectionRequest", member="ids")` and
/// `smithy_query(query="doc", shape="BatchGetCollectionGroupRequest", member="ids")`) — and
/// a request carrying neither member selects no resource at all, so neither combination is
/// servable. The branch and both sentences live here rather than in each reader, so one
/// shape of request cannot be answered two ways depending on which resource was read.
pub fn batch_selector_refusal(
    ids: Option<&[String]>,
    names: Option<&[String]>,
) -> Option<&'static str> {
    match (ids, names) {
        (Some(_), Some(_)) => Some("ids and names cannot be provided in the same request"),
        (None, None) => Some("one of ids or names must be provided"),
        _ => None,
    }
}

/// A refusal worded as this service's validation layer words one: the
/// `<n> validation error detected: Value '<value>' at '<member>' failed to satisfy
/// constraint: <constraint>` sentence the framework renders for a modelled constraint
/// failure, for the single violation a handler refuses. One wording, so a refusal a handler
/// makes reads exactly as the identical one made at the boundary does.
pub fn validation_error_message(member: &str, value: &str, constraint: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: {constraint}"
    )
}

/// The value half of a LIST member's refusal: how many items the member carried, which is
/// what the validation layer reports for a list.
///
/// A list's length violation names the count and not the entries — `Value '0 items' at
/// 'names'` — so a handler refusing a list for its length reads exactly as the identical
/// refusal made at the boundary does ([`validation_error_message`]). It also keeps the
/// refusal's size independent of the request's: a list member the model puts no maximum on
/// would otherwise be echoed back in full, letting one request choose how long the response
/// body is.
pub fn rendered_item_count(entries: &[String]) -> String {
    format!("{} items", entries.len())
}

/// The constraint clause for a list longer than `limit`, as the boundary validator words a
/// member over its own maximum length.
pub fn max_length_constraint(limit: usize) -> String {
    format!("Member must have length less than or equal to {limit}")
}

/// The first id named by BOTH an update's add list and its remove list, if any.
///
/// A request that asks for the same member to be added and removed describes no one
/// resulting set, so the operation refuses it rather than picking an order; see
/// [`apply_id_change`], which is what applies a request that names no such id.
///
/// The remove list is indexed into a set first, so the comparison is linear in the two
/// lists rather than in their product.
pub fn id_named_in_both<'a>(
    add: Option<&'a [String]>,
    remove: Option<&[String]>,
) -> Option<&'a str> {
    let (add, remove) = (add?, remove?);
    let removed: HashSet<&str> = remove.iter().map(String::as_str).collect();
    add.iter()
        .find(|id| removed.contains(id.as_str()))
        .map(String::as_str)
}

/// Apply an update's add and remove lists to one of an endpoint's id lists.
///
/// The list is a SET of ids the endpoint carries: an id it already holds is not held twice
/// when it is added again, and one it does not hold is unaffected by a removal, so the same
/// update applied twice leaves the same list. The ids the request keeps stay in the order
/// the endpoint already held them and the ones it adds follow, so what a reader reports is
/// stable across updates. Removals are applied before additions, which is why
/// [`id_named_in_both`] refuses a request naming one id in both lists instead of letting
/// that order decide.
///
/// Each membership test goes through a set, so the work is linear in the lists rather than
/// in their product however long the caller's are.
fn apply_id_change(current: &mut Vec<String>, add: Option<&[String]>, remove: Option<&[String]>) {
    if let Some(remove) = remove {
        let removed: HashSet<&str> = remove.iter().map(String::as_str).collect();
        current.retain(|id| !removed.contains(id.as_str()));
    }
    let Some(add) = add else {
        return;
    };
    let mut held: HashSet<String> = current.iter().cloned().collect();
    for id in add {
        if held.insert(id.clone()) {
            current.push(id.clone());
        }
    }
}

/// The list an update would leave, without touching the stored one: [`apply_id_change`] on
/// a copy, so a handler can hold the RESULT to the model's bounds before it writes anything.
pub fn changed_ids(
    current: &[String],
    add: Option<&[String]>,
    remove: Option<&[String]>,
) -> Vec<String> {
    let mut changed = current.to_vec();
    apply_id_change(&mut changed, add, remove);
    changed
}

/// One stored OpenSearch Serverless-managed interface VPC endpoint. Every field a reader
/// reports is held here; nothing is recomputed from a request.
#[derive(Debug, Clone)]
pub struct VpcEndpointRecord {
    /// The server-minted `VpcEndpointId`, which is also this record's key.
    pub id: String,
    pub name: String,
    /// The `VpcId` the create request named.
    pub vpc_id: String,
    /// The subnets the endpoint reaches OpenSearch Serverless through, in the order the
    /// request listed them.
    pub subnet_ids: Vec<String>,
    /// The security groups the endpoint's inbound traffic is authorized by. Empty for an
    /// endpoint whose create request named none, which the model allows
    /// (`securityGroupIds` is not `@required`), and reported as absent rather than as an
    /// empty list — `SecurityGroupIds` is `@length(min: 1)`.
    pub security_group_ids: Vec<String>,
    /// The `VpcEndpointStatus` wire value, settled through [`PendingStatus`].
    pub status: String,
    pub created_date: i64,
    /// When this endpoint was last modified, which is its creation until an update moves
    /// it. `UpdateVpcEndpointDetail$lastModifiedDate` is the one member that reports it;
    /// held exactly as [`CollectionRecord::last_modified_date`] is.
    pub last_modified_date: i64,
    /// The transition the clock owes this endpoint, while one is outstanding.
    pub pending_status: Option<PendingStatus>,
    /// When this endpoint's accepted deletion is due to be PURGED — the instant from which
    /// it stops being an endpoint any read reports at all. Set by [`Self::begin_deletion`],
    /// which is also what moves it to `DELETING`; held exactly as
    /// [`CollectionRecord::purge_at`] is.
    pub purge_at: Option<Instant>,
}

impl VpcEndpointRecord {
    /// Report `transient` from now, and owe `settled` from [`SETTLE_AFTER`] onwards.
    pub fn begin_transition(&mut self, transient: &str, settled: &'static str) {
        self.status = transient.to_string();
        self.pending_status = Some(PendingStatus {
            due_at: Instant::now() + SETTLE_AFTER,
            status: settled,
        });
    }

    /// Materialise the transition the clock has already made due, if any — the one write a
    /// reporting operation may make; see [`CollectionRecord::settle`].
    pub fn settle(&mut self, now: Instant) {
        if let Some(pending) = self.pending_status.take_if(|pending| now >= pending.due_at) {
            self.status = pending.status.to_string();
        }
    }

    /// Accept this endpoint's deletion: report it `DELETING` from now, and owe its PURGE
    /// from [`SETTLE_AFTER`] onwards.
    ///
    /// `DeleteVpcEndpoint` answers `deleteVpcEndpointDetail` with that status — the
    /// "current status of the endpoint deletion process" its own member documents — and the
    /// endpoint stays readable in it until the purge, so a caller polling the deletion
    /// observes it rather than seeing the resource vanish in one step. Mirrors
    /// [`CollectionRecord::begin_deletion`].
    pub fn begin_deletion(&mut self) {
        self.status = VPC_ENDPOINT_DELETING.to_string();
        // A delete supersedes whatever transition was outstanding: the only thing still
        // owed on this endpoint is the purge.
        self.pending_status = None;
        self.purge_at = Some(Instant::now() + SETTLE_AFTER);
    }

    /// Whether the clock has already made this endpoint's purge due at `now`.
    pub fn is_purge_due(&self, now: Instant) -> bool {
        self.purge_at.is_some_and(|due_at| now >= due_at)
    }

    /// Accept an UPDATE of this endpoint: report it `PENDING` from now, settling back to
    /// `ACTIVE` once the clock makes that due, and record the moment it was modified.
    ///
    /// `VpcEndpointStatus` models no `UPDATING` — `PENDING`, `ACTIVE`, `DELETING` and
    /// `FAILED` are all it declares — so `PENDING` is the non-terminal status the change is
    /// applied in, which is what `UpdateVpcEndpointDetail$status` ("the current status of
    /// the endpoint update process") reports. Mirrors [`CollectionRecord::begin_update`].
    pub fn begin_update(&mut self, updated_at: i64) {
        self.begin_transition(VPC_ENDPOINT_PENDING, VPC_ENDPOINT_ACTIVE);
        self.last_modified_date = updated_at;
    }

    /// The `UpdateVpcEndpointDetail` body for this record — the one place this crate renders
    /// that shape, so an accepted update cannot report an endpoint differently from the
    /// reads beside it.
    pub fn update_detail(&self) -> UpdateVpcEndpointDetail {
        UpdateVpcEndpointDetail {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            subnet_ids: Some(self.subnet_ids.clone()),
            // Absent rather than empty for an endpoint carrying no security groups, for the
            // reason [`Self::detail`] reports it that way: `SecurityGroupIds` is
            // `@length(min: 1)` on the wire.
            security_group_ids: (!self.security_group_ids.is_empty())
                .then(|| self.security_group_ids.clone()),
            last_modified_date: Some(self.last_modified_date),
        }
    }

    /// The `DeleteVpcEndpointDetail` body for this record — the one place this crate renders
    /// that shape, so an accepted deletion cannot report an endpoint differently from the
    /// reads beside it.
    pub fn delete_detail(&self) -> DeleteVpcEndpointDetail {
        DeleteVpcEndpointDetail {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
        }
    }

    /// The `CreateVpcEndpointDetail` body for this record — the one place this crate
    /// renders that shape.
    pub fn create_detail(&self) -> CreateVpcEndpointDetail {
        CreateVpcEndpointDetail {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
        }
    }

    /// The `VpcEndpointDetail` body for this record — the one place this crate renders
    /// that shape, so `BatchGetVpcEndpoint` cannot report an endpoint differently from the
    /// create that stored it.
    pub fn detail(&self) -> VpcEndpointDetail {
        VpcEndpointDetail {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            vpc_id: Some(self.vpc_id.clone()),
            subnet_ids: Some(self.subnet_ids.clone()),
            security_group_ids: (!self.security_group_ids.is_empty())
                .then(|| self.security_group_ids.clone()),
            status: Some(self.status.clone()),
            created_date: Some(self.created_date),
            // `failureCode` / `failureMessage` describe an endpoint whose create or delete
            // failed. Nothing in this slice fails an endpoint, so a stored record carries
            // neither.
            failure_code: None,
            failure_message: None,
        }
    }

    /// The `VpcEndpointSummary` body for this record — the shape `ListVpcEndpoints` pages
    /// over, so the listing reports the same identity and status the detail readers do.
    pub fn summary(&self) -> VpcEndpointSummary {
        VpcEndpointSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-opensearchserverless-opensearchserverless. A handler reaches it through [`service_state`].
///
/// Collections are keyed by the server-minted id, which is their identity in this API
/// (`BatchGetCollection`/`DeleteCollection` address them by it), with a name index beside
/// them so a name lookup and the duplicate-name check are one map probe. Both maps are
/// `BTreeMap`s so `ListCollections` walks one deterministic order a page token can resume.
/// Collection groups are held the same way, and a collection names the group it belongs to
/// by NAME (`CreateCollectionRequest$collectionGroupName`), so a group's
/// `numberOfCollections` is counted over the collections map by that name.
#[derive(Debug, Default)]
pub struct ComAmazonawsOpensearchserverlessOpensearchserverlessState {
    /// Collections by `CollectionId`.
    pub collections: BTreeMap<String, CollectionRecord>,
    /// `CollectionName` -> `CollectionId`, for name-addressed reads and the
    /// duplicate-name conflict check.
    pub collection_name_index: BTreeMap<String, String>,
    /// Collection groups by `CollectionGroupId`, which is their identity in this API
    /// (`BatchGetCollectionGroup`/`UpdateCollectionGroup`/`DeleteCollectionGroup` address
    /// them by it).
    pub collection_groups: BTreeMap<String, CollectionGroupRecord>,
    /// `CollectionGroupName` -> `CollectionGroupId`, for the name-addressed reads
    /// `BatchGetCollectionGroup` serves and the duplicate-name conflict check.
    pub collection_group_name_index: BTreeMap<String, String>,
    /// Security policies by `(SecurityPolicyType, PolicyName)`: the pair that identifies
    /// one, since the same name may be used once per type.
    pub security_policies: BTreeMap<(String, String), SecurityPolicyRecord>,
    /// Security configurations by [`security_config_id`], which is the identity this API
    /// addresses them by (`GetSecurityConfig`/`DeleteSecurityConfig` take `id`). The id
    /// starts with the type and ends with the name, so this one `BTreeMap` already walks
    /// each type's configurations in name order — the stable order
    /// `ListSecurityConfigs`' page token resumes inside.
    pub security_configs: BTreeMap<String, SecurityConfigRecord>,
    /// Data access policies by `(AccessPolicyType, PolicyName)`, the pair that identifies
    /// one, exactly as `security_policies` is keyed.
    pub access_policies: BTreeMap<(String, String), AccessPolicyRecord>,
    /// Lifecycle policies by `(LifecyclePolicyType, PolicyName)`, the pair
    /// `BatchGetLifecyclePolicy`'s own `LifecyclePolicyIdentifier` addresses one by — and
    /// a `BTreeMap`, so `ListLifecyclePolicies` walks one deterministic order a page token
    /// can resume.
    pub lifecycle_policies: BTreeMap<(String, String), LifecyclePolicyRecord>,
    /// `<operation>:<clientToken>` -> the key of the record that token's first call
    /// created, so a replayed `clientToken` on an `@idempotent` create answers the
    /// original record instead of creating a second one.
    ///
    /// PRIVATE, with `idempotency_token_order` beside it, because the two carry one
    /// invariant [`MAX_IDEMPOTENCY_TOKENS`] depends on: they are written only together,
    /// by [`Self::remember_token`] and [`Self::forget_tokens_for`], so the map can never
    /// grow past the cap and no token outlives the record it names.
    idempotency_tokens: BTreeMap<String, String>,
    /// The order [`Self::idempotency_tokens`] was written in, so the cap evicts the
    /// oldest token first.
    idempotency_token_order: VecDeque<String>,
    /// The account's own OCU capacity settings: `GetAccountSettings` reports them and
    /// `UpdateAccountSettings` writes them. Defaulted rather than optional, because an
    /// account always has limits — see [`AccountSettingsRecord`].
    pub account_settings: AccountSettingsRecord,
    /// Managed interface VPC endpoints by `VpcEndpointId`, which is their identity in this
    /// API (`BatchGetVpcEndpoint`/`UpdateVpcEndpoint`/`DeleteVpcEndpoint` address them by
    /// it) — a `BTreeMap`, so `ListVpcEndpoints` walks one deterministic order a page token
    /// can resume.
    pub vpc_endpoints: BTreeMap<String, VpcEndpointRecord>,
    /// `VpcEndpointName` -> `VpcEndpointId`, for the duplicate-name conflict check, held
    /// beside the records exactly as `collection_name_index` is.
    pub vpc_endpoint_name_index: BTreeMap<String, String>,
    /// The data-plane listener each collection is PUBLISHED on, by `CollectionId` — the
    /// address its `collectionEndpoint` names ([`collection_plane`]).
    ///
    /// Held beside the collections rather than on the record, because a live socket is
    /// neither `Clone` nor `Debug` and a record is both. Dropping an entry closes that
    /// collection's port, so [`Self::remove_collection`] is what stops a purged
    /// collection's address answering.
    pub collection_planes: BTreeMap<String, CollectionPlane>,
}

impl ComAmazonawsOpensearchserverlessOpensearchserverlessState {
    /// Apply every transition the clock has already made due, for a collection and for a
    /// managed interface endpoint alike: a status the record settles into, and the PURGE of
    /// one whose deletion was accepted. Call it before reading or writing either, so no
    /// caller observes a status the service has already moved on from. See
    /// [`CollectionRecord::settle`], [`CollectionRecord::begin_deletion`],
    /// [`VpcEndpointRecord::settle`] and [`VpcEndpointRecord::begin_deletion`].
    pub fn settle_due_transitions(&mut self) {
        let now = Instant::now();
        for record in self.collections.values_mut() {
            record.settle(now);
        }
        for record in self.vpc_endpoints.values_mut() {
            record.settle(now);
        }
        // The purge was owed at `now` whether or not anybody called, so applying it here
        // is what a later caller sees rather than a change this read made.
        let purged: Vec<String> = self
            .collections
            .values()
            .filter(|record| record.is_purge_due(now))
            .map(|record| record.id.clone())
            .collect();
        for id in purged {
            self.remove_collection(&id);
        }
        let purged: Vec<String> = self
            .vpc_endpoints
            .values()
            .filter(|record| record.is_purge_due(now))
            .map(|record| record.id.clone())
            .collect();
        for id in purged {
            self.remove_vpc_endpoint(&id);
        }
    }

    /// The collection whose ARN is `arn`, if one is stored under it.
    ///
    /// Matched on the STORED ARN, so a resource ARN naming another account or another
    /// Region resolves to nothing rather than to this account's collection of that id.
    pub fn collection_by_arn(&self, arn: &str) -> Option<&CollectionRecord> {
        self.collections.values().find(|record| record.arn == arn)
    }

    /// [`Self::collection_by_arn`], mutably.
    pub fn collection_by_arn_mut(&mut self, arn: &str) -> Option<&mut CollectionRecord> {
        self.collections
            .values_mut()
            .find(|record| record.arn == arn)
    }

    /// The encryption setting a stored `encryption` security policy assigns to the
    /// collection `name`, or `None` when no stored policy covers it.
    ///
    /// An encryption policy's document carries `Rules`, each a `ResourceType` plus the
    /// `Resource` patterns it applies to, beside the key that policy assigns
    /// (`AWSOwnedKey: true`, or a `KmsARN`) — AWS's "Encryption policies for Amazon
    /// OpenSearch Serverless" documents that shape, that a collection is covered when a
    /// rule of `ResourceType` `collection` names `collection/<name>` (optionally with a
    /// trailing wildcard), and that such a policy is what encrypts a collection whose
    /// create request carries no `encryptionConfig`.
    pub fn policy_encryption_for(&self, name: &str) -> Option<PolicyEncryption> {
        let resource = format!("{POLICY_COLLECTION_RESOURCE_TYPE}/{name}");
        self.security_policies
            .iter()
            .filter(|((kind, _), _)| kind == SECURITY_POLICY_TYPE_ENCRYPTION)
            .find_map(|(_, record)| {
                let statement = covering_statement(&record.policy, &resource)?;
                Some(PolicyEncryption {
                    kms_key_arn: statement_kms_key(statement),
                })
            })
    }

    /// The collection group whose ARN is `arn`, if one is stored under it. Matched on the
    /// STORED ARN, exactly as [`Self::collection_by_arn`] is.
    pub fn collection_group_by_arn(&self, arn: &str) -> Option<&CollectionGroupRecord> {
        self.collection_groups
            .values()
            .find(|record| record.arn == arn)
    }

    /// [`Self::collection_group_by_arn`], mutably.
    pub fn collection_group_by_arn_mut(&mut self, arn: &str) -> Option<&mut CollectionGroupRecord> {
        self.collection_groups
            .values_mut()
            .find(|record| record.arn == arn)
    }

    /// The tags the resource `arn` addresses carries, whichever KIND of resource that is,
    /// or `None` when this account holds no resource under that ARN.
    ///
    /// The tagging plane takes a `resourceArn` — "the ARN of the resource", not of a
    /// collection — so every resource this slice stores tags on is resolved here, by the
    /// kind the ARN names ([`parse_resource_arn`]). One resolver, so a tag write and a tag
    /// read can never resolve the same ARN differently.
    pub fn tags_by_arn(&self, arn: &str) -> Option<&Vec<Tag>> {
        match parse_resource_arn(arn)? {
            (ArnResourceKind::Collection, _) => {
                self.collection_by_arn(arn).map(|record| &record.tags)
            }
            (ArnResourceKind::CollectionGroup, _) => self
                .collection_group_by_arn(arn)
                .map(|record| &record.tags),
        }
    }

    /// [`Self::tags_by_arn`], mutably — what a tag write stores through, with the
    /// record's `lastModifiedDate` stamped `written_at` by the same call.
    ///
    /// A resource's tags are part of the resource, so storing a new tag set MODIFIES it:
    /// `lastModifiedDate` is "the date and time when the resource was last modified", and
    /// it has to answer that for an accepted tag write as much as for a configuration
    /// write — [`CollectionRecord::begin_update`], [`CollectionGroupRecord::apply_update`],
    /// [`PolicyWrite::apply`] and [`SecurityConfigRecord::apply`] all stamp the write they
    /// apply. The tags are reachable for writing ONLY through this stamping accessor, so a
    /// tag write cannot leave the field behind the record it changed, whichever KIND of
    /// resource the ARN addresses.
    pub fn tags_by_arn_for_write(&mut self, arn: &str, written_at: i64) -> Option<&mut Vec<Tag>> {
        match parse_resource_arn(arn)? {
            (ArnResourceKind::Collection, _) => self.collection_by_arn_mut(arn).map(|record| {
                record.last_modified_date = written_at;
                &mut record.tags
            }),
            (ArnResourceKind::CollectionGroup, _) => {
                self.collection_group_by_arn_mut(arn).map(|record| {
                    record.last_modified_date = written_at;
                    &mut record.tags
                })
            }
        }
    }

    /// Whether the TAGGING plane's `resourceArn` addresses a resource at all: one this
    /// account holds that is not on its way out.
    ///
    /// Every operation that takes a `resourceArn` documents the same precondition for it —
    /// the resource "must be active (not in the `DELETING` state), and must be owned by the
    /// account ID included in the request" (the member's own documentation on
    /// `TagResourceRequest`, `UntagResourceRequest` and `ListTagsForResourceRequest`) — so
    /// this is what the tag reads and the tag writes alike resolve an ARN through, and a
    /// collection whose deletion has been accepted is addressable by none of them. A
    /// collection group has no accepted-deletion state of its own (its delete removes the
    /// record), so holding one is enough.
    pub fn is_tag_addressable(&self, arn: &str) -> bool {
        match parse_resource_arn(arn) {
            Some((ArnResourceKind::Collection, _)) => self
                .collection_by_arn(arn)
                .is_some_and(|record| !record.is_deleting()),
            Some((ArnResourceKind::CollectionGroup, _)) => {
                self.collection_group_by_arn(arn).is_some()
            }
            None => false,
        }
    }

    /// Whether the TAGGING plane's `resourceArn` addresses a resource at all, or why it
    /// addresses none ([`ResourceArnRefusal`]).
    ///
    /// The SYNTAX of the value is settled before any lookup: a value that is no ARN
    /// ([`is_arn`]) names nothing any lookup could resolve, so it is refused as malformed
    /// input, and only an ARN is looked up and reported as not found. Every tagging
    /// operation resolves its `resourceArn` here, so the tag write, the tag read and the
    /// tag removal cannot classify one value three ways.
    pub fn resolve_tag_resource_arn(&self, arn: &str) -> Result<(), ResourceArnRefusal> {
        if !is_arn(arn) {
            return Err(ResourceArnRefusal::Invalid(invalid_resource_arn_message(
                arn,
            )));
        }
        if self.is_tag_addressable(arn) {
            Ok(())
        } else {
            Err(ResourceArnRefusal::NotFound(resource_not_found_message(arn)))
        }
    }

    /// The OCUs this account's collection GROUPS already account for, for one capacity
    /// type: the sum of each stored group's own ceiling.
    ///
    /// Counted over the stored groups rather than from a running total, so it is the
    /// capacity the groups really carry.
    pub fn collection_group_capacity_in_ocu(&self, kind: CapacityKind) -> f32 {
        // Folded from a positive zero rather than summed, so an account with no collection
        // group reports `0` and not the negative zero the standard float sum's identity
        // would render.
        self.collection_groups
            .values()
            .filter_map(|record| kind.group_ceiling(&record.capacity_limits))
            .fold(0.0, |total, ceiling| total + ceiling)
    }

    /// Hold an `UpdateAccountSettings` request to the account's non-adjustable OCU quota,
    /// answering the `ServiceQuotaExceededException` message for the first capacity type it
    /// takes past [`MAX_ACCOUNT_CAPACITY_IN_OCU`].
    ///
    /// A ceiling this account may not reach is not a setting the service stores: the quota
    /// bounds the TOTAL for a capacity type — what the request asks for, for collections
    /// outside any group, plus what the account's collection groups already commit — so the
    /// figures the refusal names are the stored settings, the groups' committed capacity and
    /// that total. `None` for a request every capacity type's quota admits.
    pub fn account_capacity_quota_refusal(&self, requested: &CapacityLimits) -> Option<String> {
        CapacityKind::ALL.into_iter().find_map(|kind| {
            let asked = kind.requested(requested)?;
            let committed = self.collection_group_capacity_in_ocu(kind);
            let total = f64::from(asked) + f64::from(committed);
            if total <= f64::from(MAX_ACCOUNT_CAPACITY_IN_OCU) {
                return None;
            }
            let word = kind.word();
            Some(format!(
                "The requested total {word} capacity of {} OCUs exceeds your maximum {word} \
                 capacity service quota limit of {MAX_ACCOUNT_CAPACITY_IN_OCU} OCUs. Current \
                 settings: {} OCUs for collections that are not part of any collection group, \
                 {committed} OCUs for collection groups. New request of {asked} OCUs would bring \
                 total to {} OCUs.",
                rendered_ocu_total(f64::from(asked)),
                self.account_settings.limit(kind),
                rendered_ocu_total(total),
            ))
        })
    }

    /// Mint a fresh `CollectionId`: [`ID_LEN`] lowercase alphanumeric characters, the
    /// shape `^[a-z0-9]{3,40}$` admits.
    ///
    /// Every character is DRAWN (see [`random_bytes`]), so the id carries no structure a
    /// caller can read — no counter, no clock, no filler — and two collections, in this
    /// store or in another process, never carry the same one.
    pub fn mint_collection_id(&self) -> String {
        self.mint_id()
    }

    /// Mint a fresh `CollectionGroupId`. `CollectionGroupId` carries the same
    /// `@length(min: 3, max: 40)` and `^[a-z0-9]{3,40}$` as `CollectionId`, so it is minted
    /// the same way and against the same stored ids; see [`Self::mint_collection_id`].
    pub fn mint_collection_group_id(&self) -> String {
        self.mint_id()
    }

    /// A drawn id no live resource of this service already carries, shared by every
    /// resource whose id shape is `^[a-z0-9]{3,40}$`.
    ///
    /// A draw is answered as soon as [`Self::id_in_use`] says it is free; one already in
    /// use is re-drawn, [`ID_MINT_ATTEMPTS`] draws in all. The last of them is answered
    /// unchecked, which takes every one of those draws colliding — a run no store reaches
    /// against [`ID_SPACE`].
    fn mint_id(&self) -> String {
        let mut drawn = render_id(draw_id_value());
        for _ in 1..ID_MINT_ATTEMPTS {
            if !self.id_in_use(&drawn) {
                return drawn;
            }
            drawn = render_id(draw_id_value());
        }
        drawn
    }

    /// Whether a live collection or collection group is stored under `id`. Both resources
    /// are checked, because both are addressed by an id of the one shape and a client maps
    /// them by that id.
    fn id_in_use(&self, id: &str) -> bool {
        self.collections.contains_key(id) || self.collection_groups.contains_key(id)
    }

    /// The collection `name` resolves to, if one is stored under it.
    pub fn collection_by_name(&self, name: &str) -> Option<&CollectionRecord> {
        let id = self.collection_name_index.get(name)?;
        self.collections.get(id)
    }

    /// Store `record` under its id, indexing its name.
    pub fn insert_collection(&mut self, record: CollectionRecord) {
        self.collection_name_index
            .insert(record.name.clone(), record.id.clone());
        self.collections.insert(record.id.clone(), record);
    }

    /// The collection group `name` resolves to, if one is stored under it.
    pub fn collection_group_by_name(&self, name: &str) -> Option<&CollectionGroupRecord> {
        let id = self.collection_group_name_index.get(name)?;
        self.collection_groups.get(id)
    }

    /// Store `record` under its id, indexing its name.
    pub fn insert_collection_group(&mut self, record: CollectionGroupRecord) {
        self.collection_group_name_index
            .insert(record.name.clone(), record.id.clone());
        self.collection_groups.insert(record.id.clone(), record);
    }

    /// How many stored collections name the collection group `group_name` — the
    /// `numberOfCollections` a collection-group read reports, counted from the collections
    /// themselves rather than from a counter a create bumped.
    pub fn collections_in_group(&self, group_name: &str) -> usize {
        self.collections
            .values()
            .filter(|record| record.collection_group_name.as_deref() == Some(group_name))
            .count()
    }

    /// Remove the collection `id` names, dropping its name index entry and every
    /// idempotency token that named it with it.
    pub fn remove_collection(&mut self, id: &str) -> Option<CollectionRecord> {
        let record = self.collections.remove(id)?;
        self.collection_name_index.remove(&record.name);
        self.forget_tokens_for(id);
        // Closes the port the collection was published on: an address this service
        // announced answers only while the collection behind it exists.
        self.collection_planes.remove(id);
        Some(record)
    }

    /// Remove the collection group `id` names, dropping its name index entry and every
    /// idempotency token that named it with it — so a later `BatchGetCollectionGroup` of
    /// that id or that name finds no live record and reports the `NOT_FOUND` error detail
    /// instead of a stale `CollectionGroupDetail`, and the freed name may be created
    /// again. Mirrors [`Self::remove_collection`].
    pub fn remove_collection_group(&mut self, id: &str) -> Option<CollectionGroupRecord> {
        let record = self.collection_groups.remove(id)?;
        self.collection_group_name_index.remove(&record.name);
        self.forget_tokens_for(id);
        Some(record)
    }

    /// Remove the security policy `(kind, name)` identifies, dropping every idempotency
    /// token that named it with it.
    pub fn remove_security_policy(
        &mut self,
        kind: &str,
        name: &str,
    ) -> Option<SecurityPolicyRecord> {
        let record = self
            .security_policies
            .remove(&(kind.to_string(), name.to_string()))?;
        self.forget_tokens_for(&policy_token_value(kind, name));
        Some(record)
    }

    /// Remove the data access policy `(kind, name)` identifies, dropping every idempotency
    /// token that named it with it — so a later read of that pair raises
    /// `ResourceNotFoundException`, exactly as [`Self::remove_security_policy`] does for a
    /// security policy.
    pub fn remove_access_policy(&mut self, kind: &str, name: &str) -> Option<AccessPolicyRecord> {
        let record = self
            .access_policies
            .remove(&(kind.to_string(), name.to_string()))?;
        self.forget_tokens_for(&policy_token_value(kind, name));
        Some(record)
    }

    /// Remove the lifecycle policy `(kind, name)` identifies, dropping every idempotency
    /// token that named it with it — so a later read of that pair reports the modelled
    /// not-found detail and the freed pair may be created again, exactly as
    /// [`Self::remove_access_policy`] does for a data access policy.
    pub fn remove_lifecycle_policy(
        &mut self,
        kind: &str,
        name: &str,
    ) -> Option<LifecyclePolicyRecord> {
        let record = self
            .lifecycle_policies
            .remove(&(kind.to_string(), name.to_string()))?;
        self.forget_tokens_for(&policy_token_value(kind, name));
        Some(record)
    }

    /// Remove the security configuration `id` names, dropping every idempotency token
    /// that named it with it.
    pub fn remove_security_config(&mut self, id: &str) -> Option<SecurityConfigRecord> {
        let record = self.security_configs.remove(id)?;
        self.forget_tokens_for(id);
        Some(record)
    }

    /// The record a replayed `clientToken` on `operation` already created, if any.
    ///
    /// `None` both when the token is new and when the record it created has since been
    /// deleted — a replay cannot resurrect a deleted resource, so a stale token simply
    /// falls through to the operation's normal path.
    ///
    /// The handler SCOPES what this answers to the resource its request addresses: a token
    /// spent again on another resource is a request for that other resource, which is still
    /// served on its own terms rather than answered with the record the token first
    /// created. Every caller in `src/ops/` therefore filters on the addressed identity.
    pub fn replayed_collection(&self, operation: &str, token: &str) -> Option<&CollectionRecord> {
        let id = self
            .idempotency_tokens
            .get(&idempotency_key(operation, token))?;
        self.collections.get(id)
    }

    /// The collection group a replayed `clientToken` on `operation` already created, if
    /// any; see [`Self::replayed_collection`] for why a stale token answers `None`.
    pub fn replayed_collection_group(
        &self,
        operation: &str,
        token: &str,
    ) -> Option<&CollectionGroupRecord> {
        let id = self
            .idempotency_tokens
            .get(&idempotency_key(operation, token))?;
        self.collection_groups.get(id)
    }

    /// The security policy a replayed `clientToken` on `operation` already created, if
    /// any; see [`Self::replayed_collection`] for why a stale token answers `None`.
    pub fn replayed_security_policy(
        &self,
        operation: &str,
        token: &str,
    ) -> Option<&SecurityPolicyRecord> {
        let key = self
            .idempotency_tokens
            .get(&idempotency_key(operation, token))?;
        let (kind, name) = key.split_once('/')?;
        self.security_policies
            .get(&(kind.to_string(), name.to_string()))
    }

    /// The data access policy a replayed `clientToken` on `operation` already created, if
    /// any; see [`Self::replayed_collection`] for why a stale token answers `None`.
    pub fn replayed_access_policy(
        &self,
        operation: &str,
        token: &str,
    ) -> Option<&AccessPolicyRecord> {
        let key = self
            .idempotency_tokens
            .get(&idempotency_key(operation, token))?;
        let (kind, name) = key.split_once('/')?;
        self.access_policies
            .get(&(kind.to_string(), name.to_string()))
    }

    /// Remember that `operation` with `token` created the record keyed `key`, evicting
    /// the oldest token first if that would push the map past
    /// [`MAX_IDEMPOTENCY_TOKENS`].
    ///
    /// FIRST write wins: a token names the request it was first spent on, so spending it
    /// again on another resource leaves the entry alone — the other resource is created on
    /// its own terms, and a later retry of the FIRST request still replays that one. The
    /// entry is dropped only with the record it names ([`Self::forget_tokens_for`]), after
    /// which the same token may name a new record.
    pub fn remember_token(&mut self, operation: &str, token: &str, key: &str) {
        let entry = idempotency_key(operation, token);
        if self.idempotency_tokens.contains_key(&entry) {
            return;
        }
        while self.idempotency_tokens.len() >= MAX_IDEMPOTENCY_TOKENS {
            match self.idempotency_token_order.pop_front() {
                Some(oldest) => {
                    self.idempotency_tokens.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the cap an
                // invariant even if they ever drift.
                None => {
                    self.idempotency_tokens.clear();
                    break;
                }
            }
        }
        if self
            .idempotency_tokens
            .insert(entry.clone(), key.to_string())
            .is_none()
        {
            self.idempotency_token_order.push_back(entry);
        }
    }

    /// Drop every idempotency token that named the record keyed `key`: a token never
    /// outlives the resource it created, so a create/delete loop cannot accumulate
    /// entries for resources that no longer exist.
    fn forget_tokens_for(&mut self, key: &str) {
        self.idempotency_tokens.retain(|_, created| created != key);
        let live = &self.idempotency_tokens;
        self.idempotency_token_order
            .retain(|token| live.contains_key(token));
    }

    /// How many `clientToken` replay entries are held; [`MAX_IDEMPOTENCY_TOKENS`] bounds
    /// it. Exposed so a test can pin the cap and the eviction.
    pub fn idempotency_token_count(&self) -> usize {
        self.idempotency_tokens.len()
    }

    /// The number of stored security policies of EVERY `SecurityPolicyType`, which is the
    /// account's own security-policy holding — what the account-wide total counts them as.
    pub fn security_policy_count(&self) -> usize {
        self.security_policies.len()
    }

    /// The number of stored security policies of `kind`, which its quota bounds and which
    /// is the per-type counter `SecurityPolicyStats` reports.
    pub fn security_policy_count_of(&self, kind: &str) -> usize {
        self.security_policies
            .keys()
            .filter(|(policy_kind, _)| policy_kind == kind)
            .count()
    }

    /// The security configuration a replayed `clientToken` on `operation` already created,
    /// if any; see [`Self::replayed_collection`] for why a stale token answers `None`.
    pub fn replayed_security_config(
        &self,
        operation: &str,
        token: &str,
    ) -> Option<&SecurityConfigRecord> {
        let id = self
            .idempotency_tokens
            .get(&idempotency_key(operation, token))?;
        self.security_configs.get(id)
    }

    /// The number of stored security configurations of EVERY `SecurityConfigType`, which is
    /// what `SecurityConfigStats`' single counter is documented to report ("the number of
    /// security configurations in the current account") and what the account-wide total
    /// counts them as.
    pub fn security_config_count(&self) -> usize {
        self.security_configs.len()
    }

    /// The number of stored security configurations of `kind`, which its quota bounds.
    pub fn security_config_count_of(&self, kind: &str) -> usize {
        self.security_configs
            .values()
            .filter(|record| record.kind == kind)
            .count()
    }

    /// The number of stored data access policies, which [`MAX_ACCESS_POLICIES`] bounds.
    pub fn access_policy_count(&self) -> usize {
        self.access_policies.len()
    }

    /// The number of stored access policies of `kind`, which is the per-type counter
    /// `AccessPolicyStats` reports. `AccessPolicyType` carries the single value `data`, so
    /// it equals [`Self::access_policy_count`] — the account-wide total the quota bounds —
    /// for [`ACCESS_POLICY_DATA`] today, and stays per-type if the enum ever grows.
    pub fn access_policy_count_of(&self, kind: &str) -> usize {
        self.access_policies
            .keys()
            .filter(|(policy_kind, _)| policy_kind == kind)
            .count()
    }

    /// The lifecycle policy a replayed `clientToken` on `operation` already created, if
    /// any; see [`Self::replayed_collection`] for why a stale token answers `None`.
    pub fn replayed_lifecycle_policy(
        &self,
        operation: &str,
        token: &str,
    ) -> Option<&LifecyclePolicyRecord> {
        let key = self
            .idempotency_tokens
            .get(&idempotency_key(operation, token))?;
        let (kind, name) = key.split_once('/')?;
        self.lifecycle_policies
            .get(&(kind.to_string(), name.to_string()))
    }

    /// The number of stored lifecycle policies, which [`MAX_LIFECYCLE_POLICIES`] bounds.
    pub fn lifecycle_policy_count(&self) -> usize {
        self.lifecycle_policies.len()
    }

    /// The number of stored lifecycle policies of `kind`, which is the per-type counter
    /// `LifecyclePolicyStats` reports. `LifecyclePolicyType` carries the single value
    /// `retention`, so it equals [`Self::lifecycle_policy_count`] — the account-wide total
    /// the quota bounds — for [`LIFECYCLE_POLICY_RETENTION`] today, and stays per-type if
    /// the enum ever grows.
    pub fn lifecycle_policy_count_of(&self, kind: &str) -> usize {
        self.lifecycle_policies
            .keys()
            .filter(|(policy_kind, _)| policy_kind == kind)
            .count()
    }

    /// Mint a fresh `VpcEndpointId` no stored endpoint already carries.
    ///
    /// Drawn like every other identifier this store mints ([`draw_vpc_endpoint_id`]), and
    /// re-drawn while the draw is already in use — [`ID_MINT_ATTEMPTS`] draws in all, the
    /// last of them answered unchecked, exactly as [`Self::mint_id`] does.
    pub fn mint_vpc_endpoint_id(&self) -> String {
        let mut drawn = draw_vpc_endpoint_id();
        for _ in 1..ID_MINT_ATTEMPTS {
            if !self.vpc_endpoints.contains_key(&drawn) {
                return drawn;
            }
            drawn = draw_vpc_endpoint_id();
        }
        drawn
    }

    /// The VPC endpoint `name` resolves to, if one is stored under it.
    pub fn vpc_endpoint_by_name(&self, name: &str) -> Option<&VpcEndpointRecord> {
        let id = self.vpc_endpoint_name_index.get(name)?;
        self.vpc_endpoints.get(id)
    }

    /// Store `record` under its id, indexing its name.
    pub fn insert_vpc_endpoint(&mut self, record: VpcEndpointRecord) {
        self.vpc_endpoint_name_index
            .insert(record.name.clone(), record.id.clone());
        self.vpc_endpoints.insert(record.id.clone(), record);
    }

    /// Remove the VPC endpoint `id` names, dropping its name index entry and every
    /// idempotency token that named it with it — so a later `BatchGetVpcEndpoint` of that id
    /// reports the `NOT_FOUND` error detail instead of a stale `VpcEndpointDetail`, and the
    /// freed name may be created again. Mirrors [`Self::remove_collection`].
    pub fn remove_vpc_endpoint(&mut self, id: &str) -> Option<VpcEndpointRecord> {
        let record = self.vpc_endpoints.remove(id)?;
        self.vpc_endpoint_name_index.remove(&record.name);
        self.forget_tokens_for(id);
        Some(record)
    }

    /// The VPC endpoint a replayed `clientToken` on `operation` already created, if any;
    /// see [`Self::replayed_collection`] for why a stale token answers `None`.
    pub fn replayed_vpc_endpoint(
        &self,
        operation: &str,
        token: &str,
    ) -> Option<&VpcEndpointRecord> {
        let id = self
            .idempotency_tokens
            .get(&idempotency_key(operation, token))?;
        self.vpc_endpoints.get(id)
    }
}

/// The `idempotency_tokens` key for `token` replayed against `operation`. The operation
/// name is part of the key so one `clientToken` reused across two different creates is
/// two independent records, not one.
fn idempotency_key(operation: &str, token: &str) -> String {
    format!("{operation}:{token}")
}

/// The `idempotency_tokens` VALUE that names one stored policy by its `(type, name)`
/// pair. Every policy type value (`encryption`, `network`, `data`) and every `PolicyName`
/// (`^[a-z][a-z0-9-]+$`) is free of the separator, so the pair round-trips unambiguously;
/// the map is keyed by operation name as well, so a token is only ever read back by the
/// create that wrote it.
pub fn policy_token_value(kind: &str, name: &str) -> String {
    format!("{kind}/{name}")
}

/// A record kind a listing pages over.
///
/// The cursor is the key the listing's order is BY, which is also what a page token
/// resumes after — so one paging implementation serves every listing in this service
/// rather than one per resource.
pub trait Paged {
    /// This record's place in its listing's order.
    fn page_cursor(&self) -> &str;
}

impl Paged for CollectionRecord {
    fn page_cursor(&self) -> &str {
        &self.id
    }
}

impl Paged for CollectionGroupRecord {
    fn page_cursor(&self) -> &str {
        &self.id
    }
}

impl Paged for SecurityConfigRecord {
    fn page_cursor(&self) -> &str {
        &self.id
    }
}

impl Paged for VpcEndpointRecord {
    fn page_cursor(&self) -> &str {
        &self.id
    }
}

/// One page of a listing: the records it carries, and the token that resumes after them.
#[derive(Debug)]
pub struct Page<'a, T> {
    /// The records of this page, in the listing's own cursor order.
    pub items: Vec<&'a T>,
    /// Set only when at least one more record remains after this page.
    pub next_token: Option<String>,
}

/// A page token is the cursor the NEXT page resumes after, hex-encoded.
///
/// Hex makes the token opaque in the sense the API promises — it is not an id a client
/// should read or construct — but it is NOT authenticated: hex is reversible, and a
/// hand-made token that decodes to UTF-8 is accepted and simply resumes after that cursor.
/// That is the same answer a token whose record has since been deleted gets, which is
/// what lets a listing resume correctly across creates and deletes rather than failing on
/// them. Nothing a token can say reaches past this listing's own order, so a forged one
/// grants no access: it can only start the same listing at a different place.
fn encode_page_token(cursor: &str) -> String {
    hex::encode(cursor)
}

/// The cursor a token resumes after, or the reason the token cannot be one this service
/// issued (its bytes are not hex, or not UTF-8).
fn decode_page_token(token: &str) -> Result<String, String> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| "nextToken is not a pagination token this service issued".to_string())
}

/// The page of `ordered` that `next_token` and `max_results` select.
///
/// `ordered` is the already-filtered record sequence in cursor order — the stable
/// sequence a token resumes inside. `max_results` is a page BOUND, not a count: a page
/// holds at most that many records, and `next_token` is returned only while more remain.
pub fn page<'a, T: Paged>(
    ordered: Vec<&'a T>,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<'a, T>, String> {
    let bound = max_results.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    let bound = usize::try_from(bound).unwrap_or(1);
    let after = match next_token {
        Some(token) => Some(decode_page_token(token)?),
        None => None,
    };

    let mut remaining = ordered.into_iter().filter(|record| {
        after
            .as_deref()
            .is_none_or(|from| record.page_cursor() > from)
    });
    let items: Vec<&'a T> = remaining.by_ref().take(bound).collect();
    // A token only when a record is left over: the last page of a listing carries none.
    let next_token = remaining
        .next()
        .and_then(|_| items.last().map(|record| encode_page_token(record.page_cursor())));

    Ok(Page { items, next_token })
}

/// The most values a stored `indexSchema` may carry.
///
/// EMULATOR RESOURCE GUARD, not an AWS limit: `CreateIndex` stores one CALLER-SUPPLIED
/// document per index and a collection may hold [`MAX_INDEXES_PER_COLLECTION`] of them, so
/// the size of ONE schema is the other half of what keeps that store finite. 10 000 values
/// is far past any real index mapping (a mapping of a few hundred fields), so a schema
/// past it is refused with the modelled `ValidationException` rather than stored.
pub const MAX_INDEX_SCHEMA_NODES: usize = 10_000;

/// Whether `document` is within the bounds this emulator stores a caller-supplied document
/// under: no deeper than [`MAX_DOCUMENT_DEPTH`] and no more than `max_nodes` values.
///
/// Walked with an EXPLICIT stack rather than by recursion, so a deeply nested document
/// cannot overflow the thread's stack before the depth check refuses it.
pub fn document_within_bounds(
    document: &::aws_smithy_types::Document,
    max_nodes: usize,
) -> bool {
    use ::aws_smithy_types::Document;

    let mut pending = vec![(document, 1usize)];
    let mut nodes = 0usize;
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if depth > MAX_DOCUMENT_DEPTH || nodes > max_nodes {
            return false;
        }
        match value {
            Document::Array(items) => pending.extend(items.iter().map(|item| (item, depth + 1))),
            Document::Object(members) => {
                pending.extend(members.values().map(|member| (member, depth + 1)));
            }
            _ => {}
        }
    }
    true
}

/// The deepest JSON nesting this emulator converts into an `aws_smithy_types::Document`.
///
/// EMULATOR RESOURCE GUARD, not an AWS limit: [`document_from_json`] recurses once per
/// nesting level over a CALLER-SUPPLIED document, and so does the resulting `Document`
/// tree's own drop, while a Rust stack overflow aborts the whole process instead of
/// failing one request. 64 levels is far past any real policy document — a data access
/// policy nests about four — so a document past it is refused with the modelled
/// `ValidationException` rather than converted. The bound is enforced HERE and does not
/// rely on `serde_json`'s own parse-time recursion limit.
pub const MAX_DOCUMENT_DEPTH: usize = 64;

/// The `aws_smithy_types::Document` a parsed JSON value is, so a policy document read
/// back is the document the caller supplied rather than its text.
///
/// `None` when the value nests deeper than [`MAX_DOCUMENT_DEPTH`]: the caller answers that
/// with the modelled `ValidationException`.
pub fn document_from_json(value: &serde_json::Value) -> Option<::aws_smithy_types::Document> {
    document_from_json_at(value, 1)
}

/// [`document_from_json`] at `depth`, where the top-level value is depth 1 and a
/// container's members are one deeper. The depth check is what bounds the recursion.
fn document_from_json_at(
    value: &serde_json::Value,
    depth: usize,
) -> Option<::aws_smithy_types::Document> {
    use ::aws_smithy_types::{Document, Number};
    if depth > MAX_DOCUMENT_DEPTH {
        return None;
    }
    Some(match value {
        serde_json::Value::Null => Document::Null,
        serde_json::Value::Bool(value) => Document::Bool(*value),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_u64() {
                Document::Number(Number::PosInt(value))
            } else if let Some(value) = value.as_i64() {
                Document::Number(Number::NegInt(value))
            } else {
                Document::Number(Number::Float(value.as_f64().unwrap_or_default()))
            }
        }
        serde_json::Value::String(value) => Document::String(value.clone()),
        serde_json::Value::Array(items) => Document::Array(
            items
                .iter()
                .map(|item| document_from_json_at(item, depth + 1))
                .collect::<Option<Vec<_>>>()?,
        ),
        serde_json::Value::Object(members) => Document::Object(
            members
                .iter()
                .map(|(name, member)| {
                    document_from_json_at(member, depth + 1).map(|member| (name.clone(), member))
                })
                .collect::<Option<_>>()?,
        ),
    })
}

/// The JSON value a request's `policy` text is: the FIRST half of the rule every
/// policy-writing operation follows, in the ONE place it lives, so no two of them can
/// drift apart over the wording of the refusal.
///
/// `Err` carries the message the caller wraps in its own modelled `ValidationException`:
/// this module owns the text, each operation owns the error enum the text travels in.
pub fn parse_policy_text(text: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(text).map_err(|error| format!("policy must be a JSON document: {error}"))
}

/// The stored document a parsed policy value converts to: the SECOND half of that rule,
/// again in one place, so the bound the refusal names is read from
/// [`MAX_DOCUMENT_DEPTH`] once.
///
/// Split from [`parse_policy_text`] because an operation whose policy content has its own
/// syntax (a `retention` lifecycle policy) checks that content against the parsed value,
/// at the point in its own order of checks where it checks it.
pub fn policy_document(parsed: &serde_json::Value) -> Result<::aws_smithy_types::Document, String> {
    document_from_json(parsed)
        .ok_or_else(|| format!("policy must not nest deeper than {MAX_DOCUMENT_DEPTH} levels"))
}

/// Both halves of the rule for an operation that needs only the document: parse the
/// `policy` text, then convert it.
pub fn policy_document_from_text(text: &str) -> Result<::aws_smithy_types::Document, String> {
    policy_document(&parse_policy_text(text)?)
}

// ---------------------------------------------------------------------------
// The audit record a call emits
// ---------------------------------------------------------------------------

/// Longest request-derived value one audit field carries.
///
/// A SigV4 access key id, a request path and a policy name are all caller-controlled, so
/// an unbounded echo would let one request emit an arbitrarily long log line.
const MAX_AUDIT_FIELD: usize = 256;

/// What the `resource` field reads as when the call resolved no resource.
pub const AUDIT_NO_RESOURCE: &str = "-";

/// A request-derived value as it may appear in an audit record: control characters (above
/// all CR and LF, which would forge a second record) replaced, and the whole value bounded
/// by [`MAX_AUDIT_FIELD`] on a character boundary.
fn audit_field(value: &str) -> String {
    value
        .chars()
        .map(|character| if character.is_control() { '.' } else { character })
        .take(MAX_AUDIT_FIELD)
        .collect()
}

/// What a call answered, as the one closed set [`audit`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    /// A policy was minted and stored.
    PolicyCreated,
    /// A stored policy was replaced by a write (`UpdateAccessPolicy`).
    PolicyUpdated,
    /// A stored policy was removed (`DeleteAccessPolicy`).
    PolicyDeleted,
    /// The `policyVersion` the write carried is not the stored one, so the write was
    /// refused (409 `ConflictException`).
    VersionMismatch,
    /// A replayed `clientToken` answered the record its first call created; nothing was
    /// written.
    IdempotentReplay,
    /// A stored policy was reported to a reader.
    PolicyRead,
    /// The account's own settings were written (`UpdateAccountSettings`).
    SettingsUpdated,
    /// The call reported the account's own settings as they stand: it named no limit the
    /// account did not already hold, so nothing was written (`UpdateAccountSettings`).
    SettingsUnchanged,
    /// The request was refused as invalid (400 `ValidationException`).
    ValidationRefused,
    /// A policy with that `(type, name)` pair already exists (409 `ConflictException`).
    AlreadyExists,
    /// An account quota was met (402 `ServiceQuotaExceededException`).
    QuotaExceeded,
    /// The `(type, name)` pair names no stored policy (404 `ResourceNotFoundException`).
    NotFound,
    /// This service's store could not be reached (500 `InternalServerException`).
    StoreUnreachable,
}

impl AuditOutcome {
    /// The stable token this outcome is logged as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PolicyCreated => "policy-created",
            Self::PolicyUpdated => "policy-updated",
            Self::PolicyDeleted => "policy-deleted",
            Self::VersionMismatch => "version-mismatch",
            Self::IdempotentReplay => "idempotent-replay",
            Self::PolicyRead => "policy-read",
            Self::SettingsUpdated => "settings-updated",
            Self::SettingsUnchanged => "settings-unchanged",
            Self::ValidationRefused => "validation-refused",
            Self::AlreadyExists => "already-exists",
            Self::QuotaExceeded => "quota-exceeded",
            Self::NotFound => "not-found",
            Self::StoreUnreachable => "store-unreachable",
        }
    }
}

/// One structured audit record for a call: which operation, the identity the request
/// presented, what it was addressed to, the resource it resolved and the outcome. The
/// `tracing` subscriber supplies the timestamp.
///
/// There is no network peer in the record because a Smithy handler is not given one: the
/// request's parts carry its method, URI, headers and extensions, and nothing in this
/// framework puts a peer address in them, so the presented credential is what identifies
/// the caller — the access key id only, never the credential itself. Deliberately metadata
/// only: no policy document or description is logged, so a caller's own data never reaches
/// the log. Every interpolated value is request-derived, so [`audit_field`] bounds it and
/// replaces the control characters that would otherwise let one request forge a second
/// record.
pub fn audit(
    operation: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    outcome: AuditOutcome,
    resource: Option<&str>,
) {
    let parts = ctx.parts();
    ::tracing::info!(
        target: "emulator::audit",
        service = ARN_NAMESPACE,
        operation = %audit_field(operation),
        access_key_id = %audit_field(&::emulator_server::extract_akid(&parts.headers)),
        request_target = %audit_field(&format!("{} {}", parts.method, parts.uri.path())),
        resource = %audit_field(resource.unwrap_or(AUDIT_NO_RESOURCE)),
        outcome = outcome.as_str(),
        "opensearchserverless api call"
    );
}

/// The `resource` field naming one data access policy: its type and name, which is its
/// identity in this API. Named for the access policy because the prefix is fixed: another
/// resource's records name their own.
pub fn audit_access_policy_resource(kind: &str, name: &str) -> String {
    format!("access-policy/{kind}/{name}")
}

/// The `resource` field naming one lifecycle policy: its type and name, which is its
/// identity in this API (`LifecyclePolicyIdentifier` is exactly that pair).
pub fn audit_lifecycle_policy_resource(kind: &str, name: &str) -> String {
    format!("lifecycle-policy/{kind}/{name}")
}

/// The `resource` field naming one security policy: its type and name, which is its
/// identity in this API (`SecurityPolicyType` plus `PolicyName` is what every
/// security-policy request addresses one by).
pub fn audit_security_policy_resource(kind: &str, name: &str) -> String {
    format!("security-policy/{kind}/{name}")
}

/// The `resource` field naming one security configuration: its `SecurityConfigId`, which is
/// the whole of its identity in this API — so a create, which is addressed by type and
/// name, names the id those two resolve to ([`security_config_id`]).
pub fn audit_security_config_resource(id: &str) -> String {
    format!("security-config/{id}")
}

/// The `resource` field naming the account's own settings, which `UpdateAccountSettings`
/// writes. A single resource per account, so it is a constant rather than a function:
/// nothing request-derived identifies it.
pub const AUDIT_ACCOUNT_SETTINGS_RESOURCE: &str = "account-settings";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsOpensearchserverlessOpensearchserverlessState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsOpensearchserverlessOpensearchserverlessState>)
}

/// What a handler answers when [`service_state`] cannot reach this service's store: the
/// modelled `InternalServerException`, which every operation in this slice declares.
pub const STORE_UNREACHABLE: &str =
    "the OpenSearch Serverless emulator's state store is not reachable from this engine";
