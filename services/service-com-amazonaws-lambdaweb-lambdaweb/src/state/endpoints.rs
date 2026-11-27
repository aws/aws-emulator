// SPDX-License-Identifier: CC0-1.0

//! The endpoint slice of this service's state.
//!
//! An endpoint belongs to exactly one web function, so a stored [`Endpoint`] lives
//! inside that function's record (`WebFunction::endpoints`) and a function's delete
//! takes its endpoints with it. Everything the endpoint reads report — the ARN, the
//! domain names, the Regions, the revision weights, the state — is stored here once
//! and projected into the modelled shapes by [`Endpoint::summary`] and
//! [`Endpoint::regional_endpoints`], so `CreateWebFunction`'s initial endpoint and
//! `GetWebFunctionEndpoint` report ONE copy of each endpoint.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    FunctionEndpointSummary, RegionalEndpoint, RevisionWeight, ScalingConfig, ThrottleConfig,
};

use super::{
    resource_name_violation, ComAmazonawsLambdawebLambdawebState, RevisionState, WebFunction,
    PROVISIONING_DELAY,
};

/// The longest `endpointName` the service accepts.
///
/// The model constrains `EndpointName` to 1..256, but the member's own
/// documentation states the narrower rule: "The name can contain letters, numbers,
/// hyphens (-), and underscores (_), and can't begin or end with a hyphen or an
/// underscore. The name is limited to 64 characters in length." (source:
/// `smithy_query(query="doc", shape="EndpointConfig", member="endpointName")`).
pub const ENDPOINT_NAME_MAX_CHARS: usize = 64;

/// `stateReason` while an endpoint is still being created.
pub const ENDPOINT_PENDING_REASON: &str = "The endpoint is being created.";
/// `stateReason` once an endpoint is ready to receive traffic.
pub const ENDPOINT_ACTIVE_REASON: &str = "The endpoint is active.";

/// The weight a lone revision carries on a fresh endpoint: all of the traffic
/// (`RevisionWeight$weight` is `@range(1..100)`, so 100 is "everything").
pub const FULL_WEIGHT: i32 = 100;

/// The `autoDeploymentMode` an endpoint takes when the request names none: "If you
/// don't specify a value, the default is `Disabled`, and this default is returned
/// in the response." (source: `smithy_query(query="doc", shape="EndpointConfig",
/// member="autoDeploymentMode")`).
pub const DEFAULT_AUTO_DEPLOYMENT_MODE: &str = "Disabled";

/// The `autoDeploymentMode` under which "the endpoint automatically serves the newest
/// revision" (`AutoDeploymentMode`), which is what
/// [`super::WebFunction::deploy_latest_revision`] acts on.
pub const AUTO_DEPLOYMENT_MODE_LATEST_REVISION: &str = "LatestRevision";

/// The `authType` under which "Lambda uses IAM to authorize requests, which must be
/// signed with Signature Version 4 (SigV4)" (`AuthType`) — the one the published data
/// plane refuses an unsigned request on ([`super::endpoint_plane`]).
pub const AUTH_TYPE_IAM: &str = "IamAuth";

/// The `authType` under which "your application handles authorization" (`AuthType`), so
/// the published data plane refuses nothing itself.
pub const AUTH_TYPE_APPLICATION_MANAGED: &str = "ApplicationManaged";

/// The `updateStatus` an endpoint carries while the configuration change it was just
/// given is being applied: "InProgress – The update is in progress."
/// (`EndpointUpdateStatus`).
pub const ENDPOINT_UPDATE_IN_PROGRESS: &str = "InProgress";

/// The `updateStatus` an endpoint carries once that change has been applied:
/// "Successful – The update completed successfully." (`EndpointUpdateStatus`).
pub const ENDPOINT_UPDATE_SUCCESSFUL: &str = "Successful";

/// `updateStatusReason` while an endpoint's update is still in progress.
pub const ENDPOINT_UPDATE_IN_PROGRESS_REASON: &str = "The endpoint update is in progress.";

/// `updateStatusReason` once an endpoint's update has completed.
pub const ENDPOINT_UPDATE_SUCCESSFUL_REASON: &str =
    "The endpoint update completed successfully.";

/// The `endpointType` that serves traffic only from the Region the function was
/// created in.
pub const ENDPOINT_TYPE_HOME_REGION: &str = "HomeRegion";
/// The `endpointType` that replicates across Regions behind one domain name.
pub const ENDPOINT_TYPE_MULTI_REGION: &str = "MultiRegion";
/// The `endpointType` that gives every configured Region its own endpoint.
pub const ENDPOINT_TYPE_PER_REGION: &str = "PerRegion";

// Where an endpoint's published `domainName` comes from: the ADDRESS this emulator answers
// that endpoint's own HTTP requests on (`super::endpoint_plane`), as the `host:port` a
// client dials.
//
// It is NOT a name in the shape AWS's own endpoint domains take, and it does NOT satisfy
// the member's modelled `@pattern` (`^([a-z0-9-]+\.)+[a-z]+$`) — a deliberate deviation,
// stated here so a client-side validator failure is traceable to this decision. That shape
// is a property of the environment rather than a requirement of the API: AWS serves the
// endpoint at port 443 under a zone AWS runs, and what a client DOES with the value is dial
// it. A name under a zone this emulator does not serve resolves to nothing on the machine
// it runs on and publishes a data plane no client can reach, so the address this emulator
// answers on is the one value that keeps the member's documented meaning — "the domain name
// assigned to the endpoint", the address the endpoint is served at. The member's modelled
// LENGTH bound (4..256) is satisfied, and the tests pin it.

/// The `EndpointState` of a stored endpoint: the model's enum values
/// (`ENDPOINT_STATE_VALUES` in `ops::types`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointState {
    Pending,
    Active,
    Failed,
    Deleting,
}

impl EndpointState {
    /// This state as the model's enum value.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Active => "Active",
            Self::Failed => "Failed",
            Self::Deleting => "Deleting",
        }
    }
}

/// What this service stores about ONE Region of an endpoint.
///
/// `domain_name` is the domain that Region answers on, and is `None` for an
/// endpoint type where a single domain serves every Region — the member is
/// optional on the modelled `RegionalEndpoint` for exactly that case.
#[derive(Debug, Clone)]
pub struct EndpointRegion {
    pub domain_name: Option<String>,
}

/// An optional endpoint configuration shape whose value is what makes it a
/// configuration at all: it carries a single optional member, and the shape sets
/// nothing when that member is absent.
///
/// Implemented for `ScalingConfig` and `ThrottleConfig`, whose member documentation
/// states the rule [`configured`] applies (source: `smithy_query(query="shape",
/// shape="ScalingConfig,ThrottleConfig")`).
pub trait EndpointConfiguration {
    /// Whether this shape names a configuration, rather than setting none of its
    /// members.
    fn is_configured(&self) -> bool;
}

impl EndpointConfiguration for ScalingConfig {
    fn is_configured(&self) -> bool {
        self.max_environments.is_some()
    }
}

impl EndpointConfiguration for ThrottleConfig {
    fn is_configured(&self) -> bool {
        self.rate_limit.is_some()
    }
}

/// `requested` as an endpoint stores it: a configuration that sets none of its own
/// members is no configuration, so it is stored — and therefore reported — as absent.
///
/// "There is no default value. If you don't specify a value, the scaling configuration
/// is absent from the response." (source: `smithy_query(query="shape",
/// shape="ScalingConfig")`, member `maxEnvironments`, and the same sentence for
/// `ThrottleConfig$rateLimit`), which `FunctionEndpointSummary$scalingConfig` repeats
/// for the readers: "This field is absent if the endpoint has no scaling
/// configuration." An empty object therefore means one thing on EVERY write path — the
/// endpoint has no such configuration — whether it arrives on a create or as the
/// documented clear of a previously set value on an update.
pub fn configured<C: EndpointConfiguration>(requested: Option<C>) -> Option<C> {
    requested.filter(EndpointConfiguration::is_configured)
}

/// One stored endpoint of a web function.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub endpoint_name: String,
    pub endpoint_arn: String,
    pub description: Option<String>,
    pub endpoint_type: String,
    /// The domain the endpoint as a whole answers on.
    pub domain_name: String,
    pub auth_type: String,
    pub auto_deployment_mode: String,
    /// Which revisions the endpoint serves, and with how much of the traffic.
    pub revision_weights: Vec<RevisionWeight>,
    /// The Regions the endpoint is configured for, keyed by Region so the listing
    /// is stable and no Region is stored twice (`RegionList` is `@uniqueItems`).
    pub regions: BTreeMap<String, EndpointRegion>,
    pub scaling_config: Option<ScalingConfig>,
    pub throttle_config: Option<ThrottleConfig>,
    pub state: EndpointState,
    pub state_reason: String,
    pub update_status: Option<String>,
    pub update_status_reason: Option<String>,
    /// Fixed at creation and never moved again.
    pub created_at: SystemTime,
    /// Bumped by every mutation of this endpoint, and by the clock-driven
    /// `Pending -> Active` transition.
    pub updated_at: SystemTime,
    /// The instant the clock owes this endpoint its `Pending -> Active`
    /// transition, or `None` once it has been materialised.
    pub activates_at: Option<SystemTime>,
    /// The instant the clock owes this endpoint the END of the update it was last
    /// given — its `updateStatus` moving `InProgress -> Successful` — or `None` when
    /// no update of it is outstanding.
    pub update_completes_at: Option<SystemTime>,
}

impl Endpoint {
    /// The Regions this endpoint is configured for, as the modelled `RegionList`.
    pub fn region_list(&self) -> Vec<String> {
        self.regions.keys().cloned().collect()
    }

    /// This endpoint as the `FunctionEndpointSummary` a create reports.
    pub fn summary(&self) -> FunctionEndpointSummary {
        FunctionEndpointSummary {
            endpoint_arn: self.endpoint_arn.clone(),
            endpoint_name: self.endpoint_name.clone(),
            description: self.description.clone(),
            endpoint_type: self.endpoint_type.clone(),
            domain_name: self.domain_name.clone(),
            auth_type: self.auth_type.clone(),
            auto_deployment_mode: self.auto_deployment_mode.clone(),
            revision_weights: self.revision_weights.clone(),
            regions: self.region_list(),
            scaling_config: self.scaling_config.clone(),
            throttle_config: self.throttle_config.clone(),
            state: self.state.wire().to_string(),
            state_reason: self.state_reason.clone(),
            update_status: self.update_status.clone(),
            update_status_reason: self.update_status_reason.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }

    /// This endpoint's per-Region configuration and state, as the modelled
    /// `RegionalEndpoints` map: one entry per configured Region, each reporting the
    /// same stored configuration the endpoint carries.
    pub fn regional_endpoints(&self) -> HashMap<String, RegionalEndpoint> {
        self.regions
            .iter()
            .map(|(region, stored)| {
                (
                    region.clone(),
                    RegionalEndpoint {
                        domain_name: stored.domain_name.clone(),
                        auth_type: self.auth_type.clone(),
                        revision_weights: self.revision_weights.clone(),
                        scaling_config: self.scaling_config.clone(),
                        throttle_config: self.throttle_config.clone(),
                        state: self.state.wire().to_string(),
                        state_reason: self.state_reason.clone(),
                        update_status: self.update_status.clone(),
                        update_status_reason: self.update_status_reason.clone(),
                    },
                )
            })
            .collect()
    }

    /// The revision ONE request to this endpoint routes to, chosen by the weights the
    /// endpoint serves its revisions with, with `pick` the request's own draw.
    ///
    /// `RevisionWeight$weight` is `@range(1..100)` and an endpoint carries one entry per
    /// revision it serves, so the weights are the shares of the traffic: a request falls in
    /// the share whose cumulative weight its draw lands in. `pick` is the DRAW — a value of
    /// the request's own, never a counter this store keeps — so serving a request changes
    /// nothing a later caller sees, and an endpoint serving one revision routes every
    /// request to it whatever the draw.
    ///
    /// `None` for an endpoint serving no revision, which has nothing to route to.
    #[must_use]
    pub fn routed_revision(&self, pick: u64) -> Option<&str> {
        let total: u64 = self
            .revision_weights
            .iter()
            .map(|weight| u64::try_from(weight.weight).unwrap_or_default())
            .sum();
        if total == 0 {
            // Every share is empty (or there are none), so there is no revision this
            // endpoint serves a share of its traffic.
            return None;
        }
        let mut landed = pick % total;
        for weight in &self.revision_weights {
            let share = u64::try_from(weight.weight).unwrap_or_default();
            if landed < share {
                return Some(&weight.revision_id);
            }
            landed -= share;
        }
        // The draw is below the total, so some share always holds it; the last share is the
        // one that does if the arithmetic above ever left none.
        self.revision_weights
            .last()
            .map(|weight| weight.revision_id.as_str())
    }

    /// Apply the `Pending -> Active` transition if the clock has already made it
    /// due. Owed at [`Self::activates_at`] whether or not anybody called, so
    /// materialising it on a read reports what was already true. `updated_at` moves
    /// to the instant the transition was DUE, so the timestamp a caller reads is
    /// stable across reads.
    pub(super) fn materialise_due(&mut self, now: SystemTime) {
        if self.state == EndpointState::Pending {
            if let Some(due) = self.activates_at.filter(|due| now >= *due) {
                self.state = EndpointState::Active;
                self.state_reason = ENDPOINT_ACTIVE_REASON.to_string();
                self.updated_at = due;
                self.activates_at = None;
            }
        }
        self.materialise_due_update(now);
    }

    /// Settle the update this endpoint was last given if the clock has already made
    /// its end due: `updateStatus` moves to `Successful`. Owed at
    /// [`Self::update_completes_at`] whether or not anybody called, and `updated_at`
    /// moves to the instant it was DUE, so what a caller reads is stable across reads.
    fn materialise_due_update(&mut self, now: SystemTime) {
        if let Some(due) = self.update_completes_at.filter(|due| now >= *due) {
            self.update_status = Some(ENDPOINT_UPDATE_SUCCESSFUL.to_string());
            self.update_status_reason = Some(ENDPOINT_UPDATE_SUCCESSFUL_REASON.to_string());
            self.updated_at = self.updated_at.max(due);
            self.update_completes_at = None;
        }
    }

    /// Apply `update` to this endpoint: every member the request named takes its new
    /// value and every member it omitted keeps the stored one ("Omit this field to keep
    /// the current …", stated by each updatable member of
    /// `UpdateWebFunctionEndpointRequest`).
    ///
    /// The configuration change is asynchronous — the operation answers `202` — so the
    /// endpoint reports its update `InProgress` and the clock owes it `Successful` one
    /// [`PROVISIONING_DELAY`] later, which is never zero, so a caller reading straight
    /// back still sees the transient status.
    fn apply(&mut self, update: EndpointUpdate, now: SystemTime) {
        if let Some(description) = update.description {
            self.description = Some(description);
        }
        if let Some(auth_type) = update.auth_type {
            self.auth_type = auth_type;
        }
        if let Some(auto_deployment_mode) = update.auto_deployment_mode {
            self.auto_deployment_mode = auto_deployment_mode;
        }
        if let Some(revision_weights) = update.revision_weights {
            self.revision_weights = revision_weights;
        }
        // "On an update, omit `scalingConfig` to keep the current value, or specify an
        // empty object to clear a previously set value." (source: `smithy_query(
        // query="doc", shape="ScalingConfig", member="maxEnvironments")`, and the same
        // sentence for `ThrottleConfig$rateLimit`), so a configuration that sets none of
        // its members clears the stored one rather than storing an empty shape.
        if let Some(scaling_config) = update.scaling_config {
            self.scaling_config = configured(Some(scaling_config));
        }
        if let Some(throttle_config) = update.throttle_config {
            self.throttle_config = configured(Some(throttle_config));
        }
        self.updated_at = now;
        self.update_status = Some(ENDPOINT_UPDATE_IN_PROGRESS.to_string());
        self.update_status_reason = Some(ENDPOINT_UPDATE_IN_PROGRESS_REASON.to_string());
        self.update_completes_at = Some(now + PROVISIONING_DELAY);
    }
}

/// The configuration change `UpdateWebFunctionEndpoint` was asked for: one member per
/// member of its request that modifies the stored endpoint, `None` for a member the
/// request omitted.
///
/// A `scaling_config` or `throttle_config` that sets none of its own members is the
/// documented CLEAR of a previously set value, not an absent member — which is why the
/// two are carried as the modelled shapes rather than as their inner values.
#[derive(Debug, Clone, Default)]
pub struct EndpointUpdate {
    pub description: Option<String>,
    pub auth_type: Option<String>,
    pub auto_deployment_mode: Option<String>,
    pub revision_weights: Option<Vec<RevisionWeight>>,
    pub scaling_config: Option<ScalingConfig>,
    pub throttle_config: Option<ThrottleConfig>,
}

/// Why a requested `revisionWeights` list is not one an endpoint of a web function can
/// serve. Two different refusals, kept apart because they are different classes: the
/// caller answers [`Self::UnknownRevision`] with the modelled `ResourceNotFoundException`
/// and [`Self::RepeatedRevision`] with a `ValidationException`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionWeightProblem {
    /// A weight names a revision the function does not have.
    UnknownRevision { revision_id: String },
    /// Two weights name ONE revision.
    RepeatedRevision { revision_id: String },
}

impl RevisionWeightProblem {
    /// The refusal's message, worded once here so every operation that applies the rule
    /// reports it identically.
    pub fn message(&self, function_name: &str) -> String {
        match self {
            Self::UnknownRevision { revision_id } => {
                super::revision_not_found_message(revision_id, function_name)
            }
            Self::RepeatedRevision { revision_id } => format!(
                "The revisionWeights parameter names revision {revision_id} more than once: each \
                 entry must name a different revision."
            ),
        }
    }
}

/// Why `requested` is not a set of revision weights an endpoint of `function` can serve,
/// or `None` when it is.
///
/// A named revision has to be one of the function's own: an endpoint "maps a revision to
/// a weight", so a weight naming no stored revision names nothing. The list is
/// `@uniqueItems`, which refuses two identical ENTRIES at the boundary but not two
/// weights for one revision, and "you can assign weights to up to two revisions"
/// (`RevisionWeight`) — one revision twice is not two revisions.
pub fn revision_weight_problem(
    function: &WebFunction,
    requested: &[RevisionWeight],
) -> Option<RevisionWeightProblem> {
    let mut named: Vec<&str> = Vec::with_capacity(requested.len());
    for weight in requested {
        if !function.revisions.contains_key(&weight.revision_id) {
            return Some(RevisionWeightProblem::UnknownRevision {
                revision_id: weight.revision_id.clone(),
            });
        }
        if named.contains(&weight.revision_id.as_str()) {
            return Some(RevisionWeightProblem::RepeatedRevision {
                revision_id: weight.revision_id.clone(),
            });
        }
        named.push(&weight.revision_id);
    }
    None
}

/// Why `function`'s newest revision is not one an endpoint can be switched onto, or
/// `None` when it is. The caller answers `Some` with the modelled `ConflictException`.
///
/// An endpoint in the [`AUTO_DEPLOYMENT_MODE_LATEST_REVISION`] mode "automatically serves
/// the newest revision" (`AutoDeploymentMode`), and only a revision the service reports
/// `Active` is "ready to serve traffic" (`RevisionState`) — so putting an endpoint into
/// that mode while the newest revision is still building, or while its build failed,
/// would route the endpoint's traffic onto a revision that cannot take it: a conflict
/// with the current state of the resource rather than a change the service can make.
///
/// A function with no revision at all has nothing for the mode to switch the endpoint
/// ONTO, so there is no conflict to report: the endpoint keeps serving the revisions its
/// `@required` `revisionWeights` already name.
///
/// The refusal names the LATEST revision and the state it is in, states that it cannot
/// be deployed, states the rule that only `Active` revisions can be deployed, and tells
/// the caller what to do about it — create a new revision. The revision id and the state
/// word are the store's own; no other resource is named, and the model's general
/// `ConflictException` sentence is not what this class is worded with.
pub fn latest_revision_switch_conflict(function: &WebFunction) -> Option<String> {
    let newest = function.latest_revision()?;
    if newest.state == RevisionState::Active {
        return None;
    }
    let revision_id = &newest.revision_id;
    let state = newest.state.wire();
    Some(format!(
        "The latest revision {revision_id} is in {state} state and cannot be deployed. Only \
         revisions in Active state can be deployed. Create a new revision."
    ))
}

/// The endpoint a create was asked for, as the two create paths supply it: the
/// members of the modelled `EndpointConfig`.
#[derive(Debug, Clone)]
pub struct NewEndpoint {
    pub endpoint_name: String,
    pub description: Option<String>,
    pub endpoint_type: String,
    pub auth_type: String,
    pub auto_deployment_mode: Option<String>,
    pub regions: Option<Vec<String>>,
    pub scaling_config: Option<ScalingConfig>,
    pub throttle_config: Option<ThrottleConfig>,
    /// The revisions the endpoint serves when it is created.
    pub revision_weights: Vec<RevisionWeight>,
}

/// The ARN of an endpoint, derived from its FUNCTION's stored ARN so the two always
/// name the same function, Region and account:
/// `<functionArn>/endpoint/<endpointName>`. `EndpointArn` is modelled with a length
/// bound only (1..2048) and no pattern, so this layout mirrors the operation's own
/// URI (`/web-functions/{functionName}/endpoints/{endpointName}`), exactly as
/// [`super::revision_arn`] mirrors the revision one.
pub fn endpoint_arn(function_arn: &str, endpoint_name: &str) -> String {
    format!("{function_arn}/endpoint/{endpoint_name}")
}

/// The `endpointName` a request names, resolved to the key the store holds.
///
/// The member's documentation says the caller "can specify either of the
/// following: The endpoint name, for example my-endpoint. The endpoint ARN."
/// (source: `smithy_query(query="doc", shape="GetWebFunctionEndpointRequest",
/// member="endpointName")`), so a value in the ARN form resolves to the name in its
/// `/endpoint/<name>` segment. Anything else is already a name.
pub fn resolve_endpoint_name(value: &str) -> &str {
    if !value.starts_with("arn:") {
        return value;
    }
    value
        .split_once("/endpoint/")
        .map_or(value, |(_, name)| name)
}

/// AWS's `ResourceNotFoundException` message for an endpoint a web function does not
/// carry, naming it by `identifier` — the value the request addressed it by, verbatim,
/// whether that was the name or the ARN.
///
/// The model fixes the status (404), the code and `message` being `@required`, but not
/// the text, so the sentence is this service's behaviour — worded once here, so every
/// operation that resolves an endpoint reports the absent endpoint alike.
pub fn endpoint_not_found_message(identifier: &str, function_name: &str) -> String {
    format!("The endpoint {identifier} doesn't exist on web function {function_name}.")
}

/// Why `config` is not an endpoint this service can create in `home_region`, or
/// `None` when it is. The caller answers `Some` with a `ValidationException`.
///
/// Two rules the model cannot express and the boundary validator therefore cannot
/// enforce:
///
/// * the `endpointName` naming rule its member documentation states, and
/// * the `regions` rule: "Required when the endpoint type is `MultiRegion` or
///   `PerRegion`: specify at least one Region other than the Region where you
///   create the endpoint (the home Region). The home Region is added automatically
///   if you don't include it; specifying only the home Region isn't allowed. When
///   the endpoint type is `HomeRegion`, omit this field or specify only the home
///   Region." (source: `smithy_query(query="doc", shape="EndpointConfig",
///   member="regions")`).
pub fn endpoint_config_violation(
    endpoint_name: &str,
    endpoint_type: &str,
    regions: Option<&[String]>,
    home_region: &str,
) -> Option<String> {
    if let Some(message) =
        resource_name_violation("endpointName", endpoint_name, ENDPOINT_NAME_MAX_CHARS)
    {
        return Some(message);
    }
    let named: Vec<&String> = regions.unwrap_or_default().iter().collect();
    let has_other_region = named.iter().any(|region| region.as_str() != home_region);
    if endpoint_type == ENDPOINT_TYPE_HOME_REGION {
        if has_other_region {
            return Some(format!(
                "The regions parameter must be omitted or specify only the home Region \
                 {home_region} when endpointType is {endpoint_type}."
            ));
        }
        return None;
    }
    if named.is_empty() {
        return Some(format!(
            "The regions parameter is required when endpointType is {endpoint_type}."
        ));
    }
    if !has_other_region {
        return Some(format!(
            "The regions parameter must specify at least one Region other than the home Region \
             {home_region} when endpointType is {endpoint_type}."
        ));
    }
    None
}

/// The Regions an endpoint of `endpoint_type` serves, and the domain each of them
/// answers on.
///
/// The home Region is always one of them — "The home Region is added automatically
/// if you don't include it" — and a Region reports a domain of its own except under
/// `MultiRegion`, which "routes one domain to the nearest Region" and so declares none
/// per Region (the member is optional on the modelled `RegionalEndpoint` for exactly
/// that case).
///
/// The domain a Region reports is the endpoint's own published address: this emulator
/// answers every Region's traffic for one endpoint at the ONE address that endpoint
/// publishes, so that is the address each Region is reached at too.
fn endpoint_regions(
    endpoint_type: &str,
    requested: Option<&[String]>,
    home_region: &str,
    published_domain_name: &str,
) -> BTreeMap<String, EndpointRegion> {
    let mut regions: BTreeMap<String, EndpointRegion> = BTreeMap::new();
    let mut insert = |region: &str| {
        let domain_name = match endpoint_type {
            ENDPOINT_TYPE_MULTI_REGION => None,
            _ => Some(published_domain_name.to_string()),
        };
        regions.insert(region.to_string(), EndpointRegion { domain_name });
    };
    insert(home_region);
    if endpoint_type != ENDPOINT_TYPE_HOME_REGION {
        for region in requested.unwrap_or_default() {
            insert(region);
        }
    }
    regions
}

impl ComAmazonawsLambdawebLambdawebState {
    /// Mint and persist an endpoint of `function_name`, returning its
    /// `endpointName`, or `None` when no such function is stored.
    ///
    /// `published_domain_name` is the address this emulator answers the endpoint's own
    /// HTTP requests on ([`super::endpoint_plane`]), which is what the endpoint reports
    /// as its `domainName`: the create opens that address before it stores the endpoint,
    /// so no endpoint is ever stored announcing a domain nothing serves.
    ///
    /// Creating an endpoint mutates the function, so its `updated_at` moves to
    /// `now`. The endpoint is `Pending` and the clock owes it `Active`: it serves a
    /// revision, so it is ready one [`PROVISIONING_DELAY`] after that revision is
    /// itself ready — one span per dependency level, and never zero, so a caller
    /// reading straight back still sees the transient state.
    ///
    /// An endpoint stored in the [`AUTO_DEPLOYMENT_MODE_LATEST_REVISION`] mode serves the
    /// newest revision that has built, so the switch the clock owes every endpoint in
    /// that mode is taken here, exactly as it is where an update writes
    /// ([`Self::update_endpoint`]) — a create that asks for the mode routes the endpoint
    /// onto that revision at once instead of at whatever call comes next, so the routing
    /// the create reports is the routing every later reader of the endpoint reports.
    pub fn add_endpoint(
        &mut self,
        function_name: &str,
        new: NewEndpoint,
        home_region: &str,
        published_domain_name: &str,
        now: SystemTime,
    ) -> Option<String> {
        let function = self.functions.get_mut(function_name)?;
        let endpoint = Endpoint {
            endpoint_arn: endpoint_arn(&function.function_arn, &new.endpoint_name),
            endpoint_name: new.endpoint_name.clone(),
            description: new.description,
            regions: endpoint_regions(
                &new.endpoint_type,
                new.regions.as_deref(),
                home_region,
                published_domain_name,
            ),
            endpoint_type: new.endpoint_type,
            domain_name: published_domain_name.to_string(),
            auth_type: new.auth_type,
            auto_deployment_mode: new
                .auto_deployment_mode
                .unwrap_or_else(|| DEFAULT_AUTO_DEPLOYMENT_MODE.to_string()),
            revision_weights: new.revision_weights,
            // An empty configuration object sets none of its members, so the endpoint is
            // stored with no such configuration at all — the one meaning this service
            // gives it on its update path too.
            scaling_config: configured(new.scaling_config),
            throttle_config: configured(new.throttle_config),
            state: EndpointState::Pending,
            state_reason: ENDPOINT_PENDING_REASON.to_string(),
            update_status: None,
            update_status_reason: None,
            created_at: now,
            updated_at: now,
            activates_at: Some(now + PROVISIONING_DELAY + PROVISIONING_DELAY),
            update_completes_at: None,
        };
        let endpoint_name = endpoint.endpoint_name.clone();
        function.endpoints.insert(endpoint_name.clone(), endpoint);
        function.updated_at = now;
        function.deploy_latest_revision();
        Some(endpoint_name)
    }

    /// Apply `update` to the endpoint `endpoint_name` of `function_name`, reporting
    /// whether both are stored.
    ///
    /// An endpoint left in the [`AUTO_DEPLOYMENT_MODE_LATEST_REVISION`] mode serves the
    /// newest revision that has built, so the switch the clock owes every endpoint in
    /// that mode is taken here too — an update that turns the mode on routes the
    /// endpoint onto that revision at once instead of at whatever call comes next.
    pub fn update_endpoint(
        &mut self,
        function_name: &str,
        endpoint_name: &str,
        update: EndpointUpdate,
        now: SystemTime,
    ) -> bool {
        let Some(function) = self.functions.get_mut(function_name) else {
            return false;
        };
        let Some(endpoint) = function.endpoints.get_mut(endpoint_name) else {
            return false;
        };
        endpoint.apply(update, now);
        function.deploy_latest_revision();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An endpoint serving `weights`, for the rows below: only the member the rule under
    /// test reads differs between them.
    fn serving(weights: &[(&str, i32)]) -> Endpoint {
        let now = SystemTime::UNIX_EPOCH;
        Endpoint {
            endpoint_arn: "arn:aws:lambda:us-east-1:000000000000:web-function/f/endpoint/ep"
                .to_string(),
            endpoint_name: "ep".to_string(),
            description: None,
            regions: BTreeMap::new(),
            endpoint_type: ENDPOINT_TYPE_HOME_REGION.to_string(),
            domain_name: "127.0.0.1:8080".to_string(),
            auth_type: AUTH_TYPE_IAM.to_string(),
            auto_deployment_mode: DEFAULT_AUTO_DEPLOYMENT_MODE.to_string(),
            revision_weights: weights
                .iter()
                .map(|(revision_id, weight)| RevisionWeight {
                    revision_id: (*revision_id).to_string(),
                    weight: *weight,
                })
                .collect(),
            scaling_config: None,
            throttle_config: None,
            state: EndpointState::Active,
            state_reason: ENDPOINT_ACTIVE_REASON.to_string(),
            update_status: None,
            update_status_reason: None,
            created_at: now,
            updated_at: now,
            activates_at: None,
            update_completes_at: None,
        }
    }

    /// One row of the routing table: the case, the endpoint's weights, and the revision
    /// each draw routes to.
    type WeightRow<'a> = (&'a str, Vec<(&'a str, i32)>, Vec<(u64, Option<&'a str>)>);

    /// RULE: a request routes to the revision whose SHARE of the endpoint's traffic its own
    /// draw lands in — the shares being the `weight` of each entry in `revisionWeights`
    /// (`@range(1..100)`) — so an endpoint serving one revision routes every request to it,
    /// and an endpoint serving none routes nowhere.
    ///
    /// The rows span the weightings an endpoint can carry: one revision, an even split, an
    /// uneven split, the smallest and largest weights the model allows, and no revision at
    /// all. Each is read at the boundaries of every share and one past each, so which share
    /// a draw lands in is pinned rather than sampled.
    #[test]
    fn a_request_routes_to_the_revision_whose_share_its_draw_lands_in() {
        // (case, the weights, the revision each draw routes to)
        let rows: Vec<WeightRow<'_>> = vec![
            (
                "one revision with all of the traffic",
                vec![("r-one", FULL_WEIGHT)],
                vec![(0, Some("r-one")), (99, Some("r-one")), (100, Some("r-one")), (7, Some("r-one"))],
            ),
            (
                "an even split",
                vec![("r-a", 50), ("r-b", 50)],
                vec![
                    (0, Some("r-a")),
                    (49, Some("r-a")),
                    (50, Some("r-b")),
                    (99, Some("r-b")),
                    (100, Some("r-a")),
                ],
            ),
            (
                "an uneven split",
                vec![("r-a", 90), ("r-b", 10)],
                vec![
                    (0, Some("r-a")),
                    (89, Some("r-a")),
                    (90, Some("r-b")),
                    (99, Some("r-b")),
                ],
            ),
            (
                "the smallest and largest weights the model allows",
                vec![("r-small", 1), ("r-large", 100)],
                vec![
                    (0, Some("r-small")),
                    (1, Some("r-large")),
                    (100, Some("r-large")),
                    (101, Some("r-small")),
                ],
            ),
            (
                "three shares",
                vec![("r-a", 20), ("r-b", 30), ("r-c", 50)],
                vec![
                    (19, Some("r-a")),
                    (20, Some("r-b")),
                    (49, Some("r-b")),
                    (50, Some("r-c")),
                    (99, Some("r-c")),
                ],
            ),
            (
                "an endpoint serving no revision",
                vec![],
                vec![(0, None), (1, None), (u64::MAX, None)],
            ),
        ];
        for (case, weights, draws) in rows {
            let endpoint = serving(&weights);
            for (draw, expected) in draws {
                let routed = endpoint.routed_revision(draw);
                assert_eq!(
                    routed, expected,
                    "case {case:?}: a draw of {draw} must route to {expected:?}, got {routed:?}"
                );
            }
        }
    }
}
