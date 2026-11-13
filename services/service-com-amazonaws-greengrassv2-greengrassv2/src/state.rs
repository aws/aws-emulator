// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-greengrassv2-greengrassv2 service, plus the
//! helpers its handlers share.
//!
//! The store holds the component catalog (one entry per component name, each
//! carrying that component's versions), the deployment history and the core-device
//! fleet ([`core_devices`]). Beside it live
//! the pieces more than one handler needs and that therefore cannot live in a
//! single `src/ops/<op>.rs`: ARN minting and parsing, component-recipe parsing and
//! format conversion, opaque pagination cursors and the timestamp conversion.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The core-device fleet, the client-device associations, and the pieces the
/// core-device operations share.
pub mod core_devices;
/// What every paginated reader of this service shares.
pub mod paging;
pub use core_devices::*;
pub use paging::*;

use crate::ops::types::{
    CloudComponentStatus, ComponentDeploymentSpecification, ComponentPlatform,
    DeploymentIoTJobConfiguration, DeploymentPolicies, ResourceNotFoundException,
    ValidationException,
};

/// Does the IoT thing or thing group a deployment names exist? Asked of the plane that
/// owns it.
pub mod iot_target;

/// May the caller perform the action it asked for? Decided from the IAM policies in force
/// on the principal its credentials speak for.
pub mod authorization;

/// This service's ARN namespace (`OPERATIONS.md`: ARN namespace `greengrass`).
pub const ARN_NAMESPACE: &str = "greengrass";

/// The region an ARN carries when the request's SigV4 credential scope names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `maxResults` bound for this service's lists, and the page size used when the
/// caller names none: the model's `DefaultMaxResults` is `range 1..100` and
/// declares no `@default`, so the upper bound is the whole-collection page.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The shortest gap this service keeps between the instant
/// `CreateComponentVersion` ACCEPTS a request — the instant its own response
/// reports the new version's state at — and the instant that request's recipe
/// validation completed, from which the version is `DEPLOYABLE`.
///
/// Greengrass validates an inline recipe as part of the create call: the create
/// answers the state at acceptance (`REQUESTED`), and a read that follows the
/// create reports the validated version (`DEPLOYABLE`). This emulator does that
/// validation in the same place — the recipe parse inside the handler — so the two
/// instants are the two clock readings around that parse, and they are always
/// apart. This constant is the floor that keeps them apart even if the wall clock
/// steps backwards between the readings.
pub const MIN_VALIDATION_GAP: Duration = Duration::from_micros(1);

/// The `ComponentType` IoT Greengrass assigns a component version published from a
/// recipe, and the value it writes into the recipe document it stores.
///
/// The type is the service's to assign, not the caller's: the other
/// `ComponentType` values name components generated from a Lambda function or
/// published by AWS itself, neither of which an `inlineRecipe` create produces.
pub const COMPONENT_TYPE_GENERIC: &str = "aws.greengrass.generic";

/// The recipe member the assigned [`COMPONENT_TYPE_GENERIC`] is written into, and
/// the member it is written after: the IoT Greengrass component recipe reference
/// publishes a recipe's members in this order, with `ComponentType` immediately
/// after `ComponentVersion`.
const RECIPE_TYPE_KEY: &str = "ComponentType";
/// See [`RECIPE_TYPE_KEY`].
const RECIPE_VERSION_KEY: &str = "ComponentVersion";
/// The recipe's list of platform-specific manifests, each of which carries its own
/// [`RECIPE_LIFECYCLE_KEY`] and [`RECIPE_ARTIFACTS_KEY`]. See
/// [`supply_empty_defaults`].
const RECIPE_MANIFESTS_KEY: &str = "Manifests";
/// A recipe member whose default is the empty mapping: the recipe's own lifecycle,
/// and a manifest's. See [`supply_empty_defaults`].
const RECIPE_LIFECYCLE_KEY: &str = "Lifecycle";
/// A manifest member whose default is the empty sequence. See
/// [`supply_empty_defaults`].
const RECIPE_ARTIFACTS_KEY: &str = "Artifacts";

/// `CloudComponentState` value a freshly created component version carries.
pub const STATE_REQUESTED: &str = "REQUESTED";
/// `CloudComponentState` value a validated component version carries.
pub const STATE_DEPLOYABLE: &str = "DEPLOYABLE";
/// `VendorGuidance` value a component version that is current carries.
pub const VENDOR_GUIDANCE_ACTIVE: &str = "ACTIVE";
/// What a `CloudComponentStatus` message member says when there is nothing to report:
/// Greengrass fills `message` and `vendorGuidanceMessage` with this sentinel rather
/// than omitting them, so a reader of either member always finds a value.
pub const STATUS_MESSAGE_NONE: &str = "NONE";
/// `ComponentVisibilityScope` of a component this account published.
pub const SCOPE_PRIVATE: &str = "PRIVATE";
/// `DeploymentStatus` of the deployment that is current for its target.
pub const DEPLOYMENT_STATUS_ACTIVE: &str = "ACTIVE";
/// What `CreateDeployment` reports for a `targetArn` whose IoT thing or thing group the
/// plane that owns it holds nothing under: one fixed sentence that names no value of the
/// request, so the refusal reports neither the ARN nor the name it carried.
pub const TARGET_NOT_FOUND_MESSAGE: &str = "Invalid Input: Target not found";

/// `DeploymentHistoryFilter` value that lists every revision of every deployment
/// (`smithy_query(query="shape",shape="DeploymentHistoryFilter")`: `ALL`,
/// `LATEST_ONLY`).
pub const HISTORY_FILTER_ALL: &str = "ALL";

/// `DeploymentHistoryFilter` value that lists only the latest revision of each
/// deployment, which is what `ListDeployments` lists when the caller names no filter
/// (`ListDeployments$historyFilter`: "Default: LATEST_ONLY").
pub const HISTORY_FILTER_LATEST_ONLY: &str = "LATEST_ONLY";

/// `DeploymentStatus` of a deployment whose caller cancelled it
/// (`smithy_query(query="shape",shape="DeploymentStatus")`: `ACTIVE`, `COMPLETED`,
/// `CANCELED`, `FAILED`, `INACTIVE`).
pub const DEPLOYMENT_STATUS_CANCELED: &str = "CANCELED";

/// `DeploymentStatus` of a revision a newer deployment to the same target replaced.
///
/// TERMINAL: a superseded revision is no longer being applied to its target, and a
/// deployment never returns to `ACTIVE` from it — the target's next revision is a new
/// deployment, not this one again.
pub const DEPLOYMENT_STATUS_INACTIVE: &str = "INACTIVE";

/// What every operation keyed by a `deploymentId` reports for an id this service holds
/// no deployment for: one fixed sentence that names no value of the request, so the
/// refusal is the same whichever id was asked about and whichever operation asked.
pub const DEPLOYMENT_NOT_FOUND_MESSAGE: &str = "Deployment not found";

/// The most tags one resource's stored tag set may hold.
///
/// The model's `TagMap` is `length 1..200`
/// (`smithy_query(query="shape",shape="TagMap")`), so a larger set is not a value this
/// service's own shape can carry. One request is already bounded by that constraint;
/// what this bounds is the stored set a caller grows by tagging the same resource
/// repeatedly with different keys.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The largest `inlineRecipe` this emulator will parse.
///
/// This is NOT an AWS quota: it is CHOSEN, orders of magnitude above any real
/// component recipe, and it exists because the document parser underneath is
/// recursive. Without a bound, one request could overflow the thread stack —
/// which aborts the whole process rather than failing the request. A breach is
/// answered as a 400, never as a crash.
pub const MAX_RECIPE_BYTES: usize = 256 * 1024;

/// The deepest nesting a recipe document may have, enforced on the LOADED tree
/// (`document_within_bounds`) and again as a budget inside the recursive
/// renderer. See [`MAX_RECIPE_BYTES`].
pub const MAX_RECIPE_DEPTH: usize = 256;

/// The most nesting-opening indicators (`[`, `{`, `-`, `?`) a recipe may contain
/// in TOTAL.
///
/// A document's nesting depth can never exceed the NUMBER of indicators that open
/// a level, so counting them — every byte, with nothing skipped for quoting or
/// comments — is an upper bound computable before any tree exists to measure.
/// Over-counting a `-` inside a scalar only makes the bound stricter, never
/// weaker.
///
/// This total is CHOSEN high enough that over-counting does not refuse a real
/// recipe: a hyphen-rich Lifecycle script across a few hundred manifests runs to a
/// few thousand indicators (the suite pins a 400-manifest recipe at roughly
/// 3 000). It is NOT by itself the recursion bound — the two tighter per-kind
/// bounds below are, because they cap how deep those indicators can actually nest.
/// See [`MAX_RECIPE_BYTES`].
pub const MAX_RECIPE_STRUCTURAL_INDICATORS: usize = 4096;

/// The most flow-collection openers (`[`, `{`) a recipe may contain in total.
///
/// Flow nesting is the one kind that costs neither a column nor a line, so the
/// count of its openers IS its depth bound — closers are never subtracted, so a
/// `]` hidden in a scalar cannot buy depth back. This caps how deep the recursive
/// parser can descend while it still has no tree to measure. A JSON recipe with a
/// thousand nested collections is already far larger than any real one.
/// See [`MAX_RECIPE_BYTES`].
pub const MAX_RECIPE_FLOW_OPENERS: usize = 1024;

/// The most compact block indicators (`-`, `?`) a recipe may use on ONE line.
///
/// A compact entry (`- - - x`) nests without costing a column, but only within its
/// own line, so a per-line cap bounds that nesting; nesting across lines is
/// bounded by [`MAX_RECIPE_INDENT`] instead. See [`MAX_RECIPE_BYTES`].
pub const MAX_RECIPE_COMPACT_PER_LINE: usize = 256;

/// The widest indentation a recipe line may use. Block nesting costs at least one
/// column per level. See [`MAX_RECIPE_BYTES`].
pub const MAX_RECIPE_INDENT: usize = 256;

/// How long a successful response is replayed for a repeated `clientToken`.
///
/// AWS documents this one: the `clientToken` member of `CreateComponentVersion`
/// and `CreateDeployment` states that "IoT Greengrass V2 caches successful
/// responses for idempotent requests for up to 8 hours". A cached token stops
/// being replayed once that window passes.
pub const IDEMPOTENCY_CACHE_TTL: Duration = Duration::from_secs(8 * 60 * 60);

/// The most cached idempotency tokens this emulator keeps at once.
///
/// [`IDEMPOTENCY_CACHE_TTL`] bounds the cache in TIME but not in COUNT: a caller
/// that varies `clientToken` per request would otherwise grow the cache for the
/// whole window. Past this count the oldest cached token is forgotten early. The
/// number is CHOSEN; each entry is small and bounded by the model, whose
/// `ClientTokenString` is `length 1..64` (checked at the boundary, before a token
/// can ever become a key here).
pub const MAX_IDEMPOTENCY_TOKENS: usize = 20_000;

/// The most component versions this emulator stores.
///
/// A breach is answered with the modelled `ServiceQuotaExceededException`, whose
/// own documentation reads "you might have the maximum number of components that
/// you can create". The NUMBER is CHOSEN — AWS's published quota is per account
/// and adjustable — and exists so sustained creates cannot grow the process
/// without bound.
pub const MAX_COMPONENT_VERSIONS: usize = 10_000;

/// The quota code reported with [`MAX_COMPONENT_VERSIONS`].
pub const COMPONENT_VERSION_QUOTA_CODE: &str = "ComponentVersionsPerAccount";

/// The most component candidates one `ResolveComponentCandidates` request may carry.
///
/// This is NOT an AWS quota: it is CHOSEN, well above the component count of any real
/// deployment. Resolving one candidate walks the versions of the component it names and,
/// for each, checks that candidate's version requirements, so the work of a request is
/// the PRODUCT of THREE factors: this candidate count, the stored version count
/// ([`MAX_COMPONENT_VERSIONS`]) and the requirement count
/// ([`MAX_RESOLVE_REQUIREMENTS`]). Only a bound on all three bounds it, so all three are
/// bounded; a breach of either caller-chosen one is answered as a 400, so the request is
/// refused rather than served slowly.
pub const MAX_RESOLVE_CANDIDATES: usize = 100;

/// The most version requirements one `ResolveComponentCandidates` request may carry in
/// TOTAL, across every candidate.
///
/// The third caller-chosen factor of the product [`MAX_RESOLVE_CANDIDATES`] describes.
/// The model puts no `@length` on `ComponentVersionRequirementMap`, so without this the
/// only ceiling would be the request-body size. CHOSEN: a real deployment's candidate
/// carries one requirement per component that depends on it, and the total is bounded
/// here rather than per candidate so a caller cannot multiply the bound by the candidate
/// count.
pub const MAX_RESOLVE_REQUIREMENTS: usize = 500;

/// The most deployments this emulator retains in total, across every target.
///
/// `targetArn` is chosen by the caller, so a per-target bound alone would still
/// let a caller that varies the target grow the store forever. Past this count the
/// OLDEST deployment is evicted, whichever target it belongs to, and a target whose
/// history empties loses its index entries with it — so `deployments`,
/// `deployments_by_target` and `latest_deployment_by_target` are all bounded by
/// this one number. Evicting rather than refusing keeps `CreateDeployment`
/// answering what AWS answers: the model gives it no quota error.
pub const MAX_DEPLOYMENTS: usize = 20_000;

/// The most deployment revisions this emulator retains per target.
///
/// A deployment history is a log, and this one is trimmed oldest-first so the
/// store cannot grow without bound; the deployment that is latest for a target is
/// never trimmed. CHOSEN, for the same reason as [`MAX_COMPONENT_VERSIONS`].
pub const MAX_DEPLOYMENT_REVISIONS_PER_TARGET: usize = 1_000;

/// Which wire format a stored recipe's bytes are written in — the
/// `RecipeOutputFormat` enum (`JSON | YAML`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipeFormat {
    Json,
    Yaml,
}

impl RecipeFormat {
    /// The `RecipeOutputFormat` enum value this format is named by on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Yaml => "YAML",
        }
    }

    /// The format a `RecipeOutputFormat` wire value names, or `None` when the
    /// value is not one the enum declares.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "JSON" => Some(Self::Json),
            "YAML" => Some(Self::Yaml),
            _ => None,
        }
    }
}

/// One stored component version: the recipe document the service derived from the
/// submitted one, plus everything `DescribeComponent` / `ListComponents` report
/// about it.
#[derive(Debug, Clone)]
pub struct ComponentVersionEntry {
    pub arn: String,
    pub component_name: String,
    pub component_version: String,
    pub recipe: Vec<u8>,
    pub recipe_format: RecipeFormat,
    pub description: Option<String>,
    pub publisher: Option<String>,
    pub platforms: Vec<ComponentPlatform>,
    pub creation_timestamp: SystemTime,
    pub status: CloudComponentStatus,
    /// The instant this version's recipe validation completed, from which the
    /// version is `DEPLOYABLE`.
    pub deployable_at: SystemTime,
    pub scope: String,
    pub tags: BTreeMap<String, String>,
}

impl ComponentVersionEntry {
    /// The component version's status at `now`.
    ///
    /// `componentState` is DERIVED from the instant the version's recipe
    /// validation completed rather than written during a read: a reporting
    /// operation changes nothing a later caller sees, two readers at the same
    /// instant agree, and the work is O(1) per entry rather than a walk of the
    /// whole catalog. So the create that published the version reports the state
    /// at the instant it accepted the request — before it validated the recipe,
    /// which is `REQUESTED` — and every call that follows it reports the
    /// validated version's terminal `DEPLOYABLE`.
    pub fn status_at(&self, now: SystemTime) -> CloudComponentStatus {
        let mut status = self.status.clone();
        status.component_state = Some(
            if now >= self.deployable_at {
                STATE_DEPLOYABLE
            } else {
                STATE_REQUESTED
            }
            .to_string(),
        );
        status
    }
}

/// One component name and every version published under it, keyed by version.
#[derive(Debug, Clone)]
pub struct ComponentEntry {
    pub name: String,
    pub name_arn: String,
    pub versions: BTreeMap<String, ComponentVersionEntry>,
}

impl ComponentEntry {
    /// The component's latest version: the greatest by semantic-version
    /// precedence among the versions visible in `scope`.
    pub fn latest_version(&self, scope: &str) -> Option<&ComponentVersionEntry> {
        self.versions
            .values()
            .filter(|v| v.scope == scope)
            .max_by(|a, b| version_cmp(&a.component_version, &b.component_version))
    }
}

/// One stored deployment, exactly as `CreateDeployment` recorded it.
#[derive(Debug, Clone)]
pub struct DeploymentEntry {
    pub deployment_id: String,
    pub revision_id: String,
    pub deployment_name: Option<String>,
    pub target_arn: String,
    pub parent_target_arn: Option<String>,
    pub components: BTreeMap<String, ComponentDeploymentSpecification>,
    pub deployment_policies: Option<DeploymentPolicies>,
    pub iot_job_configuration: Option<DeploymentIoTJobConfiguration>,
    pub iot_job_id: String,
    pub iot_job_arn: String,
    pub deployment_status: String,
    pub is_latest_for_target: bool,
    pub creation_timestamp: SystemTime,
    pub tags: BTreeMap<String, String>,
}

/// One target's deployment history: its deployment ids in creation order, and how
/// many revisions that target has ever had (so a revision number keeps rising even
/// after the oldest entries are trimmed).
#[derive(Debug, Default, Clone)]
pub struct TargetDeployments {
    pub ids: VecDeque<String>,
    pub revisions: u64,
}

/// The Greengrass service role association this account holds: the role's ARN and the
/// instant it was associated at.
///
/// ONE record, not a collection: the service role is a single account-level
/// association, so associating a role REPLACES whatever was associated before rather
/// than adding to a list, and disassociating removes the one record. The instant is
/// kept as a `SystemTime` and rendered where it is reported
/// ([`to_timestamp_text`]), so the read reports the same `associatedAt` the write
/// answered.
#[derive(Debug, Clone)]
pub struct ServiceRoleAssociation {
    pub role_arn: String,
    pub associated_at: SystemTime,
}

/// Per-service state for com-amazonaws-greengrassv2-greengrassv2. A handler reaches it through [`service_state`].
///
/// Every collection here is bounded by an enforced COUNT: component versions by
/// [`MAX_COMPONENT_VERSIONS`]; the deployment store and both of its indexes by
/// [`MAX_DEPLOYMENTS`] in total (with a per-target cap of
/// [`MAX_DEPLOYMENT_REVISIONS_PER_TARGET`] on top); and the idempotency cache by
/// [`MAX_IDEMPOTENCY_TOKENS`], on top of AWS's own documented
/// [`IDEMPOTENCY_CACHE_TTL`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGreengrassv2Greengrassv2State {
    /// Published components, keyed by component name.
    pub components: BTreeMap<String, ComponentEntry>,
    /// How many component versions `components` holds in total, so the quota check
    /// costs no walk of the catalog.
    pub component_version_count: usize,
    /// Deployments, keyed by deployment id.
    pub deployments: BTreeMap<String, DeploymentEntry>,
    /// Each target's deployment history, which is both the revision counter and
    /// the order that target's history is trimmed in.
    pub deployments_by_target: BTreeMap<String, TargetDeployments>,
    /// Every stored deployment id in creation order, so the OLDEST deployment
    /// across all targets can be evicted once [`MAX_DEPLOYMENTS`] is reached.
    pub deployment_order: VecDeque<String>,
    /// The deployment that is latest for each `targetArn`, so `isLatestForTarget`
    /// and `historyFilter=LATEST_ONLY` are answered from state.
    pub latest_deployment_by_target: BTreeMap<String, String>,
    /// A replayed `@idempotencyToken` maps back to the resource the first request
    /// created, so the replay returns that result instead of creating a duplicate.
    /// Keys are namespaced by operation (`"<Operation>:<clientToken>"`), and each
    /// entry carries the instant it stops being replayed.
    pub idempotency_tokens: BTreeMap<String, (String, SystemTime)>,
    /// The cached tokens in expiry order, so expiring them costs only the entries
    /// that actually expired.
    pub idempotency_expiry: VecDeque<(SystemTime, String)>,
    /// The Greengrass service role associated with this account, or `None` while no
    /// role is associated — which is the state an account starts in and the one a
    /// disassociation returns it to. At most one record, so it needs no count bound.
    pub service_role: Option<ServiceRoleAssociation>,
    /// The account's Greengrass core devices, keyed by thing name — the fleet
    /// `ListCoreDevices` enumerates and `GetCoreDevice` reads. Bounded by
    /// [`MAX_CORE_DEVICES`]; written as the device's own status update writes it
    /// ([`core_devices`]).
    pub core_devices: BTreeMap<String, CoreDeviceEntry>,
    /// The client devices associated with each core device thing name for cloud
    /// discovery, and the instant each was associated. Keyed by the CORE DEVICE's thing
    /// name, and held beside the fleet rather than on a core-device record because AWS
    /// records an association for a thing name whether or not a core device has reported
    /// itself under it. Bounded by [`MAX_CORE_DEVICES_WITH_ASSOCIATIONS`] names, each by
    /// [`MAX_CLIENT_DEVICES_PER_CORE_DEVICE`].
    pub client_device_associations: BTreeMap<String, BTreeMap<String, SystemTime>>,
}

impl ComAmazonawsGreengrassv2Greengrassv2State {
    /// The stored version a component-version ARN names.
    pub fn component_version(&self, arn: &str) -> Option<&ComponentVersionEntry> {
        let (name, version) = parse_component_version_arn(arn)?;
        let entry = self.components.get(&name)?.versions.get(&version)?;
        (entry.arn == arn).then_some(entry)
    }

    /// The tag set stored for the resource a `GenericV2ARN` names, or `None` when
    /// this service holds no such resource.
    ///
    /// A tag set belongs to the resource it was written on, so this reads the ONE
    /// copy the creating operation persisted rather than a second map beside it:
    /// a component version's tags from the catalog entry, a deployment's from the
    /// deployment history, a core device's from its own fleet record — so an ARN whose
    /// resource this service does not hold, of any of the three kinds, names nothing
    /// here.
    pub fn resource_tags(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        match NamedResource::from_arn(arn)? {
            NamedResource::ComponentVersion { .. } => Some(&self.component_version(arn)?.tags),
            NamedResource::Deployment { deployment_id } => {
                Some(&self.deployments.get(&deployment_id)?.tags)
            }
            NamedResource::CoreDevice {
                core_device_thing_name,
            } => Some(&self.core_device(&core_device_thing_name)?.tags),
        }
    }

    /// The stored version a component-version ARN names, for an operation that WRITES
    /// to it. The same lookup [`Self::component_version`] does, so a reader and a
    /// writer address one entry.
    pub fn component_version_mut(&mut self, arn: &str) -> Option<&mut ComponentVersionEntry> {
        let (name, version) = parse_component_version_arn(arn)?;
        let entry = self.components.get_mut(&name)?.versions.get_mut(&version)?;
        (entry.arn == arn).then_some(entry)
    }

    /// The tag set stored for the resource a `GenericV2ARN` names, for an operation
    /// that writes tags, or `None` when this service holds no such resource.
    ///
    /// The write side of [`Self::resource_tags`], resolving the ARN the same way: the
    /// tag set a mutation changes is the ONE copy the resource holds, so a later
    /// listing reports what was written rather than a second map beside it.
    pub fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        match NamedResource::from_arn(arn)? {
            NamedResource::ComponentVersion { .. } => {
                Some(&mut self.component_version_mut(arn)?.tags)
            }
            NamedResource::Deployment { deployment_id } => {
                Some(&mut self.deployments.get_mut(&deployment_id)?.tags)
            }
            NamedResource::CoreDevice {
                core_device_thing_name,
            } => Some(&mut self.core_device_mut(&core_device_thing_name)?.tags),
        }
    }

    /// Insert a published component version, maintaining the total count.
    pub fn insert_component_version(&mut self, name_arn: &str, entry: ComponentVersionEntry) {
        let component = self
            .components
            .entry(entry.component_name.clone())
            .or_insert_with(|| ComponentEntry {
                name: entry.component_name.clone(),
                name_arn: name_arn.to_string(),
                versions: BTreeMap::new(),
            });
        if component
            .versions
            .insert(entry.component_version.clone(), entry)
            .is_none()
        {
            self.component_version_count += 1;
        }
    }

    /// Remove a published component version — and the component itself once its
    /// last version is gone — maintaining the total count.
    pub fn remove_component_version(&mut self, name: &str, version: &str) -> bool {
        let Some(component) = self.components.get_mut(name) else {
            return false;
        };
        if component.versions.remove(version).is_none() {
            return false;
        }
        self.component_version_count = self.component_version_count.saturating_sub(1);
        if component.versions.is_empty() {
            self.components.remove(name);
        }
        true
    }

    /// Forget every cached idempotency token whose replay window has passed.
    ///
    /// Tokens are cached for a fixed window, so `idempotency_expiry` is already in
    /// expiry order and only the expired prefix is examined.
    pub fn expire_tokens(&mut self, now: SystemTime) {
        while let Some((expires_at, key)) = self.idempotency_expiry.front() {
            if *expires_at > now {
                break;
            }
            self.idempotency_tokens.remove(key);
            self.idempotency_expiry.pop_front();
        }
    }

    /// Record `token -> resource` for an operation's idempotency token.
    ///
    /// Bounded in BOTH dimensions: expired tokens are dropped first
    /// ([`IDEMPOTENCY_CACHE_TTL`]), and past [`MAX_IDEMPOTENCY_TOKENS`] the oldest
    /// cached token is forgotten early, so a caller that varies `clientToken` per
    /// request cannot grow the cache for the length of the replay window.
    pub fn remember_token(
        &mut self,
        operation: &str,
        token: Option<&str>,
        resource: &str,
        now: SystemTime,
    ) {
        self.expire_tokens(now);
        if let Some(token) = token {
            let key = format!("{operation}:{token}");
            let expires_at = now + IDEMPOTENCY_CACHE_TTL;
            if self
                .idempotency_tokens
                .insert(key.clone(), (resource.to_string(), expires_at))
                .is_some()
            {
                // The key was already cached, so its earlier queue entry is stale:
                // drop it, or the count cap would evict live entries early.
                self.idempotency_expiry.retain(|(_, cached)| cached != &key);
            }
            self.idempotency_expiry.push_back((expires_at, key));
            while self.idempotency_tokens.len() > MAX_IDEMPOTENCY_TOKENS {
                let Some((_, oldest)) = self.idempotency_expiry.pop_front() else {
                    break;
                };
                self.idempotency_tokens.remove(&oldest);
            }
        }
    }

    /// The resource a replayed idempotency token already created, if its replay
    /// window has not passed.
    pub fn recall_token(
        &self,
        operation: &str,
        token: Option<&str>,
        now: SystemTime,
    ) -> Option<String> {
        let token = token?;
        let (resource, expires_at) = self.idempotency_tokens.get(&format!("{operation}:{token}"))?;
        (*expires_at > now).then(|| resource.clone())
    }

    /// Record a deployment as the newest revision for its target.
    ///
    /// Supersedes the deployment that held `isLatestForTarget` — which stops being the
    /// latest AND stops being applied, so an `ACTIVE` revision settles at
    /// [`DEPLOYMENT_STATUS_INACTIVE`] while one already in a terminal status keeps it —
    /// trims that target's history to [`MAX_DEPLOYMENT_REVISIONS_PER_TARGET`], and then
    /// trims the store as a whole to [`MAX_DEPLOYMENTS`] by dropping the oldest
    /// deployment across every target — so neither the number of revisions per target nor
    /// the number of distinct targets, both caller-chosen, can grow the store without
    /// bound.
    pub fn insert_deployment(&mut self, entry: DeploymentEntry) {
        let target = entry.target_arn.clone();
        let id = entry.deployment_id.clone();

        if let Some(previous) = self
            .latest_deployment_by_target
            .insert(target.clone(), id.clone())
        {
            if let Some(superseded) = self.deployments.get_mut(&previous) {
                superseded.is_latest_for_target = false;
                if superseded.deployment_status == DEPLOYMENT_STATUS_ACTIVE {
                    superseded.deployment_status = DEPLOYMENT_STATUS_INACTIVE.to_string();
                }
            }
        }
        self.deployments.insert(id.clone(), entry);
        self.deployment_order.push_back(id.clone());
        let history = self.deployments_by_target.entry(target).or_default();
        history.revisions = history.revisions.saturating_add(1);
        history.ids.push_back(id.clone());

        // Trim this target's own history, never the entry that is latest for it.
        let mut stale: Vec<String> = Vec::new();
        while history.ids.len() > MAX_DEPLOYMENT_REVISIONS_PER_TARGET {
            let Some(oldest) = history.ids.pop_front() else {
                break;
            };
            if oldest == id {
                history.ids.push_front(oldest);
                break;
            }
            stale.push(oldest);
        }
        if !stale.is_empty() {
            // One pass over the global order queue for the whole trimmed batch.
            self.deployment_order
                .retain(|stored| !stale.iter().any(|dropped| dropped == stored));
            for oldest in stale {
                self.forget_deployment(&oldest);
            }
        }

        // Then trim the store as a whole, oldest first, whichever target it
        // belongs to. The oldest is at the front of the order queue, so each
        // eviction is O(1) in it.
        while self.deployments.len() > MAX_DEPLOYMENTS {
            let Some(oldest) = self.deployment_order.front().cloned() else {
                break;
            };
            if oldest == id {
                break;
            }
            self.deployment_order.pop_front();
            self.forget_deployment(&oldest);
        }
    }

    /// Drop one stored deployment and every index entry that referred to it.
    ///
    /// The caller owns `deployment_order`; this maintains `deployments`, the
    /// target's history, and the invariant that a target WITH history has exactly
    /// one latest deployment — so evicting the one that held `isLatestForTarget`
    /// promotes the newest survivor rather than leaving the target with none.
    fn forget_deployment(&mut self, id: &str) {
        let Some(entry) = self.deployments.remove(id) else {
            return;
        };
        let target = entry.target_arn;
        let mut newest = None;
        let mut emptied = false;
        if let Some(history) = self.deployments_by_target.get_mut(&target) {
            history.ids.retain(|stored| stored != id);
            emptied = history.ids.is_empty();
            newest = history.ids.back().cloned();
        }
        if emptied {
            self.deployments_by_target.remove(&target);
            self.latest_deployment_by_target.remove(&target);
            return;
        }
        if self
            .latest_deployment_by_target
            .get(&target)
            .is_some_and(|latest| latest == id)
        {
            match newest {
                Some(promoted_id) => {
                    self.latest_deployment_by_target
                        .insert(target, promoted_id.clone());
                    if let Some(promoted) = self.deployments.get_mut(&promoted_id) {
                        promoted.is_latest_for_target = true;
                    }
                }
                None => {
                    self.latest_deployment_by_target.remove(&target);
                }
            }
        }
    }

    /// Delete the deployment `id` names, and every index entry that referred to it.
    ///
    /// `false` when this service holds no deployment under that id, so a caller refuses
    /// the request rather than reporting a delete it did not make. The global order
    /// queue is this method's to maintain (the private eviction path leaves it to its
    /// own caller), so the id is dropped from it before the record and its indexes go.
    pub fn remove_deployment(&mut self, id: &str) -> bool {
        if !self.deployments.contains_key(id) {
            return false;
        }
        self.deployment_order.retain(|stored| stored != id);
        self.forget_deployment(id);
        true
    }

    /// The revision number `CreateDeployment` assigns to the next deployment for
    /// this target. It counts every revision the target has ever had, so trimming
    /// the history never makes a revision number repeat.
    pub fn next_target_revision(&self, target_arn: &str) -> u64 {
        self.deployments_by_target
            .get(target_arn)
            .map_or(0, |history| history.revisions)
            .saturating_add(1)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGreengrassv2Greengrassv2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGreengrassv2Greengrassv2State>)
}

/// The SHARED IoT jobs store in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A deployment is APPLIED by an AWS IoT job — `CreateDeployment` reports that job's
/// `iotJobId` and `iotJobArn` — and a client then tracks the rollout through the IoT
/// jobs plane, which is a different service crate. So the job this service mints is
/// recorded in the one store that plane reads,
/// `service_common::iot_jobs::IotJobStore`, and not in
/// [`ComAmazonawsGreengrassv2Greengrassv2State`]: the engine keys state by TYPE, so a
/// copy of its own here would be invisible there and the id this service reports would
/// address nothing.
///
/// LOCK ORDER: [`service_state`] first, this one second.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn iot_job_store<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::iot_jobs::IotJobStore>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<::service_common::iot_jobs::IotJobStore>)
}

// ---------------------------------------------------------------------------
// Request-derived values
// ---------------------------------------------------------------------------

/// The region the request was signed for, which is the region every ARN this
/// request mints carries. Falls back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The name-level component ARN (no `:versions:` segment) `ListComponents` reports.
pub fn component_name_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("components:{name}"))
}

/// The component-version ARN `CreateComponentVersion` mints.
pub fn component_version_arn(region: &str, name: &str, version: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("components:{name}:versions:{version}"),
    )
}

/// The deployment ARN that names a deployment of this account — the `deployments:` form of
/// the model's `GenericV2ARN`, which is the ARN the tagging route takes for one
/// ([`NamedResource`]).
pub fn deployment_arn(region: &str, deployment_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("deployments:{deployment_id}"))
}

/// The core-device ARN that names a core device of this account — the `coreDevices:` form
/// of the model's `GenericV2ARN` ([`NamedResource`]).
pub fn core_device_arn(region: &str, core_device_thing_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("coreDevices:{core_device_thing_name}"),
    )
}

/// The IoT job ARN a deployment's job is reported under.
pub fn iot_job_arn(region: &str, job_id: &str) -> String {
    ::engine::emulator_arn("iot", region, &format!("job/{job_id}"))
}

/// Split a component-version ARN into its component name and version.
///
/// The shape is
/// `arn:<partition>:greengrass:<region>:<account>:components:<name>:versions:<version>`,
/// and the model's `ComponentVersionARN` pattern makes both `<name>` and
/// `<version>` colon-free, so a plain `:` split recovers them.
pub fn parse_component_version_arn(arn: &str) -> Option<(String, String)> {
    let fields: Vec<&str> = arn.split(':').collect();
    match fields.as_slice() {
        ["arn", _partition, "greengrass", _region, _account, "components", name, "versions", version]
            if !name.is_empty() && !version.is_empty() =>
        {
            Some(((*name).to_string(), (*version).to_string()))
        }
        _ => None,
    }
}

/// The component name a NAME-level component ARN names — the shape
/// `arn:<partition>:greengrass:<region>:<account>:components:<name>`, which is what
/// `ListComponents` reports and `ListComponentVersions` takes.
///
/// The model's `ComponentARN` pattern makes `<name>` colon-free and forbids the
/// `:versions:` tail, so a plain `:` split recovers it.
pub fn parse_component_arn(arn: &str) -> Option<String> {
    let fields: Vec<&str> = arn.split(':').collect();
    match fields.as_slice() {
        ["arn", _partition, "greengrass", _region, _account, "components", name]
            if !name.is_empty() =>
        {
            Some((*name).to_string())
        }
        _ => None,
    }
}

/// The resource a `GenericV2ARN` names.
///
/// That shape's pattern admits exactly three resource fields — `components:`,
/// `deployments:` and `coreDevices:` — so an ARN any operation taking one carries
/// names a component version, a deployment or a core device, and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamedResource {
    /// A published component version, `components:<name>:versions:<version>`.
    ComponentVersion { name: String, version: String },
    /// A deployment, `deployments:<deploymentId>`.
    Deployment { deployment_id: String },
    /// A core device, `coreDevices:<coreDeviceThingName>`.
    CoreDevice { core_device_thing_name: String },
}

impl NamedResource {
    /// The resource `arn` names, or `None` when its resource field is not the shape
    /// any of the three kinds has.
    ///
    /// Every one of those resource fields is colon-separated and its identifier is
    /// itself colon-free (`ComponentVersionARN`'s pattern, and a deployment id and a
    /// thing name carry no `:`), so a plain `:` split recovers it.
    pub fn from_arn(arn: &str) -> Option<Self> {
        // The component-version shape is already parsed once in this module, for the
        // operations that take a `ComponentVersionARN`; this is the same ARN.
        if let Some((name, version)) = parse_component_version_arn(arn) {
            return Some(Self::ComponentVersion { name, version });
        }
        let fields: Vec<&str> = arn.split(':').collect();
        match fields.as_slice() {
            ["arn", _partition, "greengrass", _region, _account, "deployments", deployment_id]
                if !deployment_id.is_empty() =>
            {
                Some(Self::Deployment {
                    deployment_id: (*deployment_id).to_string(),
                })
            }
            ["arn", _partition, "greengrass", _region, _account, "coreDevices", thing_name]
                if !thing_name.is_empty() =>
            {
                Some(Self::CoreDevice {
                    core_device_thing_name: (*thing_name).to_string(),
                })
            }
            _ => None,
        }
    }

    /// The `resourceType` an exception about this resource carries, as the handlers
    /// that already report each kind word it.
    pub fn resource_type(&self) -> &'static str {
        match self {
            Self::ComponentVersion { .. } => "COMPONENT_VERSION",
            Self::Deployment { .. } => "DEPLOYMENT",
            Self::CoreDevice { .. } => "CORE_DEVICE",
        }
    }
}

// ---------------------------------------------------------------------------
// The tagging route's own refusal
// ---------------------------------------------------------------------------

/// The error code the tagging route (`/tags/{resourceArn}`) answers for an ARN that
/// names no resource this service holds.
///
/// RECORDED behaviour, not modelled: this service's model declares no shape of that
/// name (`smithy_query(query="shape",shape="NotFoundException")` finds none) and lists
/// `ResourceNotFoundException` on the three tagging operations, yet the tagging layer
/// answers its OWN not-found class — distinct from the code the id-keyed component and
/// deployment operations carry for the same missing resource. A client of the tagging
/// route branches on this code, so it is what the route answers.
pub const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";

/// The sentence that refusal carries: entirely fixed text, the same whichever ARN was
/// presented, so the refusal reports no value of the request.
pub const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The HTTP status that refusal carries.
pub const TAGGING_NOT_FOUND_STATUS: u16 = 404;

/// The `message` member of [`TaggingNotFound`].
static TAGGING_NOT_FOUND_MESSAGE_MEMBER: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "smithy.framework#NotFoundException$message",
            "smithy.framework",
            "message",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "message",
        0,
    )
    .with_xml_name("Message");

/// Schema for the shape [`TaggingNotFound`] serializes as.
///
/// Deliberately in the `smithy.framework` namespace, as `crate::scaffolded`'s synthetic
/// shapes are, so it cannot collide with a modelled shape; the wire `__type` and the
/// `x-amzn-errortype` header come from each tagging operation's own
/// `OperationError::shape_name`, which answers [`TAGGING_NOT_FOUND_CODE`], not from here.
pub static TAGGING_NOT_FOUND_SCHEMA: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "smithy.framework#NotFoundException",
            "smithy.framework",
            "NotFoundException",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        &[&TAGGING_NOT_FOUND_MESSAGE_MEMBER],
    );

/// The refusal every operation on the tagging route answers an ARN that names no
/// resource this service holds with.
///
/// Stated here once rather than per operation: the three operations the model binds to
/// `/tags/{resourceArn}` refuse a missing resource identically, so two of them cannot
/// answer differently.
#[derive(Debug, Clone)]
pub struct TaggingNotFound {
    pub message: String,
}

impl TaggingNotFound {
    /// The refusal, carrying [`TAGGING_NOT_FOUND_MESSAGE`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            message: TAGGING_NOT_FOUND_MESSAGE.to_string(),
        }
    }
}

impl Default for TaggingNotFound {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TaggingNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{TAGGING_NOT_FOUND_CODE}: {}", self.message)
    }
}

impl std::error::Error for TaggingNotFound {}

impl ::aws_smithy_schema::serde::SerializableStruct for TaggingNotFound {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> Result<(), ::aws_smithy_schema::serde::SerdeError> {
        serializer.write_string(&TAGGING_NOT_FOUND_MESSAGE_MEMBER, self.message.as_str())?;
        Ok(())
    }
}

/// What an operation keyed by a `deploymentId` reports for an id this service holds no
/// deployment for.
///
/// Stated once for the whole deployment surface: the refusal is the modelled 404
/// carrying [`DEPLOYMENT_NOT_FOUND_MESSAGE`], the id the caller asked about as its
/// `resourceId` and the kind that id addressed as its `resourceType`.
pub fn deployment_not_found(deployment_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: DEPLOYMENT_NOT_FOUND_MESSAGE.to_string(),
        resource_id: deployment_id.to_string(),
        resource_type: NamedResource::Deployment {
            deployment_id: deployment_id.to_string(),
        }
        .resource_type()
        .to_string(),
    }
}

/// What `CreateDeployment` reports for a target the plane that owns IoT things and thing
/// groups holds nothing under.
///
/// The modelled `ValidationException` (HTTP 400) carrying
/// [`TARGET_NOT_FOUND_MESSAGE`], and nothing else: the refusal is one fixed sentence, so
/// it states no `reason` and names no field.
pub fn target_not_found() -> ValidationException {
    ValidationException {
        message: TARGET_NOT_FOUND_MESSAGE.to_string(),
        reason: None,
        fields: None,
    }
}

/// What `DeleteDeployment` reports for a deployment that is still being applied: the
/// modelled `ValidationException` (HTTP 400) carrying
/// [`DEPLOYMENT_MUST_BE_CANCELED_MESSAGE`].
///
/// One fixed sentence, like every other refusal on this surface: it names no value of the
/// request, so the refusal reports neither the id nor the status.
pub fn deployment_must_be_canceled() -> ValidationException {
    ValidationException {
        message: DEPLOYMENT_MUST_BE_CANCELED_MESSAGE.to_string(),
        reason: None,
        fields: None,
    }
}

/// The sentence [`deployment_must_be_canceled`] carries, which is the rule the operation
/// documents: "To delete an active deployment, you must first cancel it"
/// (`DeleteDeployment`'s own documentation).
const DEPLOYMENT_MUST_BE_CANCELED_MESSAGE: &str =
    "Invalid Input: Greengrass deployment must be canceled before deletion";

/// The account every resource this emulator serves belongs to, as the messages that
/// name an account report it.
pub fn account_id() -> &'static str {
    ::emulator_server::emulator_account_id()
}

/// A fresh opaque identifier for a deployment or its IoT job.
pub fn new_identifier() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// Order two component versions the way Greengrass does: by semantic-version
/// precedence (the recipe reference requires `ComponentVersion` to be a semantic
/// version), falling back to a byte comparison when one will not parse.
pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    match (::semver::Version::parse(a), ::semver::Version::parse(b)) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => a.cmp(b),
    }
}

/// A `SystemTime` as the `Timestamp` the wire carries.
pub fn to_datetime(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = instant
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    ::aws_smithy_types::DateTime::from_millis(i64::try_from(millis).unwrap_or(i64::MAX))
}

/// A `SystemTime` as the instant a STRING timestamp member reports.
///
/// The service role association's `associatedAt` and `disassociatedAt` are modelled as
/// plain strings rather than as `timestamp`
/// (`smithy_query(query="operation",operation="GetServiceRoleForAccount")`), so the
/// framework renders nothing for them and the handler states the form: the RFC 3339
/// instant in UTC that an AWS `restJson1` timestamp carries, to whole seconds.
pub fn to_timestamp_text(instant: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from_secs(epoch_seconds(instant))
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// A `SystemTime` in epoch SECONDS, which is how the shared IoT stores timestamp a
/// record — so the deployment and the IoT job it creates are stamped from one instant
/// rather than from two clock reads.
pub fn epoch_seconds(instant: SystemTime) -> i64 {
    let seconds = instant
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    i64::try_from(seconds).unwrap_or(i64::MAX)
}

/// An optional tag map for a response: Greengrass reports the tag set of a resource
/// with no tags as an EMPTY map rather than omitting the member, so a client reading
/// `tags` always finds one.
pub fn tags_out(tags: &BTreeMap<String, String>) -> Option<std::collections::HashMap<String, String>> {
    Some(tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

/// Collect a request's tag map into the ordered map state keeps.
pub fn tags_in(tags: Option<&std::collections::HashMap<String, String>>) -> BTreeMap<String, String> {
    tags.map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Pagination cursors
// ---------------------------------------------------------------------------

/// Encode the key a page resumes at into an opaque `nextToken`.
pub fn encode_cursor(key: &str) -> String {
    ::hex::encode(key.as_bytes())
}

/// Decode a `nextToken` back into the key its page resumes at.
pub fn decode_cursor(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

// ---------------------------------------------------------------------------
// Component recipes
// ---------------------------------------------------------------------------

/// The parts of a component recipe the control plane reports, and the recipe
/// document it stores.
#[derive(Debug, Clone)]
pub struct ParsedRecipe {
    pub component_name: String,
    pub component_version: String,
    pub description: Option<String>,
    pub publisher: Option<String>,
    pub platforms: Vec<ComponentPlatform>,
    pub format: RecipeFormat,
    /// The recipe document to STORE for this version: the submitted document plus
    /// the service-assigned `ComponentType`, serialized in `format`. See
    /// [`normalized_recipe`].
    pub stored_recipe: Vec<u8>,
}

/// Why a submitted recipe was refused.
#[derive(Debug, Clone)]
pub enum RecipeError {
    /// The bytes are not a recipe document at all.
    Unparseable(String),
    /// The document parsed but omits a key the recipe reference requires.
    MissingKey(&'static str),
    /// The document breaches a bound this emulator puts on an untrusted recipe.
    OutOfBounds(String),
}

/// Pre-flight bounds on an untrusted recipe document, applied BEFORE any parser
/// builds a tree from it (see [`MAX_RECIPE_BYTES`] for why).
///
/// This is a GUARD, not a parser, and it deliberately knows NOTHING about YAML
/// context: it examines every byte and skips nothing for quoting, comments, tags
/// or escapes. Trying to decide whether a byte is "structural" is what makes a
/// hand-rolled scanner bypassable — `'` inside a plain scalar, `#` after a
/// non-space, a tag before an alias — so instead the guard measures only
/// quantities that a hiding trick can push UP but never DOWN:
///
/// * the document's length ([`MAX_RECIPE_BYTES`]);
/// * the total number of nesting-OPENING indicators `[`, `{`, `-`, `?`
///   ([`MAX_RECIPE_STRUCTURAL_INDICATORS`]);
/// * the number of FLOW openers `[`, `{` ([`MAX_RECIPE_FLOW_OPENERS`]) — flow
///   nesting costs neither a column nor a line, so its opener count IS its depth
///   bound. Closers are deliberately never subtracted, so a `]` hidden in a scalar
///   cannot buy an attacker depth back;
/// * the number of COMPACT indicators `-`, `?` on any ONE line
///   ([`MAX_RECIPE_COMPACT_PER_LINE`]), which is where a compact entry
///   (`- - - x`) can nest without costing a column;
/// * the leading indentation of any line ([`MAX_RECIPE_INDENT`]), which is what
///   block nesting across lines costs.
///
/// Together the last three bound how deep the recursive parser can descend while
/// there is still no tree to measure.
///
/// It does NOT try to spot anchors or aliases: an alias is refused BEFORE the
/// load by [`reject_aliases`], where the parser's own event stream names one
/// unambiguously, rather than by a byte that might equally be a shell `&&` or a
/// `*` glob inside a Lifecycle command.
///
/// Over-counting is the only failure mode, and every bound is set far above what a
/// real recipe needs.
fn guard_recipe(text: &str) -> Result<(), RecipeError> {
    if text.len() > MAX_RECIPE_BYTES {
        return Err(RecipeError::OutOfBounds(format!(
            "the recipe is {} bytes; this emulator parses at most {MAX_RECIPE_BYTES}",
            text.len()
        )));
    }

    let mut openers = 0usize;
    let mut flow_openers = 0usize;
    let mut compact_on_line = 0usize;
    let mut indent = 0usize;
    let mut counting_indent = true;

    for byte in text.bytes() {
        match byte {
            b'\n' | b'\r' => {
                indent = 0;
                compact_on_line = 0;
                counting_indent = true;
            }
            b' ' | b'\t' if counting_indent => {
                indent += 1;
                if indent > MAX_RECIPE_INDENT {
                    return Err(RecipeError::OutOfBounds(format!(
                        "a recipe line must not be indented past {MAX_RECIPE_INDENT} columns"
                    )));
                }
            }
            b'[' | b'{' | b'-' | b'?' => {
                counting_indent = false;
                openers += 1;
                if openers > MAX_RECIPE_STRUCTURAL_INDICATORS {
                    return Err(RecipeError::OutOfBounds(format!(
                        "a recipe must not carry more than \
                         {MAX_RECIPE_STRUCTURAL_INDICATORS} nesting indicators"
                    )));
                }
                if byte == b'[' || byte == b'{' {
                    flow_openers += 1;
                    if flow_openers > MAX_RECIPE_FLOW_OPENERS {
                        return Err(RecipeError::OutOfBounds(format!(
                            "a recipe must not open more than {MAX_RECIPE_FLOW_OPENERS} \
                             flow collections"
                        )));
                    }
                } else {
                    compact_on_line += 1;
                    if compact_on_line > MAX_RECIPE_COMPACT_PER_LINE {
                        return Err(RecipeError::OutOfBounds(format!(
                            "a recipe line must not carry more than \
                             {MAX_RECIPE_COMPACT_PER_LINE} '-' or '?' indicators"
                        )));
                    }
                }
            }
            _ => counting_indent = false,
        }
    }
    Ok(())
}

/// Refuses a recipe that uses a YAML alias, BEFORE the loader can expand one.
///
/// This cannot be done on the loaded tree: `yaml-rust2`'s loader RESOLVES an alias
/// by copying the anchored node, so by the time a tree exists the expansion has
/// already happened (an 8-level, few-hundred-byte alias chain takes minutes). It
/// also cannot be done by scanning bytes without refusing a Lifecycle command
/// that contains a shell `&&` or a `*` glob. So it is done with the parser's own
/// event stream, which reports an alias as `Event::Alias` and copies nothing: the
/// document is parsed once for this check, and only an alias-free document is ever
/// handed to the loader.
///
/// An anchor that no alias refers to expands nothing, and is accepted.
fn reject_aliases(text: &str) -> Result<(), RecipeError> {
    #[derive(Default)]
    struct AliasDetector {
        saw_alias: bool,
    }
    impl ::yaml_rust2::parser::EventReceiver for AliasDetector {
        fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
            if matches!(event, ::yaml_rust2::parser::Event::Alias(_)) {
                self.saw_alias = true;
            }
        }
    }

    let mut detector = AliasDetector::default();
    ::yaml_rust2::parser::Parser::new_from_str(text)
        .load(&mut detector, true)
        .map_err(|e| RecipeError::Unparseable(format!("the recipe could not be parsed: {e}")))?;
    if detector.saw_alias {
        return Err(RecipeError::OutOfBounds(
            "a recipe must not use YAML aliases".to_string(),
        ));
    }
    Ok(())
}

/// Check a loaded recipe tree against the bound only a tree can show: that it
/// nests within [`MAX_RECIPE_DEPTH`], which is what the recursive renderers rely
/// on. Walked with an EXPLICIT stack, so the check cannot itself recurse;
/// [`guard_recipe`] is what keeps the loader's own recursion in range long enough
/// to get a tree to measure.
fn document_within_bounds(root: &::yaml_rust2::Yaml) -> Result<(), RecipeError> {
    use ::yaml_rust2::Yaml;
    let mut pending = vec![(root, 1usize)];
    while let Some((node, depth)) = pending.pop() {
        if depth > MAX_RECIPE_DEPTH {
            return Err(RecipeError::OutOfBounds(format!(
                "a recipe must not nest past {MAX_RECIPE_DEPTH} levels"
            )));
        }
        match node {
            Yaml::Array(items) => pending.extend(items.iter().map(|item| (item, depth + 1))),
            Yaml::Hash(map) => {
                for (key, value) in map {
                    pending.push((key, depth + 1));
                    pending.push((value, depth + 1));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Whether the document could be JSON at all: a JSON document's first significant
/// byte is `{` or `[`, while a Greengrass YAML recipe is a block mapping and so
/// starts with neither. Probing only those keeps a YAML recipe from being expanded
/// twice.
fn looks_like_json(text: &str) -> bool {
    matches!(text.trim_start().as_bytes().first(), Some(b'{' | b'['))
}

/// Parse a bounded recipe document into its YAML tree.
fn load_document(text: &str) -> Result<::yaml_rust2::Yaml, RecipeError> {
    guard_recipe(text)?;
    reject_aliases(text)?;
    let mut documents = ::yaml_rust2::YamlLoader::load_from_str(text)
        .map_err(|e| RecipeError::Unparseable(format!("the recipe could not be parsed: {e}")))?;
    if documents.len() != 1 {
        return Err(RecipeError::Unparseable(format!(
            "the recipe must be exactly one document; found {}",
            documents.len()
        )));
    }
    let document = documents.remove(0);
    document_within_bounds(&document)?;
    Ok(document)
}

/// Look a key up in a recipe mapping.
fn mapping_get<'a>(
    map: &'a ::yaml_rust2::yaml::Hash,
    key: &str,
) -> Option<&'a ::yaml_rust2::Yaml> {
    map.iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .map(|(_, v)| v)
}

/// A recipe scalar as the string the API reports it as.
fn scalar_string(value: &::yaml_rust2::Yaml) -> Option<String> {
    match value {
        ::yaml_rust2::Yaml::String(s) | ::yaml_rust2::Yaml::Real(s) => Some(s.clone()),
        ::yaml_rust2::Yaml::Boolean(b) => Some(b.to_string()),
        ::yaml_rust2::Yaml::Integer(i) => Some(i.to_string()),
        _ => None,
    }
}

/// Parse an `inlineRecipe` blob into the metadata the control plane derives from it.
///
/// A Greengrass recipe is a JSON or a YAML document. YAML 1.2 is a superset of
/// JSON, so the YAML parser reads both; JSON is probed separately — with a
/// non-allocating validate, not a second tree — only so `GetComponent` can report
/// which format the stored bytes are written in. The untrusted bytes pass
/// [`guard_recipe`] before any parser sees them.
pub fn parse_recipe(bytes: &[u8]) -> Result<ParsedRecipe, RecipeError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| RecipeError::Unparseable(format!("the recipe is not valid UTF-8: {e}")))?;
    let document = load_document(text)?;
    let format = if looks_like_json(text)
        && ::serde_json::from_str::<::serde::de::IgnoredAny>(text).is_ok()
    {
        RecipeFormat::Json
    } else {
        RecipeFormat::Yaml
    };
    let map = document.as_hash().ok_or_else(|| {
        RecipeError::Unparseable(
            "the recipe must be a mapping of recipe keys to values".to_string(),
        )
    })?;

    let component_name = mapping_get(map, "ComponentName")
        .and_then(scalar_string)
        .ok_or(RecipeError::MissingKey("ComponentName"))?;
    let component_version = mapping_get(map, "ComponentVersion")
        .and_then(scalar_string)
        .ok_or(RecipeError::MissingKey("ComponentVersion"))?;

    Ok(ParsedRecipe {
        component_name,
        component_version,
        description: mapping_get(map, "ComponentDescription").and_then(scalar_string),
        publisher: mapping_get(map, "ComponentPublisher").and_then(scalar_string),
        platforms: recipe_platforms(map),
        format,
        stored_recipe: normalized_recipe(map, format).ok_or_else(|| {
            RecipeError::Unparseable(
                "every member of a recipe must be named by a string".to_string(),
            )
        })?,
    })
}

/// The recipe document this service STORES for a version published from
/// `submitted`: that document with the members the service supplies, serialized in
/// `format`.
///
/// Greengrass does not keep the submitted bytes. It stores the recipe it derived
/// and serializes THAT document for every reader, which is why a recipe read back
/// as JSON comes back compact, carries a `ComponentType` member no caller
/// submitted, and carries the recipe members whose default is empty even when the
/// submitted document left them out — while every member the caller did submit
/// survives. Member order is the submitted document's, with `ComponentType` where
/// the recipe reference publishes it (immediately after `ComponentVersion`, which
/// [`parse_recipe`] has already required) and a supplied default appended to the
/// mapping that was missing it.
fn normalized_recipe(
    submitted: &::yaml_rust2::yaml::Hash,
    format: RecipeFormat,
) -> Option<Vec<u8>> {
    use ::yaml_rust2::Yaml;

    let type_key = Yaml::String(RECIPE_TYPE_KEY.to_string());
    let assigned = Yaml::String(COMPONENT_TYPE_GENERIC.to_string());
    let mut document = ::yaml_rust2::yaml::Hash::new();
    for (key, value) in submitted {
        // A submitted `ComponentType` does not decide the stored one: the service
        // assigns the type, so the member is rewritten wherever it appears.
        if key.as_str() == Some(RECIPE_TYPE_KEY) {
            continue;
        }
        document.insert(key.clone(), value.clone());
        if key.as_str() == Some(RECIPE_VERSION_KEY) {
            document.insert(type_key.clone(), assigned.clone());
        }
    }
    if !document.contains_key(&type_key) {
        document.insert(type_key, assigned);
    }
    supply_empty_defaults(&mut document);
    render_document(&Yaml::Hash(document), format)
}

/// Add the recipe members whose default is the EMPTY collection to a stored recipe
/// document that does not declare them: the recipe's own `Lifecycle`, and each
/// manifest's `Lifecycle` and `Artifacts`.
///
/// Greengrass answers a recipe read with all three whether or not the submitted
/// document carried them, so a client that reads `recipe['Lifecycle']` or iterates
/// `Manifests[i]['Artifacts']` always finds a collection to read. A member the
/// document DOES declare is left exactly as it was declared, empty or not.
fn supply_empty_defaults(document: &mut ::yaml_rust2::yaml::Hash) {
    use ::yaml_rust2::Yaml;

    let lifecycle = Yaml::String(RECIPE_LIFECYCLE_KEY.to_string());
    let artifacts = Yaml::String(RECIPE_ARTIFACTS_KEY.to_string());
    if let Some(Yaml::Array(manifests)) =
        document.get_mut(&Yaml::String(RECIPE_MANIFESTS_KEY.to_string()))
    {
        for manifest in manifests.iter_mut() {
            if let Yaml::Hash(manifest) = manifest {
                if !manifest.contains_key(&lifecycle) {
                    manifest.insert(lifecycle.clone(), Yaml::Hash(::yaml_rust2::yaml::Hash::new()));
                }
                if !manifest.contains_key(&artifacts) {
                    manifest.insert(artifacts.clone(), Yaml::Array(Vec::new()));
                }
            }
        }
    }
    if !document.contains_key(&lifecycle) {
        document.insert(lifecycle, Yaml::Hash(::yaml_rust2::yaml::Hash::new()));
    }
}

/// A recipe document serialized in `format` — the ONE place this service decides
/// how a stored recipe is written, so the bytes a create stores and the bytes a
/// cross-format read renders agree: COMPACT JSON (no space after `:` or `,`, as
/// the service answers `GetComponent` with) or the YAML emitter's block form.
fn render_document(document: &::yaml_rust2::Yaml, format: RecipeFormat) -> Option<Vec<u8>> {
    match format {
        RecipeFormat::Json => ::serde_json::to_vec(&OrderedDocument {
            node: document,
            budget: MAX_RECIPE_DEPTH,
        })
        .ok(),
        RecipeFormat::Yaml => {
            let mut rendered = String::new();
            ::yaml_rust2::YamlEmitter::new(&mut rendered)
                .dump(document)
                .ok()?;
            Some(rendered.into_bytes())
        }
    }
}

/// A recipe document as `serde_json` writes it, with every mapping's members in
/// the ORDER that mapping declares them.
///
/// `serde_json`'s `Value` object is a `BTreeMap`, so converting a recipe into one
/// would reorder its members; handing the document to the serializer through this
/// wrapper instead keeps `serde_json` responsible for every byte it writes — the
/// compact separators, the string escapes and the number spelling — and leaves
/// only the order to the document itself.
struct OrderedDocument<'a> {
    node: &'a ::yaml_rust2::Yaml,
    /// The recursion left: [`load_document`] already refused anything past
    /// [`MAX_RECIPE_DEPTH`], and this second, independent bound means a future
    /// caller that forgets that check still cannot drive the serializer into the
    /// stack.
    budget: usize,
}

impl ::serde::Serialize for OrderedDocument<'_> {
    fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use ::serde::ser::{Error, SerializeMap, SerializeSeq};
        use ::yaml_rust2::Yaml;

        let budget = self
            .budget
            .checked_sub(1)
            .ok_or_else(|| S::Error::custom("the recipe nests deeper than can be rendered"))?;
        match self.node {
            Yaml::Hash(map) => {
                let mut object = serializer.serialize_map(Some(map.len()))?;
                for (key, value) in map {
                    let key = key.as_str().ok_or_else(|| {
                        S::Error::custom("every member of a recipe must be named by a string")
                    })?;
                    object.serialize_entry(key, &OrderedDocument { node: value, budget })?;
                }
                object.end()
            }
            Yaml::Array(items) => {
                let mut sequence = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    sequence.serialize_element(&OrderedDocument { node: item, budget })?;
                }
                sequence.end()
            }
            scalar => scalar_as_json(scalar)
                .ok_or_else(|| S::Error::custom("a recipe scalar that cannot be rendered"))?
                .serialize(serializer),
        }
    }
}

/// The platforms a recipe declares: one `ComponentPlatform` per manifest that
/// carries a `Platform` mapping, whose `name` key is the platform's name and
/// whose remaining keys (`os`, `architecture`, …) are its attributes.
fn recipe_platforms(map: &::yaml_rust2::yaml::Hash) -> Vec<ComponentPlatform> {
    let Some(manifests) = mapping_get(map, "Manifests").and_then(::yaml_rust2::Yaml::as_vec) else {
        return Vec::new();
    };
    manifests
        .iter()
        .filter_map(|manifest| {
            let platform = mapping_get(manifest.as_hash()?, "Platform")?.as_hash()?;
            let mut name = None;
            let mut attributes = std::collections::HashMap::new();
            for (key, value) in platform {
                let (Some(key), Some(value)) = (key.as_str(), scalar_string(value)) else {
                    continue;
                };
                if key == "name" {
                    name = Some(value);
                } else {
                    attributes.insert(key.to_string(), value);
                }
            }
            Some(ComponentPlatform {
                name,
                attributes: (!attributes.is_empty()).then_some(attributes),
            })
        })
        .collect()
}

/// A recipe SCALAR as the JSON value that renders it.
///
/// Only scalars: the collections are written by [`write_compact_json`], which is
/// what keeps a mapping's members in the order the document declares them.
fn scalar_as_json(scalar: &::yaml_rust2::Yaml) -> Option<::serde_json::Value> {
    use ::serde_json::Value;
    use ::yaml_rust2::Yaml;
    Some(match scalar {
        Yaml::Null | Yaml::BadValue => Value::Null,
        Yaml::Boolean(b) => Value::Bool(*b),
        Yaml::Integer(i) => Value::Number((*i).into()),
        Yaml::Real(r) => r
            .parse::<f64>()
            .ok()
            .and_then(::serde_json::Number::from_f64)
            .map_or_else(|| Value::String(r.clone()), Value::Number),
        Yaml::String(s) => Value::String(s.clone()),
        // A collection is the caller's to write, and an alias is refused before
        // the load by `reject_aliases`, so neither can reach here.
        Yaml::Array(_) | Yaml::Hash(_) | Yaml::Alias(_) => return None,
    })
}

/// The recipe bytes to answer `GetComponent` with, in the format the caller asked
/// for: the stored bytes verbatim when that is already the format they are written
/// in, and the same document rendered in the other format otherwise.
///
/// The stored bytes already passed [`guard_recipe`] when they were published; the
/// guard runs again here (through [`load_document`]) because re-rendering
/// re-expands them, and a bound that is only checked on the write path is one
/// refactor away from being skipped.
pub fn render_recipe(
    bytes: &[u8],
    stored: RecipeFormat,
    requested: RecipeFormat,
) -> Option<Vec<u8>> {
    if stored == requested {
        return Some(bytes.to_vec());
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let document = load_document(text).ok()?;
    render_document(&document, requested)
}
