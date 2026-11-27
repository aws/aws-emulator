// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lambdaweb-lambdaweb service.
//!
//! Every web function a client creates lives here, with its revisions and its
//! endpoints nested under it, so `CreateWebFunction`, `CreateWebFunctionRevision`,
//! `GetWebFunction`, `GetWebFunctionRevision`, `GetWebFunctionEndpoint`,
//! `ListWebFunctions` and `DeleteWebFunction` all read and write ONE copy of each
//! resource. The helpers beside the state struct — ARN construction, the request's
//! region, the documented resource-name rule, the record-to-modelled-shape
//! projections, the clock-driven `Pending -> Active` transition and the opaque list
//! cursor codec — are shared by those handlers rather than duplicated per
//! operation. The endpoint slice lives in [`endpoints`], the resource-policy
//! slice in [`resource_policies`] and the tag slice in [`tags`].

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    BuildConfig, Filter, FunctionRevisionSummary, FunctionSummary, RevisionError, ServiceConfig,
};

/// The account-settings slice: the quotas this emulator applies to web
/// functions, and the usage `GetWebAccountSettings` derives from the store.
mod account;
pub use account::*;

/// The deployment package a revision is built from: the object its `s3Object` names,
/// whether the build can read an archive there at all, and the modules it holds.
pub mod code_package;

/// The JavaScript engine a revision's deployment package RUNS in, which is what a
/// published endpoint's data plane answers a request with.
pub mod web_runtime;

/// The data plane an endpoint publishes: the listener this emulator answers the
/// endpoint's own HTTP requests on, and the address it announces.
pub mod endpoint_plane;
pub use endpoint_plane::{open_endpoint_plane, EndpointPlane};

/// The endpoint slice: what an endpoint of a web function is, and the rules the
/// create paths validate it against.
mod endpoints;
pub use endpoints::*;

/// The grammar a resource-based policy document has to follow before the put
/// will attach it.
mod policy_document;
pub use policy_document::*;

/// The resource-policy slice: the resource-based policy a web function carries,
/// the document rule the put holds it to and the revision rule both mutating
/// operations apply.
mod resource_policies;
pub use resource_policies::*;

/// The customer managed KMS key a revision would be encrypted under: when each create
/// asks KMS about it, and the sentence a create is refused with when it cannot be used.
pub mod revision_key;

/// The revision slice: how a request names a revision, the sentence an absent one is
/// reported with, and the rule a revision's delete follows.
mod revisions;
pub use revisions::*;

/// The tag slice: the tags a web function carries, and the rules the tag
/// operations hold a key set to.
mod tags;
pub use tags::*;

/// The region an emulated request is treated as having been made in when its
/// SigV4 credential scope does not name one (an unsigned or anonymous request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` / `endpointPrefix` / sigv4 name this service's ARNs carry,
/// from the model's `aws.api#service` trait (`smithy_query(query="service_info")`:
/// `arnNamespace: lambda`).
pub const ARN_NAMESPACE: &str = "lambda";

/// How long after the call that created it a web function (or a revision) stays
/// `Pending` before the clock owes it `Active`.
///
/// CHOSEN, not cited: the model documents the states but no duration for the
/// transition. It is deliberately non-zero, so a caller that reads straight back
/// still observes the transient `Pending` state the real service reports.
pub const PROVISIONING_DELAY: Duration = Duration::from_millis(500);

/// The most web functions this emulator stores for the account before
/// `CreateWebFunction` answers its modelled `ServiceQuotaExceededException`.
///
/// The quota EXISTS per the model (`ServiceQuotaExceededException`: "the maximum
/// number of web functions in your account"), but its value is not published, so
/// this bound is CHOSEN: far above any plausible client's function count, while
/// keeping the store a caller can grow bounded.
pub const MAX_WEB_FUNCTIONS: usize = 500;

/// The most revisions this emulator stores per web function before a create
/// answers its modelled `ServiceQuotaExceededException` ("the maximum number of
/// revisions or endpoints per web function"). CHOSEN for the same reason as
/// [`MAX_WEB_FUNCTIONS`].
pub const MAX_REVISIONS_PER_FUNCTION: usize = 100;

/// The longest `functionName` the service accepts.
///
/// The model constrains `FunctionName` to 1..256, but the member's own
/// documentation states the narrower rule this emulator enforces: "The name can
/// contain letters, numbers, hyphens (-), and underscores (_), and can't begin or
/// end with a hyphen or an underscore. The name is limited to 64 characters in
/// length." (source: `smithy_query(query="doc", shape="CreateWebFunctionRequest",
/// member="functionName")`).
pub const FUNCTION_NAME_MAX_CHARS: usize = 64;

/// `ServiceConfig$timeoutSeconds`' modelled `@default` (model: range 3..900,
/// default 30).
pub const DEFAULT_TIMEOUT_SECONDS: i32 = 30;

/// `ServiceConfig$maxConcurrencyPerEnvironment`' modelled `@default` (model:
/// range 1..128, default 64).
pub const DEFAULT_MAX_CONCURRENCY_PER_ENVIRONMENT: i32 = 64;

/// The `applicationLogLevel` a revision takes when the request names none: "If you don't
/// specify a value, the default is `INFO`, and this default is returned in the response."
/// (source: `smithy_query(query="doc", shape="LoggingConfig",
/// member="applicationLogLevel")`).
pub const DEFAULT_APPLICATION_LOG_LEVEL: &str = "INFO";

/// The `systemLogLevel` a revision takes when the request names none: the same sentence,
/// one member along (source: `smithy_query(query="doc", shape="LoggingConfig",
/// member="systemLogLevel")`).
pub const DEFAULT_SYSTEM_LOG_LEVEL: &str = "INFO";

/// The `logGroup` a revision takes when the request names none: "If you don't specify a
/// value, the default is `/aws/lambda/web/{functionName}`, and this default is returned in
/// the response." (source: `smithy_query(query="doc", shape="LoggingConfig",
/// member="logGroup")`).
pub fn default_log_group(function_name: &str) -> String {
    format!("/aws/lambda/web/{function_name}")
}

/// The page size a listing uses when the caller sends no `maxResults`:
/// the member's modelled `@default` (and the largest page its `@range` allows).
pub const DEFAULT_MAX_RESULTS: usize = 50;

/// The largest page a listing returns (`MaxResults` range 1..50).
pub const MAX_MAX_RESULTS: usize = 50;

/// The only filter name the web-function and revision listings support: "The only
/// supported filter name is `state`." (source: `smithy_query(query="doc",
/// shape="ListWebFunctionsRequest", member="filters")`, stated identically by
/// `ListWebFunctionRevisionsRequest$filters`).
pub const STATE_FILTER_NAME: &str = "state";

/// `stateReason` while a web function is still being created.
pub const FUNCTION_PENDING_REASON: &str = "The function is being created.";
/// `stateReason` once a web function is serving.
pub const FUNCTION_ACTIVE_REASON: &str = "The function is active.";
/// `stateReason` while a revision is still being built.
pub const REVISION_PENDING_REASON: &str = "The revision is being created.";
/// `stateReason` once a revision is built.
pub const REVISION_ACTIVE_REASON: &str = "The revision is active.";

/// The `FunctionState` of a stored web function.
///
/// The variants are exactly the model's enum values
/// (`FUNCTION_STATE_VALUES` in `ops::types`), and [`FunctionState::wire`] is the
/// only place a state becomes a wire string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionState {
    Pending,
    Active,
    Failed,
    Deleting,
}

impl FunctionState {
    /// This state as the model's enum value.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Active => "Active",
            Self::Failed => "Failed",
            Self::Deleting => "Deleting",
        }
    }

    /// The state a wire value names, or `None` when it names none — which is how
    /// a `state` filter value that is not a `FunctionState` is refused.
    pub fn from_wire(value: &str) -> Option<Self> {
        [Self::Pending, Self::Active, Self::Failed, Self::Deleting]
            .into_iter()
            .find(|state| state.wire() == value)
    }
}

/// The `RevisionState` of a stored revision: the model's enum values
/// (`REVISION_STATE_VALUES` in `ops::types`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionState {
    Pending,
    Active,
    Failed,
}

impl RevisionState {
    /// This state as the model's enum value.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Active => "Active",
            Self::Failed => "Failed",
        }
    }

    /// The state a wire value names, or `None` when it names none — which is how
    /// a `state` filter value that is not a `RevisionState` is refused.
    pub fn from_wire(value: &str) -> Option<Self> {
        [Self::Pending, Self::Active, Self::Failed]
            .into_iter()
            .find(|state| state.wire() == value)
    }
}

/// One stored revision of a web function.
///
/// `build_config` and `service_config` hold the modelled shapes VERBATIM —
/// including the `timeoutSeconds` / `maxConcurrencyPerEnvironment` defaults
/// [`adopt_service_config`] applied — so every later read reports what was stored
/// instead of re-deriving anything from a request.
#[derive(Debug, Clone)]
pub struct Revision {
    pub revision_id: String,
    pub revision_arn: String,
    pub description: Option<String>,
    pub kms_key_arn: Option<String>,
    pub build_config: BuildConfig,
    pub service_config: ServiceConfig,
    pub state: RevisionState,
    pub state_reason: String,
    pub errors: Vec<RevisionError>,
    pub created_at: SystemTime,
    /// The instant the clock owes this revision the end of its BUILD, or `None` once
    /// that has been materialised.
    pub activates_at: Option<SystemTime>,
    /// Why this revision's build fails, as established by the read of its deployment
    /// package ([`code_package`]) when the create was served, or `None` for a build
    /// that succeeds. It is what decides whether the revision settles `Active` or
    /// `Failed`, so a revision whose code is not an archive the build can read is never
    /// reported as having built.
    pub build_failure: Option<String>,
    /// The modules the build read out of this revision's deployment package
    /// ([`code_package::DeploymentPackage`]), or `None` when the build established no
    /// package at all. It is what an endpoint serving this revision RUNS
    /// ([`web_runtime`]), so a revision whose code was never read answers for that rather
    /// than for a response nothing produced.
    pub package: Option<Arc<code_package::DeploymentPackage>>,
}

impl Revision {
    /// This revision as the `FunctionRevisionSummary` a create reports.
    pub fn summary(&self) -> FunctionRevisionSummary {
        FunctionRevisionSummary {
            revision_arn: self.revision_arn.clone(),
            revision_id: self.revision_id.clone(),
            description: self.description.clone(),
            state: self.state.wire().to_string(),
            state_reason: self.state_reason.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
        }
    }

    /// The stored build errors as the modelled `RevisionErrors` list, or `None`
    /// when there are none: the model constrains the list to 1..50 entries, so a
    /// revision with no errors reports no member rather than an empty list.
    pub fn modeled_errors(&self) -> Option<Vec<RevisionError>> {
        if self.errors.is_empty() {
            return None;
        }
        Some(self.errors.clone())
    }

    /// The instant this revision's build was due to finish: its creation plus the one
    /// provisioning span, which is when the clock owed it its `Active` (or `Failed`) state.
    ///
    /// The one instant a switch onto this revision is derived from, so the timestamp a
    /// caller reads for that switch is stable across reads.
    pub fn built_at(&self) -> SystemTime {
        self.created_at + PROVISIONING_DELAY
    }

    /// Settle this revision's build if the clock has already made its end due: `Active`
    /// for a build that succeeded, `Failed` carrying the build's own reason for one that
    /// did not ([`Self::build_failure`]). Owed at [`Self::activates_at`] whether or not
    /// anybody called, so materialising it on a read reports what was already true.
    fn materialise_due(&mut self, now: SystemTime) {
        if self.state != RevisionState::Pending || !self.activates_at.is_some_and(|due| now >= due)
        {
            return;
        }
        match &self.build_failure {
            Some(reason) => {
                self.state = RevisionState::Failed;
                self.state_reason = reason.clone();
            }
            None => {
                self.state = RevisionState::Active;
                self.state_reason = REVISION_ACTIVE_REASON.to_string();
            }
        }
        self.activates_at = None;
    }
}

/// One stored web function: what `CreateWebFunction` was given, plus the ARN this
/// service minted, the timestamps it fixed and the revisions created under it.
#[derive(Debug, Clone)]
pub struct WebFunction {
    pub function_name: String,
    pub function_arn: String,
    pub state: FunctionState,
    pub state_reason: String,
    /// Fixed at creation and never moved again.
    pub created_at: SystemTime,
    /// Bumped by every mutation of this function, and by the clock-driven
    /// `Pending -> Active` transition.
    pub updated_at: SystemTime,
    /// The instant the clock owes this function its `Pending -> Active`
    /// transition, or `None` once it has been materialised.
    pub activates_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    /// The revisions of this function, keyed by `revisionId`.
    pub revisions: BTreeMap<String, Revision>,
    pub latest_revision_id: Option<String>,
    /// The endpoints of this function, keyed by `endpointName` — the endpoint's
    /// identity in this API. Nested here, so a function's delete takes its
    /// endpoints with it and every endpoint reader sees ONE copy of each.
    pub endpoints: BTreeMap<String, Endpoint>,
    /// The resource-based policy attached to this function, or `None` while it
    /// carries none. A function has at most one, and it is nested here for the
    /// same reason the endpoints are: a function's delete takes its policy with
    /// it, and every resource-policy operation reads ONE copy of it.
    pub resource_policy: Option<ResourcePolicy>,
}

impl WebFunction {
    /// This function as the `FunctionSummary` `ListWebFunctions` returns: the
    /// stored record's own fields, never request input.
    pub fn summary(&self) -> FunctionSummary {
        FunctionSummary {
            function_name: self.function_name.clone(),
            function_arn: self.function_arn.clone(),
            state: self.state.wire().to_string(),
            state_reason: self.state_reason.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }

    /// The stored tags as the modelled `Tags` map, or `None` when the function
    /// carries none.
    pub fn modeled_tags(&self) -> Option<HashMap<String, String>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(modeled_tag_map(&self.tags))
    }

    /// The Region this web function belongs to: "the Region where you created the
    /// web function", which is the Region its own stored ARN names.
    ///
    /// A web function is a REGIONAL resource — the create mints its ARN in the
    /// Region the call was signed for ([`function_arn`]) — so the stored ARN is
    /// where the Region it belongs to is recoverable from, for every reader that
    /// has to tell one Region's functions from another's. A stored ARN whose Region
    /// field is not a Region label names no Region, and leaves `signed_for`: the
    /// Region the reading call was made in.
    pub fn home_region<'value>(&'value self, signed_for: &'value str) -> &'value str {
        ::service_common::arn::region_of(&self.function_arn, ARN_NAMESPACE).unwrap_or(signed_for)
    }

    /// The revision this function's `latestRevisionId` names, if it still has one.
    pub fn latest_revision(&self) -> Option<&Revision> {
        self.latest_revision_id
            .as_ref()
            .and_then(|id| self.revisions.get(id))
    }

    /// Apply every transition the clock has already made due to this function and
    /// its revisions. The function's `updated_at` moves to the instant the
    /// transition was DUE, not to now, so the timestamp a caller reads is stable
    /// across reads.
    fn materialise_due(&mut self, now: SystemTime) {
        if self.state == FunctionState::Pending {
            if let Some(due) = self.activates_at.filter(|due| now >= *due) {
                self.state = FunctionState::Active;
                self.state_reason = FUNCTION_ACTIVE_REASON.to_string();
                self.updated_at = due;
                self.activates_at = None;
            }
        }
        for revision in self.revisions.values_mut() {
            revision.materialise_due(now);
        }
        for endpoint in self.endpoints.values_mut() {
            endpoint.materialise_due(now);
        }
        self.deploy_latest_revision();
    }

    /// Move every endpoint whose `autoDeploymentMode` is `LatestRevision` onto the newest
    /// revision that has BUILT — "the endpoint automatically serves the newest revision"
    /// (`AutoDeploymentMode`).
    ///
    /// A revision that is still building, or whose build failed, takes no traffic: only a
    /// revision the service reports `Active` is "ready to serve traffic"
    /// (`RevisionState`), so an endpoint in that mode keeps serving what it was serving
    /// until the newest revision has built. Applied where the build itself settles, so
    /// the switch is owed at the same instant the revision became ready whether or not
    /// anybody called.
    fn deploy_latest_revision(&mut self) {
        let Some(newest) = self
            .revisions
            .values()
            .filter(|revision| revision.state == RevisionState::Active)
            .max_by_key(|revision| (revision.created_at, revision.revision_id.clone()))
            .map(|revision| (revision.revision_id.clone(), revision.built_at()))
        else {
            return;
        };
        let (revision_id, became_ready) = newest;
        for endpoint in self.endpoints.values_mut() {
            if endpoint.auto_deployment_mode != AUTO_DEPLOYMENT_MODE_LATEST_REVISION {
                continue;
            }
            let serving_only_newest = endpoint.revision_weights.len() == 1
                && endpoint.revision_weights[0].revision_id == revision_id;
            if serving_only_newest {
                continue;
            }
            endpoint.revision_weights = vec![crate::ops::types::RevisionWeight {
                revision_id: revision_id.clone(),
                weight: FULL_WEIGHT,
            }];
            // The switch moves the endpoint, so its `updated_at` moves with it — to the
            // instant that revision's build was due to finish, which is the one instant
            // every reader derives this switch from, rather than to whenever it was read.
            endpoint.updated_at = endpoint.updated_at.max(became_ready);
        }
    }
}

/// The configuration a new revision is built from, as the two create operations
/// supply it.
#[derive(Debug, Clone)]
pub struct NewRevision {
    pub description: Option<String>,
    pub kms_key_arn: Option<String>,
    pub build_config: BuildConfig,
    pub service_config: ServiceConfig,
    /// Why the build of this revision fails, as the read of its deployment package
    /// established it ([`code_package::CodePackage::failure_reason`]), or `None` for a
    /// build that succeeds.
    pub build_failure: Option<String>,
    /// The modules the build read out of the deployment package, as
    /// [`code_package::CodePackage::package`] answered for it.
    pub package: Option<Arc<code_package::DeploymentPackage>>,
}

/// Per-service state for com-amazonaws-lambdaweb-lambdaweb. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLambdawebLambdawebState {
    /// Every stored web function, keyed by `functionName` — the resource's
    /// identity in this API. A `BTreeMap`, so `ListWebFunctions` walks one stable
    /// order and a cursor taken on one page resumes at the right place.
    pub functions: BTreeMap<String, WebFunction>,
    /// Monotonic counter behind every minted `revisionId`, so two revisions —
    /// of one function or of two — never share an id.
    pub next_revision_seq: u64,
    /// Monotonic counter behind every minted POLICY `revisionId`, so two
    /// generations of a resource-based policy — of one function or of two — never
    /// share an id.
    pub next_policy_revision_seq: u64,
    /// The listener each endpoint's own data plane is served on, keyed by that
    /// endpoint's ARN. Holding the handle is what keeps the port open and what makes
    /// the published `domainName` an address this emulator answers on; removing the
    /// entry closes it ([`Self::prune_endpoint_planes`]).
    pub endpoint_planes: BTreeMap<String, EndpointPlane>,
}

impl ComAmazonawsLambdawebLambdawebState {
    /// Apply every state transition the clock has already made due, across every
    /// stored function and revision.
    ///
    /// Called at the start of every handler, including the reporting ones: the
    /// transition was owed at [`PROVISIONING_DELAY`] after the call that started
    /// it whether or not anybody read the resource, so materialising it here
    /// reports what was already true rather than changing what a later caller
    /// sees.
    pub fn materialise_due_transitions(&mut self, now: SystemTime) {
        for function in self.functions.values_mut() {
            function.materialise_due(now);
        }
    }

    /// The stored web functions that belong to `region` — the ones whose own ARN
    /// names it ([`WebFunction::home_region`]).
    ///
    /// The store holds every Region's functions together, so a reader that reports
    /// on ONE Region takes this view of it rather than the whole map. Walks the
    /// same `functionName` order `functions` is keyed in.
    ///
    /// A record is read against [`DEFAULT_REGION`] rather than against the Region
    /// the asking call was signed for, so each stored function belongs to exactly
    /// ONE Region — the one its ARN names, or, for an ARN that names no Region
    /// label at all, the Region a request that names none is treated as having been
    /// made in — and the Regions' views partition the store between them.
    pub fn functions_in_region<'view>(
        &'view self,
        region: &'view str,
    ) -> impl Iterator<Item = &'view WebFunction> + 'view {
        self.functions
            .values()
            .filter(move |function| function.home_region(DEFAULT_REGION) == region)
    }

    /// Close the listener of every endpoint this store no longer holds.
    ///
    /// An endpoint's address is the endpoint's: once the record is gone — its own delete,
    /// or its function's, which takes its endpoints with it — the port it answered on is
    /// released, so a deleted endpoint stops answering. Dropping the handle is what closes
    /// it, and dropping it also severs every connection in flight on that port, which is
    /// why it is done by the DELETE that removed the record and not on every read: a
    /// client being served at a published address is never cut off by somebody else's
    /// call.
    pub fn prune_endpoint_planes(&mut self) {
        if self.endpoint_planes.is_empty() {
            return;
        }
        let live: ::std::collections::BTreeSet<String> = self
            .functions
            .values()
            .flat_map(|function| {
                function
                    .endpoints
                    .values()
                    .map(|endpoint| endpoint.endpoint_arn.clone())
            })
            .collect();
        self.endpoint_planes
            .retain(|endpoint_arn, _| live.contains(endpoint_arn));
    }

    /// Mint a fresh `revisionId`: the counter rendered in the 8-4-4-4-12 shape
    /// this service's other revision ids carry (`DeleteResourcePolicy`'s
    /// `revisionId` is modelled `^[0-9a-f-]{36}$`), which also keeps the id free
    /// of `/` so it rides in the `{revisionId}` URI label unescaped.
    pub fn mint_revision_id(&mut self) -> String {
        self.next_revision_seq = self.next_revision_seq.wrapping_add(1);
        hex_uuid_shape(0, self.next_revision_seq)
    }

    /// Mint and persist a revision of `function_name`, returning its `revisionId`,
    /// or `None` when no such function is stored.
    ///
    /// Adding a revision mutates the function, so its `updated_at` moves to `now`.
    pub fn add_revision(
        &mut self,
        function_name: &str,
        new: NewRevision,
        now: SystemTime,
    ) -> Option<String> {
        if !self.functions.contains_key(function_name) {
            return None;
        }
        let revision_id = self.mint_revision_id();
        let function = self.functions.get_mut(function_name)?;
        let revision = Revision {
            revision_arn: revision_arn(&function.function_arn, &revision_id),
            revision_id: revision_id.clone(),
            description: new.description,
            kms_key_arn: new.kms_key_arn,
            build_config: new.build_config,
            service_config: adopt_service_config(new.service_config, function_name),
            state: RevisionState::Pending,
            state_reason: REVISION_PENDING_REASON.to_string(),
            errors: Vec::new(),
            created_at: now,
            activates_at: Some(now + PROVISIONING_DELAY),
            build_failure: new.build_failure,
            package: new.package,
        };
        function.revisions.insert(revision_id.clone(), revision);
        function.latest_revision_id = Some(revision_id.clone());
        function.updated_at = now;
        Some(revision_id)
    }
}

/// The `ServiceConfig` to store: the supplied one with the model's declared and
/// DOCUMENTED defaults applied to any member that arrived without a value.
///
/// `timeoutSeconds` is `@range(3..900) @default(30)` and
/// `maxConcurrencyPerEnvironment` is `@range(1..128) @default(64)`. Those ranges
/// are MODELLED, so the boundary validator has already refused any value outside
/// them before a handler runs — which is why a 0 reaching here can only mean the
/// member was absent, and why no range check belongs in this function.
///
/// The logging configuration's three members carry no `@default` but DOCUMENT one each,
/// in the same words: "If you don't specify a value, the default is …, and this default is
/// returned in the response" ([`DEFAULT_APPLICATION_LOG_LEVEL`],
/// [`DEFAULT_SYSTEM_LOG_LEVEL`], [`default_log_group`]). Returning the default is part of
/// the rule, so the effective configuration is materialised here — once, where it is
/// stored — and every later read reports what was stored.
pub fn adopt_service_config(mut config: ServiceConfig, function_name: &str) -> ServiceConfig {
    if config.timeout_seconds == 0 {
        config.timeout_seconds = DEFAULT_TIMEOUT_SECONDS;
    }
    if config.max_concurrency_per_environment == 0 {
        config.max_concurrency_per_environment = DEFAULT_MAX_CONCURRENCY_PER_ENVIRONMENT;
    }
    let mut telemetry = config.telemetry_config.unwrap_or_default();
    let mut logging = telemetry.logging_config.unwrap_or_default();
    if logging.log_group.is_none() {
        logging.log_group = Some(default_log_group(function_name));
    }
    if logging.application_log_level.is_none() {
        logging.application_log_level = Some(DEFAULT_APPLICATION_LOG_LEVEL.to_string());
    }
    if logging.system_log_level.is_none() {
        logging.system_log_level = Some(DEFAULT_SYSTEM_LOG_LEVEL.to_string());
    }
    telemetry.logging_config = Some(logging);
    config.telemetry_config = Some(telemetry);
    config
}

/// The region of the request being served: the third field of its SigV4
/// credential scope, falling back to [`DEFAULT_REGION`] for a request that is not
/// signed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a web function in `region`:
/// `arn:aws:lambda:<region>:<account>:web-function/<functionName>` — the shape the
/// resource-policy operations' `resourceArn` pattern requires
/// (`^arn:[^:]+:lambda:[^:]+:\d{12}:web-function/[^/]+$`).
pub fn function_arn(region: &str, function_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("web-function/{function_name}"),
    )
}

/// The ARN of a revision, derived from its FUNCTION's stored ARN so the two always
/// name the same function, region and account:
/// `<functionArn>/revision/<revisionId>`. `RevisionArn` is modelled with a length
/// bound only (1..2048) and no pattern, so this layout mirrors the operation's own
/// URI (`/web-functions/{functionName}/revisions/{revisionId}`).
pub fn revision_arn(function_arn: &str, revision_id: &str) -> String {
    format!("{function_arn}/revision/{revision_id}")
}

/// The `functionName` a request names, resolved to the key the store holds.
///
/// The member's documentation says the caller "can specify either of the
/// following: The function name, for example my-function. The function ARN."
/// (source: `smithy_query(query="doc", shape="GetWebFunctionRequest",
/// member="functionName")`), so a value in the ARN form resolves to the name in
/// its `web-function/<name>` resource segment. Anything else is already a name.
pub fn resolve_function_name(value: &str) -> &str {
    if !is_arn_form(value) {
        return value;
    }
    value
        .split_once(":web-function/")
        .map_or(value, |(_, name)| name)
}

/// Whether `value` names a resource in the ARN form rather than by its bare name.
///
/// The one test every member that accepts both forms is read with, so the resolvers
/// and the length rule below cannot disagree about which form a value arrived in.
pub fn is_arn_form(value: &str) -> bool {
    value.starts_with("arn:")
}

/// AWS's `ResourceNotFoundException` message for a web function this store does
/// not hold, naming it by `identifier` — the value the request addressed it by,
/// verbatim, whether that was the name or the ARN.
///
/// The model fixes the status (404), the code and `message` being `@required`, but
/// not the text, so the sentence is the service's recorded behaviour: AWS reports
/// the absent FUNCTION in one sentence, from every operation that resolves one —
/// including the readers and writers of a function's sub-resources (its
/// resource-based policy, its tags), so a sub-resource call addressed at a
/// function that is gone reports the FUNCTION as absent rather than the
/// sub-resource or an empty answer. One function serves every such branch so the
/// wording cannot drift between them.
pub fn function_not_found_message(identifier: &str) -> String {
    format!("The function {identifier} does not exist.")
}

/// Why `name` is not a legal `functionName`, or `None` when it is.
///
/// The model bounds the member's length (1..256) only; this is the narrower rule
/// its documentation states — letters, numbers, hyphens and underscores, never
/// starting or ending with a hyphen or an underscore, at most
/// [`FUNCTION_NAME_MAX_CHARS`] characters — which the boundary validator cannot
/// enforce because the model carries no pattern for it.
pub fn function_name_violation(name: &str) -> Option<String> {
    resource_name_violation("functionName", name, FUNCTION_NAME_MAX_CHARS)
}

/// Why `identifier` is not a legal value for the `member` a request ADDRESSES an
/// already-stored resource of this service by, or `None` when it is.
///
/// Such a member takes either form — "You can specify either of the following: The
/// function name, for example my-function. The function ARN." — and states which
/// length bound applies to which: "The length constraint applies only to the full
/// ARN. If you specify only the function name, it is limited to 64 characters in
/// length." (source: `smithy_query(query="doc", shape="GetWebFunctionRequest",
/// member="functionName")`, worded identically for `endpointName`). So a value in
/// the ARN form carries only the model's own 1..256 bound, which the boundary
/// validator already enforces, while a value in the NAME form is bounded at
/// `max_chars` here: past it the request named a malformed parameter, judged before
/// the store is consulted, not a resource the store happens not to hold.
///
/// Only the length clause of [`resource_name_violation`] applies: the character and
/// first/last-character clauses are documented by the members that NAME a resource
/// being created, not by the ones that address a stored one.
pub fn addressed_name_violation(
    member: &str,
    identifier: &str,
    max_chars: usize,
) -> Option<String> {
    if is_arn_form(identifier) {
        return None;
    }
    (identifier.chars().count() > max_chars).then(|| max_length_message(member, max_chars))
}

/// Why the `functionName` a request addresses a stored web function by is outside the
/// documented rule for the name form, or `None` when it is within it. Every operation
/// that resolves a function applies this before the store lookup.
pub fn addressed_function_name_violation(identifier: &str) -> Option<String> {
    addressed_name_violation("functionName", identifier, FUNCTION_NAME_MAX_CHARS)
}

/// Why `name` is not a legal value for the `member` that names a resource of this
/// service, or `None` when it is.
///
/// One rule, stated identically by `functionName` and by `endpointName`: letters,
/// numbers, hyphens and underscores, never starting or ending with a hyphen or an
/// underscore, at most `max_chars` characters. The model gives those members a
/// length bound only, so the boundary validator cannot enforce it and every create
/// path checks it here.
pub fn resource_name_violation(member: &str, name: &str, max_chars: usize) -> Option<String> {
    if name.is_empty() {
        return Some(format!(
            "Member must have length greater than or equal to 1: {member}"
        ));
    }
    if name.chars().count() > max_chars {
        return Some(max_length_message(member, max_chars));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Some(format!(
            "Member must contain only letters, numbers, hyphens (-) and underscores (_): {member}"
        ));
    }
    let first = name.chars().next();
    let last = name.chars().next_back();
    if matches!(first, Some('-' | '_')) || matches!(last, Some('-' | '_')) {
        return Some(format!(
            "Member must not begin or end with a hyphen (-) or an underscore (_): {member}"
        ));
    }
    None
}

/// The sentence a value longer than `max_chars` characters is refused with, worded
/// once so the create paths and the addressing paths state one rule alike.
fn max_length_message(member: &str, max_chars: usize) -> String {
    format!("Member must have length less than or equal to {max_chars}: {member}")
}

/// 32 hex digits — `high` then `low`, 16 each — rendered in the 8-4-4-4-12 shape,
/// which satisfies this service's `^[a-f0-9-]{36}$` token and revision-id
/// patterns.
fn hex_uuid_shape(high: u64, low: u64) -> String {
    let digits = format!("{high:016x}{low:016x}");
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// The page a listing serves, from its `maxResults` member.
///
/// `MaxResults` is `@range(1..50)` with `@default(50)` on every listing of this
/// service, so a value outside that range is already refused at the boundary and a
/// non-positive one here means the member was absent: the default page size applies.
pub fn page_size(max_results: i32) -> usize {
    usize::try_from(max_results)
        .ok()
        .filter(|size| *size >= 1)
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .min(MAX_MAX_RESULTS)
}

/// The states each supplied filter narrows a listing to, or the `ValidationException`
/// message the request earns.
///
/// ONE rule for every listing of this service: the only filter name supported is
/// [`STATE_FILTER_NAME`] and each of its values must name one of `values` — the
/// resource's own state enum, parsed by `from_wire`. The result holds one entry per
/// supplied filter, so a resource matches when its state is among the values of
/// EVERY filter.
pub fn state_filters<State>(
    filters: &[Filter],
    values: &[&str],
    from_wire: impl Fn(&str) -> Option<State>,
) -> Result<Vec<Vec<State>>, String> {
    let mut wanted = Vec::with_capacity(filters.len());
    for filter in filters {
        if filter.name != STATE_FILTER_NAME {
            return Err(format!(
                "Member must satisfy the supported filter names [{STATE_FILTER_NAME}]: filters.name"
            ));
        }
        let mut states = Vec::with_capacity(filter.values.len());
        for value in &filter.values {
            let Some(state) = from_wire(value) else {
                return Err(format!(
                    "Member must satisfy enum value set [{}]: filters.values",
                    values.join(", ")
                ));
            };
            states.push(state);
        }
        wanted.push(states);
    }
    Ok(wanted)
}

/// Where a listing page resumes: the digest of the key the page starts
/// at, and that key's index in the listing the cursor was minted from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListCursor {
    /// The first 8 bytes of SHA-256 over the resume key, so a cursor still finds
    /// its key after the listing shifted.
    pub digest: u64,
    /// The resume key's index when the cursor was minted — the fallback when the
    /// key itself is gone.
    pub offset: usize,
}

/// The first 8 bytes of SHA-256 over `key`, as the cursor's validator.
fn key_digest(key: &str) -> u64 {
    let digest = Sha256::digest(key.as_bytes());
    let mut head = [0u8; 8];
    head.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(head)
}

/// The opaque `nextToken` for a page that resumes at `key`, the `offset`-th item
/// of the listing it was minted from.
///
/// Self-describing rather than stored: nothing a client can read structure out of,
/// nothing the service has to keep, and — as the model's
/// `NextToken` pattern `^[a-f0-9-]{36}$` demands — 36 characters of lowercase hex
/// and hyphens.
pub fn encode_cursor(key: &str, offset: usize) -> String {
    hex_uuid_shape(key_digest(key), u64::try_from(offset).unwrap_or(u64::MAX))
}

/// The cursor a `nextToken` carries, or `None` when the token is not one this
/// service minted (the caller answers that with a `ValidationException`).
pub fn decode_cursor(token: &str) -> Option<ListCursor> {
    let groups: Vec<&str> = token.split('-').collect();
    if groups.iter().map(|group| group.len()).ne([8usize, 4, 4, 4, 12]) {
        return None;
    }
    let digits = groups.concat();
    // Lowercase only, as the model's `^[a-f0-9-]{36}$` pattern says and as
    // `encode_cursor` always mints: an upper-cased token is not one this service
    // handed out.
    if !digits.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
        return None;
    }
    let digest = u64::from_str_radix(&digits[0..16], 16).ok()?;
    let offset = u64::from_str_radix(&digits[16..32], 16).ok()?;
    Some(ListCursor {
        digest,
        offset: usize::try_from(offset).ok()?,
    })
}

/// The index in `keys` that `cursor` resumes at.
///
/// The cursor's own offset when the key there still hashes to the cursor's
/// digest; else wherever that key moved to; else — the key was deleted between
/// pages — the offset it pointed at, clamped to the end of the listing.
pub fn resume_index(cursor: ListCursor, keys: &[&str]) -> usize {
    if keys
        .get(cursor.offset)
        .is_some_and(|key| key_digest(key) == cursor.digest)
    {
        return cursor.offset;
    }
    keys.iter()
        .position(|key| key_digest(key) == cursor.digest)
        .unwrap_or_else(|| cursor.offset.min(keys.len()))
}

/// The slice of a listing one page covers, and the cursor that resumes after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPage {
    /// The index of the first item of the page in the listing it was taken from.
    pub start: usize,
    /// One past the index of its last item.
    pub end: usize,
    /// The cursor naming the first item AFTER this page, present exactly when one
    /// remains.
    pub next_token: Option<String>,
}

/// The page a listing serves, from the identities of its matching items in their
/// stable order and the cursor the caller sent.
///
/// `None` when `next_token` is not a cursor this service minted — the caller answers
/// that with its own `ValidationException`, naming its own operation.
///
/// The resume point is clamped to the end of the CURRENT listing, so a cursor can never
/// index past the matches — whether because items were deleted between pages or because
/// the token was minted from a listing narrowed by different filters. A resume point past
/// the last match is an empty final page, never a panic.
pub fn page_bounds(keys: &[&str], next_token: Option<&str>, page_size: usize) -> Option<ListPage> {
    let start = match next_token {
        None => 0,
        Some(token) => resume_index(decode_cursor(token)?, keys),
    }
    .min(keys.len());
    let end = keys.len().min(start.saturating_add(page_size));
    Some(ListPage {
        start,
        end,
        next_token: keys.get(end).map(|key| encode_cursor(key, end)),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLambdawebLambdawebState>> {
    shared_engine(engine).map(::engine::InMemoryEngine::state::<ComAmazonawsLambdawebLambdawebState>)
}

/// The ONE shared engine behind a handler's `&*self.engine`, for the things that are the
/// ENGINE's rather than this service's store: the core-owned listener an endpoint's data
/// plane is served on ([`endpoint_plane`]).
///
/// `None` for an engine this emulator was not assembled with, which is what leaves a
/// caller answering a modelled server error instead of publishing an address nothing
/// serves.
pub fn shared_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<&::engine::InMemoryEngine> {
    (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()
}
