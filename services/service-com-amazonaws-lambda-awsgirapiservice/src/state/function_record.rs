// SPDX-License-Identifier: CC0-1.0

//! The FUNCTION RECORD: one Lambda function as this emulator holds it, and everything a
//! read of that function reports out of what is stored.
//!
//! It lives in its own module rather than in the already long [`crate::state`], which
//! re-exports it, so a handler still names it as `crate::state::FunctionRecord` and the
//! module split is invisible to it. The STORE that holds the records
//! ([`crate::state::ComAmazonawsLambdaAwsgirapiserviceState::functions`]) stays there, as
//! do the records this one carries (its configuration, its code, its aliases, its
//! versions) and the ARN and message helpers it reads; the recursive-loop bookkeeping kept
//! on the record is [`crate::state::recursion`], which implements its own methods on it.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CreateAliasOutput, CreateFunctionOutput, EnvironmentResponse, EphemeralStorage, Execution,
    ExecutionStartedDetails, FunctionCodeLocation, GetFunctionScalingConfigOutput, ImageConfigResponse,
    Layer, LoggingConfig, PutFunctionConcurrencyOutput, ResolvedS3Object, RuntimeVersionConfig,
    SnapStartResponse, TracingConfigResponse, VpcConfigResponse,
};

use super::{
    code_location_url, configured_application_log_level, configured_capacity_provider,
    configured_function_arn, configured_log_format, configured_log_group,
    configured_system_log_level, event_payload, iso8601,
    mint_durable_execution_id, mint_revision_id, resolved_image_uri, runtime_version_arn,
    service_managed_function_scaling_config, AliasRecord, AsyncInvocationOutcome,
    AsyncInvocationRecord, CapacityProviderVersion, CodeRecord, DurableExecutionRecord,
    DurableExecutionStart, EventInvokeConfigRecord, FunctionConfig, FunctionScalingConfigRecord,
    FunctionUrlConfigRecord,
    FunctionVersionRecord, PolicyRecord, ProvisionedConcurrencyRecord, RequestChainRecord,
    RuntimeManagementConfigRecord, ASYNC_DELIVERY_DELAY, ASYNC_NOT_DELIVERED_REASON,
    ASYNC_RECURSION_STOPPED_REASON,
    DURABLE_EXECUTION_ARN_INFIX, EVENT_TYPE_EXECUTION_STARTED, EXECUTION_STATUS_RUNNING,
    LAST_UPDATE_STATUS_IN_PROGRESS, LAST_UPDATE_STATUS_SUCCESSFUL,
    MAX_DURABLE_EXECUTIONS, MAX_QUEUED_ASYNC_INVOCATIONS,
    NO_CAPACITY_PROVIDER_MESSAGE, REPOSITORY_TYPE_ECR, REPOSITORY_TYPE_S3,
    S3_OBJECT_STORAGE_MODE_REFERENCE, SNAP_START_APPLY_ON_NONE,
    SNAP_START_APPLY_ON_PUBLISHED_VERSIONS, SNAP_START_OPTIMIZATION_STATUS_OFF,
    SNAP_START_OPTIMIZATION_STATUS_ON, STATE_ACTIVE, STATE_ACTIVE_NON_INVOCABLE, STATE_PENDING,
    STATE_REASON_CODE_CREATING, STATE_REASON_CREATING, UPDATE_DELAY, VERSION_LATEST,
    VERSION_LATEST_PUBLISHED,
};

/// The deployment a code update replaced, kept while that update is still being applied.
///
/// A code update opens a window — `LastUpdateStatus` is `InProgress` until the clock makes
/// its completion due — and "During this time, you can't modify the function, but you can
/// still invoke it": an invocation inside the window is served by the deployment that was
/// already in place. So the package it runs is kept here, beside the revision it was
/// deployed at, which is what says which execution environments may serve it
/// ([`FunctionRecord::serving_code_of`], [`FunctionRecord::serving_revision_of`]).
#[derive(Debug, Clone)]
pub struct ServingDeployment {
    /// The deployment package that keeps serving until the update completes.
    pub code: CodeRecord,
    /// The `RevisionId` the function was at when that package was deployed.
    pub revision_id: String,
}

/// One Lambda function: its unpublished (`$LATEST`) configuration, its code, its
/// lifecycle state, its tags and its published versions.
#[derive(Debug, Clone)]
pub struct FunctionRecord {
    /// The unqualified function ARN, minted once at creation.
    pub arn: String,
    /// The region the function was created in, which its ARN and its code location
    /// carry.
    pub region: String,
    /// The `$LATEST` configuration.
    pub config: FunctionConfig,
    /// Where the code came from.
    pub code: CodeRecord,
    /// The deployment that KEEPS SERVING invocations while a code update is still being
    /// applied: "During this time, you can't modify the function, but you can still invoke
    /// it" — and what those invocations run is the deployment the function was already
    /// serving, because the new one is not in place until the update completes.
    ///
    /// [`Self::code`] is the package the function has been GIVEN, which every report of the
    /// function (its `CodeSha256`, its `CodeSize`, its code location, and a version it
    /// publishes) names from the instant the update is accepted. This holds the one it
    /// replaced until the clock makes the update's completion due, when
    /// [`FunctionRecord::materialize`] drops it and the two become one again; a function with
    /// no code update in flight holds none at all. Read it through
    /// [`Self::serving_code_of`].
    pub serving_code: Option<ServingDeployment>,
    /// The model's `State` of the function.
    pub state: String,
    /// The model's `StateReason`, while there is one.
    pub state_reason: Option<String>,
    /// The model's `StateReasonCode`, while there is one.
    pub state_reason_code: Option<String>,
    /// The model's `LastUpdateStatus`, first set once creation completes.
    pub last_update_status: Option<String>,
    /// When the move from [`LAST_UPDATE_STATUS_IN_PROGRESS`] to
    /// [`LAST_UPDATE_STATUS_SUCCESSFUL`] becomes due, while an update is in flight. Set
    /// by [`FunctionRecord::begin_update`].
    pub last_update_due: Option<SystemTime>,
    /// When the move to [`STATE_ACTIVE`] becomes due.
    pub active_due: SystemTime,
    /// The `NetworkInterfaceId` of each elastic network interface Lambda stands up in the
    /// account's EC2 inventory for this function's VPC attachment — "Lambda creates an
    /// elastic network interface for each subnet in your function's VPC configuration" — in
    /// the order of the subnets they sit in; empty for a function attached to no VPC.
    ///
    /// It is a FUNCTION-level holding rather than a version-specific one: the interfaces carry
    /// the traffic of the function, so every qualifier of it reaches the VPC through the same
    /// ones, and they are given back when the attachment is removed or the function is deleted
    /// ([`crate::state::vpc_interfaces`]).
    pub vpc_network_interface_ids: Vec<String>,
    /// Function-level tags, as supplied.
    pub tags: BTreeMap<String, String>,
    /// The reserved concurrency, when `PutFunctionConcurrency` has set one.
    pub reserved_concurrent_executions: Option<i32>,
    /// How many executions of this function are being processed at this instant: an
    /// invocation counts from the moment Lambda hands it to an instance of this function
    /// until that run ends.
    ///
    /// It is what the function's RESERVATION bounds — "Concurrency settings apply to the
    /// function as a whole, including all published versions and the unpublished version"
    /// (`PutFunctionConcurrency`), so every qualifier of the function draws on the one count
    /// — and what the account's concurrency metering charges to that reservation, or to the
    /// account's unreserved pool when the function reserves nothing (see
    /// [`crate::state::concurrency_metering`]). It is claimed and released by the slot one
    /// admitted invocation holds ([`crate::state::execution_slot`]), so what it reports is
    /// the runs that are really in flight.
    pub concurrent_executions: i32,
    /// How many of those executions each QUALIFIER of this function is processing, keyed by
    /// the qualifier the invocation addressed — an alias name, a published version number or
    /// [`VERSION_LATEST`].
    ///
    /// Provisioned concurrency is allocated per alias or version
    /// ([`Self::provisioned_concurrency_configs`]), so deciding whether an invocation is
    /// served by one of those initialised environments is a question about that qualifier's
    /// own in-flight count rather than about the function's. Held and released by the same
    /// slot as [`Self::concurrent_executions`]; a qualifier processing nothing holds no entry.
    pub in_flight_by_qualifier: BTreeMap<String, i32>,
    /// The ARN of the code signing configuration this function deploys under, when one
    /// has been associated with it. It is a function-level setting — the association is
    /// addressed by a `FunctionName` alone, with no qualifier member anywhere on
    /// `PutFunctionCodeSigningConfig`, `GetFunctionCodeSigningConfig` or
    /// `DeleteFunctionCodeSigningConfig` — so it lives on the record beside the
    /// function's other function-level settings, and a function nothing has associated a
    /// configuration with holds none at all.
    pub code_signing_config_arn: Option<String>,
    /// The function's recursive loop detection configuration, one of the model's
    /// `RecursiveLoop` values. It is a function-level setting rather than a
    /// version-specific one — `GetFunctionRecursionConfig` and
    /// `PutFunctionRecursionConfig` both take an `UnqualifiedFunctionName`, whose
    /// pattern admits no qualifier — so it lives on the record beside the function's
    /// other function-level settings and starts at [`crate::state::DEFAULT_RECURSIVE_LOOP`].
    pub recursive_loop: String,
    /// The highest version published so far; 0 when none has been.
    pub latest_version: u64,
    /// The published versions, ordered by version number.
    pub versions: BTreeMap<u64, FunctionVersionRecord>,
    /// The resource-based policy of each qualifier that has one, keyed by the version
    /// label the permission was granted on: the function-level policy under
    /// [`VERSION_LATEST`], and one per published version or alias. A policy is attached
    /// to the ARN the permission names, so two qualifiers never share one.
    pub policies: BTreeMap<String, PolicyRecord>,
    /// The function URL of each qualifier that has one, keyed by the version label the
    /// URL was created on: the unpublished version under [`VERSION_LATEST`], and one
    /// per alias. A function URL is attached to the qualifier it was created on, and
    /// each qualifier has at most one, so a second create on the same qualifier
    /// conflicts rather than replacing it.
    pub url_configs: BTreeMap<String, FunctionUrlConfigRecord>,
    /// The aliases of this function, keyed by alias name so `ListAliases` pages through
    /// them in a stable order. An alias belongs to the function, not to one of its
    /// versions, and names the version it invokes.
    pub aliases: BTreeMap<String, AliasRecord>,
    /// The asynchronous invocations accepted for this function, oldest first, bounded
    /// by [`MAX_QUEUED_ASYNC_INVOCATIONS`].
    pub async_invocations: VecDeque<AsyncInvocationRecord>,
    /// What each request chain this function has been invoked as part of has produced so
    /// far: the bookkeeping Lambda's recursive-loop guardrail counts on, oldest hop first
    /// and bounded by [`crate::state::recursion::MAX_TRACKED_REQUEST_CHAINS`]. Written
    /// through [`Self::count_chain_invocation`]; see [`crate::state::recursion`] for what
    /// it decides.
    pub request_chains: VecDeque<RequestChainRecord>,
    /// The durable executions of this function, keyed by execution name — the name
    /// "must be unique within the function"
    /// (`Invoke$DurableExecutionAlreadyStartedException`), so one name names one
    /// execution however it is addressed, and the map's order is stable. Bounded by
    /// [`MAX_DURABLE_EXECUTIONS`].
    pub durable_executions: BTreeMap<String, DurableExecutionRecord>,
    /// The runtime management configuration of each version that has had one stored,
    /// keyed by the version label it belongs to — [`VERSION_LATEST`] or a published
    /// version number. A version with no entry carries
    /// [`RuntimeManagementConfigRecord::default`]; see
    /// [`Self::runtime_management_config`].
    pub runtime_management_configs: BTreeMap<String, RuntimeManagementConfigRecord>,
    /// The provisioned concurrency configuration of each alias or published version that
    /// has one, keyed by the qualifier the `PutProvisionedConcurrencyConfig` call
    /// addressed. A configuration "is added to a function's alias or version", so an
    /// alias and the version it invokes hold their own, and a qualifier nobody has
    /// configured holds none at all.
    pub provisioned_concurrency_configs: BTreeMap<String, ProvisionedConcurrencyRecord>,
    /// The scaling configuration of each qualifier that has had one stored, keyed by the
    /// qualifier the `PutFunctionScalingConfig` call addressed — a published version
    /// number or [`VERSION_LATEST_PUBLISHED`]. A qualifier nobody has configured holds
    /// none at all.
    pub function_scaling_configs: BTreeMap<String, FunctionScalingConfigRecord>,
    /// The asynchronous-invocation configuration of each qualifier that has had one put on
    /// it, keyed by the qualifier the `PutFunctionEventInvokeConfig` call addressed —
    /// [`VERSION_LATEST`], a published version number or an alias name. The resource is "a
    /// function, version, or alias", so a configuration put on an alias is the alias's own
    /// and never the one the version that alias invokes holds, and a qualifier nobody has
    /// configured holds none at all.
    pub event_invoke_configs: BTreeMap<String, EventInvokeConfigRecord>,
}

impl FunctionRecord {
    /// Bring this record up to whatever the clock has already made due.
    ///
    /// The ONLY transitions it applies are the ones the clock owed at `now`: a function
    /// that has finished provisioning becomes `Active`, and an update that has finished
    /// being applied becomes `Successful`. Both would have been owed at the same instant
    /// even if nobody had called, so applying them on a read reports the truth rather
    /// than inventing a change.
    ///
    /// The instants are COMPARED, never arithmetic: each due instant was computed once,
    /// at the call that started the transition, so nothing here can overflow a
    /// `SystemTime` however long a stored record has been around.
    pub fn materialize(&mut self, now: SystemTime) {
        if self.state == STATE_PENDING && now >= self.active_due {
            self.state = STATE_ACTIVE.to_string();
            self.state_reason = None;
            self.state_reason_code = None;
            // "This is first set to Successful after function creation completes" — an
            // update accepted while the function was still being provisioned is settled
            // by its own due instant below, so creation completing does not settle it.
            if self.last_update_status.is_none() {
                self.last_update_status = Some(LAST_UPDATE_STATUS_SUCCESSFUL.to_string());
            }
        }
        if self.last_update_due.is_some_and(|due| now >= due) {
            self.last_update_status = Some(LAST_UPDATE_STATUS_SUCCESSFUL.to_string());
            self.last_update_due = None;
        }
        // A replaced deployment keeps serving only while an update is in flight, so it is
        // dropped whenever the window is CLOSED rather than only where this reader closed it:
        // the update just applied, and any other way the window ends, leave the package the
        // function was given as the one that serves ([`Self::serving_code`]).
        if self.last_update_due.is_none() {
            self.serving_code = None;
        }
        // The same rule for the asynchronous event queue: a queued event whose delivery
        // attempt the clock has already made due and which NO ATTEMPT IS PENDING for has had
        // no attempt at all, so it was never executed. The outcome would have been owed at
        // the same instant even if nobody had called. An event an attempt is pending for is
        // left alone — whether that attempt has claimed the entry yet
        // ([`AsyncInvocationOutcome::Delivering`]) or is still to be made, including one a
        // throttle put back on the queue — because the attempt records what the run came to
        // ([`super::async_invocation::deliver`]) and the clock owes nothing about a delivery
        // that is under way.
        for invocation in &mut self.async_invocations {
            if invocation.outcome == AsyncInvocationOutcome::Queued
                && !invocation.attempt_pending
                && now >= invocation.attempt_due
            {
                invocation.outcome =
                    AsyncInvocationOutcome::Failed(ASYNC_NOT_DELIVERED_REASON.to_string());
            }
        }
        // And the same rule for every provisioned concurrency allocation under way: the
        // warm-up the clock has already made due is finished, so the allocation is ready.
        for config in self.provisioned_concurrency_configs.values_mut() {
            config.materialize(now);
        }
        // And the same rule for every scaling configuration still being applied: bounds
        // the clock has already made due are the ones in effect.
        for config in self.function_scaling_configs.values_mut() {
            config.materialize(now);
        }
        // And the same rule for every durable execution: one whose `ExecutionTimeout`
        // the clock has already made due has run past it, so it timed out.
        self.materialize_durable_executions(now);
    }

    /// Bring every durable execution of this function up to whatever the clock has already
    /// made due, and NOTHING else.
    ///
    /// Split out of [`FunctionRecord::materialize`] because one reader wants exactly this and
    /// none of the rest: the status-change publication
    /// ([`crate::state::durable_events`]) reports what the executions moved into, and it has
    /// no business settling this function's asynchronous event queue or its provisioned
    /// concurrency on the way past. So the loop lives in one place and both callers run it.
    pub fn materialize_durable_executions(&mut self, now: SystemTime) {
        for execution in self.durable_executions.values_mut() {
            execution.materialize(now);
        }
    }

    /// Record that an update to this function's code or configuration was accepted at
    /// `now`: the model's `LastUpdateStatus` is [`LAST_UPDATE_STATUS_IN_PROGRESS`] until
    /// the clock makes the move to [`LAST_UPDATE_STATUS_SUCCESSFUL`] due,
    /// [`UPDATE_DELAY`] later.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.last_update_status = Some(LAST_UPDATE_STATUS_IN_PROGRESS.to_string());
        self.last_update_due = Some(now + UPDATE_DELAY);
    }

    /// Store `code` as this function's deployment package, keeping the one it replaces
    /// SERVING until the update that is starting completes.
    ///
    /// "During this time, you can't modify the function, but you can still invoke it": an
    /// invocation that arrives inside the update window is served, and what it runs is the
    /// package the function was already serving — the new one takes over when the clock makes
    /// the update's completion due ([`Self::materialize`]). Every REPORT of the function
    /// names the new package at once, which is why it is this field that moves and
    /// [`Self::serving_code`] that remembers.
    pub fn deploy_code(&mut self, code: CodeRecord) {
        let replaced = ServingDeployment {
            revision_id: self.config.revision_id.clone(),
            code: ::std::mem::replace(&mut self.code, code),
        };
        // The window is exclusive — a second deployment inside it is refused with the
        // modelled conflict — so the deployment that was serving when this one arrived is
        // the one just replaced, unless one is already waiting to take over.
        if self.serving_code.is_none() {
            self.serving_code = Some(replaced);
        }
    }

    /// Freeze `config` as this function's next published version, answering the number
    /// it got.
    ///
    /// Versions are numbered from 1 upwards and a number is never reused, so the next
    /// one follows the highest ever published rather than the count held. `None` when no
    /// further number can be assigned.
    ///
    /// The version is a SEPARATE identity from the unpublished one: it gets its own
    /// `RevisionId`, which is what makes a revision precondition mean one identity rather
    /// than two ("The RevisionId provided does not match the latest RevisionId for the
    /// Lambda function or alias"). It also freezes the deployment package the function
    /// currently holds, because a later `UpdateFunctionCode` replaces only the unpublished
    /// version's.
    ///
    /// Publishing is a CHANGE to the function, so it also leaves the unpublished
    /// configuration at a fresh `RevisionId` of its own: a revision names one state of the
    /// function, and the state a caller reads after this call holds a version it did not
    /// hold before. The revision it is left at is kept beside the version it published as
    /// [`FunctionVersionRecord::unpublished_revision_id`], so the "nothing changed,
    /// nothing published" rule can still ask whether anything has changed since.
    pub fn publish_next_version(
        &mut self,
        mut config: FunctionConfig,
        now: SystemTime,
    ) -> Option<u64> {
        let version = self.latest_version.checked_add(1)?;
        config.revision_id = mint_revision_id();
        let unpublished_revision_id = mint_revision_id();
        self.config.revision_id = unpublished_revision_id.clone();
        self.versions.insert(
            version,
            FunctionVersionRecord {
                version,
                config,
                unpublished_revision_id,
                code: self.code.clone(),
                created: iso8601(now),
            },
        );
        self.latest_version = version;
        Some(version)
    }

    /// Whether an update of this function is still being applied — the window
    /// `UpdateFunctionConfiguration` documents as "During this time, you can't modify
    /// the function, but you can still invoke it".
    ///
    /// Read it only after [`FunctionRecord::materialize`], so an update the clock has
    /// already settled is not reported as still in flight.
    #[must_use]
    pub fn update_in_progress(&self) -> bool {
        self.last_update_due.is_some()
    }

    /// Queue one asynchronous invocation of `version` carrying `payload`, accepted at
    /// `now` under the request id `request_id`.
    ///
    /// The id is the one the RUN this event is delivered to is given as its
    /// `context.aws_request_id`, so the delivery attempt finds the entry it has to record
    /// its outcome on by it.
    ///
    /// The queue is bounded by [`MAX_QUEUED_ASYNC_INVOCATIONS`]: once it is full the
    /// oldest entry makes room for the newest, so accepting an event never fails for
    /// want of memory.
    pub fn queue_async_invocation(
        &mut self,
        version: &str,
        request_id: &str,
        payload: Vec<u8>,
        now: SystemTime,
    ) {
        self.hold_async_invocation(AsyncInvocationRecord {
            version: version.to_string(),
            request_id: request_id.to_string(),
            payload,
            received: iso8601(now),
            // When Lambda sends this event to the function: after the queueing delay, and
            // never before the function is INVOCABLE. "For asynchronous invocation, Lambda
            // adds events to a queue before sending them to your function" (`Invoke`), and
            // the function cannot be invoked while it is still being provisioned ("During
            // this time, you can't invoke or modify the function", `CreateFunction`) — so an
            // event accepted inside that window waits on the queue until the instant the
            // clock already owes the move to `Active` ([`Self::active_due`]) and is sent to
            // the function then. COMPARED, never arithmetic on a stored instant: both
            // instants exist already.
            attempt_due: (now + ASYNC_DELIVERY_DELAY).max(self.active_due),
            attempts: 0,
            // Whether an attempt is coming for this event is [`deliver`]'s to say, which the
            // accepting call asks it right after this: a queued event nothing is delivering
            // is one the clock settles.
            attempt_pending: false,
            outcome: AsyncInvocationOutcome::Queued,
        });
    }

    /// Record one asynchronous invocation of `version` carrying `payload`, accepted at
    /// `now`, that Lambda's recursive-loop guardrail STOPPED
    /// ([`crate::state::recursion::Guardrail::Stopped`]).
    ///
    /// It is held like any other accepted event, and settled from the start: the guardrail
    /// "stops your function being invoked", so there is no delivery attempt still to come
    /// and the entry carries [`ASYNC_RECURSION_STOPPED_REASON`] rather than waiting to
    /// record that nothing ran.
    pub fn record_stopped_async_invocation(
        &mut self,
        version: &str,
        request_id: &str,
        payload: Vec<u8>,
        now: SystemTime,
    ) {
        self.hold_async_invocation(AsyncInvocationRecord {
            version: version.to_string(),
            request_id: request_id.to_string(),
            payload,
            received: iso8601(now),
            attempt_due: now,
            attempts: 0,
            // The guardrail "stops your function being invoked", so there is no attempt to
            // come for this event and none pending.
            attempt_pending: false,
            outcome: AsyncInvocationOutcome::Failed(ASYNC_RECURSION_STOPPED_REASON.to_string()),
        });
    }

    /// Hold one accepted asynchronous invocation, keeping the queue inside
    /// [`MAX_QUEUED_ASYNC_INVOCATIONS`] by making room with the oldest entry.
    fn hold_async_invocation(&mut self, invocation: AsyncInvocationRecord) {
        self.async_invocations.push_back(invocation);
        while self.async_invocations.len() > MAX_QUEUED_ASYNC_INVOCATIONS {
            self.async_invocations.pop_front();
        }
    }

    /// The ARN of `version` of this function: the unqualified ARN for `$LATEST`, and
    /// the `:<version>`-suffixed one for a published version, exactly as the model's
    /// `NameSpacedFunctionArn` pattern allows.
    ///
    /// The spelling itself is [`super::qualified_arn`], which a refusal naming a function
    /// this store holds NO record of shares, so the two cannot drift.
    #[must_use]
    pub fn versioned_arn(&self, version: &str) -> String {
        super::qualified_arn(&self.arn, version)
    }

    /// The model's `State` reported for the configuration of `version`, which is this
    /// function's lifecycle state narrowed by whether that configuration can be invoked
    /// directly.
    ///
    /// The UNPUBLISHED configuration of a function that names a capacity provider cannot
    /// be: the compute it runs on is the provider's, and what the provider runs is a
    /// published version registered with it, so a settled function is provisioned and
    /// healthy without `$LATEST` being invocable. The `State` enum's own
    /// [`STATE_ACTIVE_NON_INVOCABLE`] is the value for exactly that. Every other
    /// configuration reports the lifecycle state as it stands, and so does one that has
    /// not settled yet: a function still being provisioned is [`STATE_PENDING`] whatever
    /// it will be invocable as.
    ///
    /// `version` is read through [`Self::resolved_version`], so an alias reports the state
    /// of the version it invokes.
    ///
    /// Read it only after [`Self::materialize`], so the state reported is the one the
    /// clock has already made due.
    #[must_use]
    pub fn reported_state(&self, version: &str) -> String {
        if self.state == STATE_ACTIVE
            && self.resolved_version(version).as_deref() == Some(VERSION_LATEST)
            && configured_capacity_provider(&self.config).is_some()
        {
            return STATE_ACTIVE_NON_INVOCABLE.to_string();
        }
        self.state.clone()
    }

    /// Start a durable execution named `name` against `version` at `now`, carrying
    /// `payload` as its input, and report what became of the start.
    ///
    /// `Invoke$DurableExecutionName` states the whole rule: "If you invoke a durable
    /// function using a name that already exists with the same payload, Lambda returns the
    /// existing execution instead of creating a duplicate. If the payload differs, Lambda
    /// returns a DurableExecutionAlreadyStartedException error." So a name this function
    /// already holds never starts a second execution and never replaces the one held —
    /// [`DurableExecutionStart::AlreadyStarted`] answers the stored execution's ARN for the
    /// repeat that carries the same payload, and
    /// [`DurableExecutionStart::NameAlreadyStarted`] is the state the modelled
    /// `DurableExecutionAlreadyStartedException` describes ("Each durable execution name
    /// must be unique within the function") for the one that does not. A function already
    /// holding [`MAX_DURABLE_EXECUTIONS`] of them answers
    /// [`DurableExecutionStart::LimitExceeded`], the ceiling that keeps the map (and the
    /// per-read walk over it in [`FunctionRecord::materialize`]) bounded.
    ///
    /// The execution's settings come from the function's own `DurableConfig`: the key it
    /// encrypts payload data under, and the `ExecutionTimeout` whose due instant is
    /// stored here once rather than recomputed on every read.
    ///
    /// Started for NO tenant: the start an invocation carrying a `TenantId` makes is
    /// [`FunctionRecord::start_durable_execution_for_tenant`], which scopes the whole rule
    /// above to the tenant that owns the execution.
    pub fn start_durable_execution(
        &mut self,
        name: &str,
        version: &str,
        payload: &[u8],
        now: SystemTime,
    ) -> DurableExecutionStart {
        self.start_durable_execution_for_tenant(name, version, payload, None, now)
    }

    /// [`FunctionRecord::start_durable_execution`] for the invocation of a TENANT.
    ///
    /// An execution is the state of ONE tenant's work, and what a de-duplicated repeat is
    /// answered with is that execution's own ARN and, later, its own result — so the
    /// de-duplication above holds only between starts of the SAME tenant. A start under a name
    /// another tenant already holds is [`DurableExecutionStart::NameAlreadyStarted`]: the name
    /// is taken within the function, which is the state the modelled
    /// `DurableExecutionAlreadyStartedException` describes, and nothing of the other tenant's
    /// execution is reported to this caller.
    pub fn start_durable_execution_for_tenant(
        &mut self,
        name: &str,
        version: &str,
        payload: &[u8],
        tenant_id: Option<&str>,
        now: SystemTime,
    ) -> DurableExecutionStart {
        if let Some(held) = self.durable_executions.get(name) {
            if held.tenant_id.as_deref() != tenant_id {
                return DurableExecutionStart::NameAlreadyStarted(None);
            }
            if held.input_payload == payload {
                return DurableExecutionStart::AlreadyStarted(self.durable_execution_arn(held));
            }
            return DurableExecutionStart::NameAlreadyStarted(Some(
                self.durable_execution_arn(held),
            ));
        }
        if self.durable_executions.len() >= MAX_DURABLE_EXECUTIONS {
            return DurableExecutionStart::LimitExceeded;
        }
        let durable_config = self
            .config_of(version)
            .and_then(|config| config.durable_config.clone());
        // The instant the execution started, at the resolution every report of it shares
        // ([`crate::state::execution_instant`]): the `StartTimestamp` a read answers and the
        // start instant every status-change event of this execution carries are one instant,
        // and the `ExecutionTimeout` falls due a whole number of seconds after it.
        let now = crate::state::execution_instant(now);
        let mut execution = DurableExecutionRecord {
            name: name.to_string(),
            tenant_id: tenant_id.map(str::to_string),
            execution_id: mint_durable_execution_id(),
            version: version.to_string(),
            status: EXECUTION_STATUS_RUNNING.to_string(),
            started: now,
            input_payload: payload.to_vec(),
            ended: None,
            error: None,
            // The result is the one the RUN produces, so a freshly started execution has
            // none (see [`DurableExecutionRecord::settle`]).
            result: None,
            kms_key_arn: durable_config
                .as_ref()
                .and_then(|config| config.kms_key_arn.clone()),
            timeout_due: durable_config
                .as_ref()
                .and_then(|config| config.execution_timeout)
                .and_then(|seconds| u64::try_from(seconds).ok())
                // CHECKED, because the seconds come from a stored configuration: a
                // timeout no `SystemTime` can represent falls due at no instant rather
                // than at a panic.
                .and_then(|seconds| now.checked_add(Duration::from_secs(seconds))),
            history: Vec::new(),
            // The token the execution's FIRST checkpoint must carry: "a unique token that
            // identifies the current checkpoint state", which the Lambda runtime driving the
            // execution is handed and every later checkpoint rotates
            // ([`crate::state::durable_checkpoint`]).
            checkpoint_token: crate::state::mint_checkpoint_token(),
            // Nothing has been checkpointed yet, so no duplicate request can be identified.
            answered_checkpoint: None,
            // Nor has the run taken any wait for something outside itself, so there is no
            // callback for a client to signal yet (`crate::state::durable_callbacks`).
            callbacks: BTreeMap::new(),
        };
        // The history opens with the event the start itself is: "The input payload
        // provided for the durable execution" and "the maximum amount of time that the
        // durable execution is allowed to run, in seconds", which is the
        // `ExecutionTimeout` of the function's `DurableConfig`. An execution of a
        // function whose durable configuration names no timeout runs under none, and the
        // event reports the `DurationSeconds` floor of 0 rather than a limit nothing
        // bounds it by.
        let mut started = execution.next_event(EVENT_TYPE_EXECUTION_STARTED, now);
        started.execution_started_details = Some(ExecutionStartedDetails {
            input: event_payload(payload),
            execution_timeout: durable_config
                .as_ref()
                .and_then(|config| config.execution_timeout)
                .unwrap_or(0),
        });
        execution.history.push(started);
        let arn = self.durable_execution_arn(&execution);
        self.durable_executions.insert(name.to_string(), execution);
        DurableExecutionStart::Started(arn)
    }

    /// The ARN `execution` is addressed by, in the one form the model's
    /// `DurableExecutionArn` pattern admits: the function ARN qualified by the version
    /// the execution runs under — qualified even for `$LATEST`, which that pattern
    /// requires — then the execution's name and id under
    /// [`DURABLE_EXECUTION_ARN_INFIX`].
    #[must_use]
    pub fn durable_execution_arn(&self, execution: &DurableExecutionRecord) -> String {
        format!(
            "{}/{DURABLE_EXECUTION_ARN_INFIX}/{}/{}",
            self.durable_execution_function_arn(execution),
            execution.name,
            execution.execution_id
        )
    }

    /// The ARN of the function VERSION `execution` runs on, as a durable execution names the
    /// function it belongs to: this function's ARN qualified by that version, `$LATEST`
    /// included.
    ///
    /// It is the function part of the execution's own ARN ([`Self::durable_execution_arn`]),
    /// whose pattern carries the qualifier for every version — so the function a durable
    /// execution's status-change event names and the function its ARN names are one string,
    /// spelled once here.
    #[must_use]
    pub fn durable_execution_function_arn(&self, execution: &DurableExecutionRecord) -> String {
        format!("{}:{}", self.arn, execution.version)
    }

    /// The `Execution` the wire reports for `execution`, built entirely out of what is
    /// stored: the ARN it is addressed by, the function version it runs under, its
    /// status and the instants it started and (once it has one) ended.
    #[must_use]
    pub fn listed_durable_execution(&self, execution: &DurableExecutionRecord) -> Execution {
        Execution {
            durable_execution_arn: self.durable_execution_arn(execution),
            durable_execution_name: execution.name.clone(),
            function_arn: self.versioned_arn(&execution.version),
            status: execution.status.clone(),
            start_timestamp: ::aws_smithy_types::DateTime::from(execution.started),
            end_timestamp: execution.ended.map(::aws_smithy_types::DateTime::from),
            kms_key_arn: execution.kms_key_arn.clone(),
        }
    }

    /// How this function's `version` is reported as a user of the capacity provider its
    /// configuration names: the ARN of that version, the function and version it
    /// belongs to, and the lifecycle `State` of the function the version is part of
    /// (which is a property of the function rather than of one version, see
    /// [`FunctionRecord::state`]).
    ///
    /// That is the LIFECYCLE state, not the narrowed [`FunctionRecord::reported_state`]
    /// the unpublished configuration reports: a version registered with a capacity
    /// provider is the one thing that provider runs, so this listing reports it as the
    /// live resource it is.
    #[must_use]
    pub fn capacity_provider_version(&self, version: &str) -> CapacityProviderVersion {
        CapacityProviderVersion {
            function_arn: self.versioned_arn(version),
            function_name: self.config.name.clone(),
            version: version.to_string(),
            state: self.state.clone(),
        }
    }

    /// The version label `qualifier` names, or `None` when this function has no such
    /// version or alias.
    ///
    /// [`VERSION_LATEST`] and a published version number name themselves; every other
    /// qualifier names an alias, and an alias names the version it invokes — "use
    /// aliases to provide clients with a function identifier that you can update to
    /// invoke a different version" (`CreateAlias`).
    #[must_use]
    pub fn resolved_version(&self, qualifier: &str) -> Option<String> {
        if qualifier == VERSION_LATEST {
            return Some(VERSION_LATEST.to_string());
        }
        if let Ok(number) = qualifier.parse::<u64>() {
            return self
                .versions
                .contains_key(&number)
                .then(|| number.to_string());
        }
        self.aliases
            .get(qualifier)
            .map(|alias| alias.function_version.clone())
    }

    /// The names of this function's aliases that reference the version label `version`,
    /// in the order the store keeps them (by alias name).
    ///
    /// An alias references a version two ways, and both count: the version it invokes
    /// ([`AliasRecord::function_version`]) and the second version its routing
    /// configuration shifts part of its traffic to
    /// ([`AliasRecord::additional_version_weights`], the model's
    /// `AliasRoutingConfiguration$AdditionalVersionWeights`). Either reference makes an
    /// invocation of the alias reach that version, so either one is a live reference to
    /// it.
    #[must_use]
    pub fn aliases_referencing(&self, version: &str) -> Vec<String> {
        self.aliases
            .values()
            .filter(|alias| {
                alias.function_version == version
                    || alias.additional_version_weights.contains_key(version)
            })
            .map(|alias| alias.name.clone())
            .collect()
    }

    /// The qualifier label a `PublishedFunctionQualifier` names on this function, or
    /// `None` when the function has no such qualifier.
    ///
    /// That shape admits exactly two forms, and they resolve differently:
    /// [`VERSION_LATEST_PUBLISHED`] names the published view of the unpublished
    /// configuration, which every function has, while a number names a version only once
    /// that version has been published. An alias is not among them — the pattern admits
    /// no alias name — so [`Self::resolved_version`] is not what reads this qualifier.
    #[must_use]
    pub fn published_qualifier(&self, qualifier: &str) -> Option<String> {
        if qualifier == VERSION_LATEST_PUBLISHED {
            return Some(VERSION_LATEST_PUBLISHED.to_string());
        }
        let number = qualifier.parse::<u64>().ok()?;
        self.versions
            .contains_key(&number)
            .then(|| number.to_string())
    }

    /// The ARN of the qualifier `published_qualifier` names, as the model's `FunctionArn`
    /// spells it.
    ///
    /// That pattern admits `$LATEST` or a published version as an ARN's qualifier but no
    /// `.`-bearing label, so the published view of the unpublished configuration is
    /// reported by the function's unqualified ARN, and a published version by its own
    /// qualified one.
    #[must_use]
    pub fn published_qualifier_arn(&self, published_qualifier: &str) -> String {
        if published_qualifier == VERSION_LATEST_PUBLISHED {
            return self.arn.clone();
        }
        self.versioned_arn(published_qualifier)
    }

    /// The configuration the qualifier a `PublishedFunctionQualifier` names holds:
    /// [`VERSION_LATEST_PUBLISHED`] is the published view of the unpublished
    /// configuration, and a number names the configuration that version froze.
    ///
    /// Read it only through a qualifier [`Self::published_qualifier`] resolved, which is
    /// what establishes that the qualifier exists at all.
    #[must_use]
    pub fn published_qualifier_config(&self, published_qualifier: &str) -> Option<&FunctionConfig> {
        if published_qualifier == VERSION_LATEST_PUBLISHED {
            return Some(&self.config);
        }
        self.config_of(published_qualifier)
    }

    /// What a read of one qualifier's scaling configuration reports, built entirely out of
    /// what is stored.
    ///
    /// A qualifier a `PutFunctionScalingConfig` call has set bounds on reports that record
    /// ([`FunctionScalingConfigRecord::reported`]). A qualifier nobody has configured
    /// reports the configuration the SERVICE manages for it: a Managed Instances function
    /// is scaled by the service from the moment it exists, so its requested configuration
    /// is [`service_managed_function_scaling_config`]. Neither reports an APPLIED
    /// configuration before one is in effect.
    ///
    /// Read it only after [`Self::materialize`], so bounds the clock has already applied
    /// are not reported as still only requested, and only through a qualifier
    /// [`Self::require_capacity_provider_configuration`] has admitted, which is what
    /// establishes that the service scales this qualifier at all.
    #[must_use]
    pub fn reported_function_scaling_config(
        &self,
        published_qualifier: &str,
    ) -> GetFunctionScalingConfigOutput {
        let function_arn = self.published_qualifier_arn(published_qualifier);
        if let Some(stored) = self.function_scaling_configs.get(published_qualifier) {
            return stored.reported(function_arn);
        }
        GetFunctionScalingConfigOutput {
            function_arn: Some(function_arn),
            applied_function_scaling_config: None,
            requested_function_scaling_config: Some(service_managed_function_scaling_config()),
        }
    }

    /// Admit a function-scaling call on the qualifier it resolved, or refuse it because
    /// that qualifier runs on no capacity provider.
    ///
    /// Both function-scaling operations are documented for a Lambda Managed Instances
    /// function — one whose configuration names a capacity provider — so this is the
    /// AUTHORIZATION step they share: it runs once the function and the qualifier have
    /// resolved, and a qualifier whose configuration names no provider is refused
    /// [`NO_CAPACITY_PROVIDER_MESSAGE`] rather than answered for. The configuration read
    /// is the QUALIFIER's own, because a published version froze the one it was published
    /// from.
    pub fn require_capacity_provider_configuration(
        &self,
        published_qualifier: &str,
    ) -> Result<(), crate::scaffolded::AccessDeniedException> {
        if self
            .published_qualifier_config(published_qualifier)
            .and_then(configured_capacity_provider)
            .is_some()
        {
            return Ok(());
        }
        Err(crate::scaffolded::AccessDeniedException::new(
            NO_CAPACITY_PROVIDER_MESSAGE,
        ))
    }

    /// The configuration of `version`, or `None` when this function has no such
    /// version. An alias resolves to the configuration of the version it invokes; see
    /// [`Self::resolved_version`].
    #[must_use]
    pub fn config_of(&self, version: &str) -> Option<&FunctionConfig> {
        let resolved = self.resolved_version(version)?;
        if resolved == VERSION_LATEST {
            return Some(&self.config);
        }
        resolved
            .parse::<u64>()
            .ok()
            .and_then(|number| self.versions.get(&number))
            .map(|published| &published.config)
    }

    /// The `AliasConfiguration` the wire reports for one of this function's aliases,
    /// built entirely out of what is stored.
    ///
    /// The model declares ONE `AliasConfiguration`, generated here as
    /// [`CreateAliasOutput`]; an operation whose output is generated as its own struct
    /// over that shape carries these members across one for one rather than rebuilding
    /// them from state a second time.
    #[must_use]
    pub fn alias_configuration(&self, alias: &AliasRecord) -> CreateAliasOutput {
        CreateAliasOutput {
            alias_arn: Some(self.versioned_arn(&alias.name)),
            name: Some(alias.name.clone()),
            function_version: Some(alias.function_version.clone()),
            // `Description` is a member of every `AliasConfiguration` AWS reports — the
            // alias's own read, the write that answers it and each listing entry: an
            // alias created without one carries it as the empty string rather than not
            // at all, exactly as a function configuration does (see
            // [`FunctionRecord::configuration`]), so a client can read the member
            // unconditionally. The member's own bound (`@length(min: 0, max: 256)`)
            // admits the empty string, so "no description" and "the empty description"
            // are one value on the wire.
            description: Some(alias.description.clone().unwrap_or_default()),
            routing_config: alias.routing_config(),
            revision_id: Some(alias.revision_id.clone()),
        }
    }

    /// The `FunctionConfiguration` the wire reports for `version`, built entirely out
    /// of what is stored.
    #[must_use]
    pub fn configuration(&self, version: &str, config: &FunctionConfig) -> CreateFunctionOutput {
        // `LastUpdateStatus` is "The status of the last update that was performed on the
        // function", and the update `UpdateFunctionConfiguration` describes is applied to
        // the UNPUBLISHED version only ("You can't modify the configuration of a published
        // version, only the unpublished version"). A published version froze its
        // configuration before any such update began, so it is not the thing being updated
        // and reports the settled status rather than the in-flight one.
        let last_update_status = if version == VERSION_LATEST {
            self.last_update_status.clone()
        } else {
            self.last_update_status.as_ref().map(|status| {
                if status == LAST_UPDATE_STATUS_IN_PROGRESS {
                    LAST_UPDATE_STATUS_SUCCESSFUL.to_string()
                } else {
                    status.clone()
                }
            })
        };
        // "The LastUpdateStatus, LastUpdateStatusReason, and LastUpdateStatusReasonCode
        // fields in the response from GetFunctionConfiguration indicate when the update is
        // complete": the three members report ONE thing, so the reason and its code are
        // reported exactly while the status is the in-flight one, and the work in flight is
        // the same provisioning of a new instance of the function that creation performs
        // ("When you update a function, Lambda provisions an instance of the function and
        // its supporting resources"), which is what [`STATE_REASON_CODE_CREATING`] and its
        // sentence name. A settled or failed status has no work in flight to report, so it
        // carries neither member.
        let update_in_flight = last_update_status.as_deref() == Some(LAST_UPDATE_STATUS_IN_PROGRESS);
        CreateFunctionOutput {
            function_name: Some(config.name.clone()),
            function_arn: Some(self.versioned_arn(version)),
            runtime: config.runtime.clone(),
            role: Some(config.role.clone()),
            handler: config.handler.clone(),
            code_size: config.code_size,
            // `Description` is a member of every `FunctionConfiguration` AWS reports: a
            // function created without one carries it as the empty string rather than
            // not at all, so a client can read it unconditionally.
            description: Some(config.description.clone().unwrap_or_default()),
            timeout: Some(config.timeout),
            memory_size: Some(config.memory_size),
            last_modified: Some(config.last_modified.clone()),
            code_sha256: config.code_sha256.clone(),
            // The `Version` member's own pattern (`^(\$LATEST|[0-9]+)$`) admits no alias
            // name, so a request addressed by an alias reports the version that alias
            // invokes, while the ARN above keeps the qualifier the caller named.
            version: Some(
                self.resolved_version(version)
                    .unwrap_or_else(|| version.to_string()),
            ),
            // `FunctionConfiguration$Environment` reports "the function's environment
            // variables", so it is reported exactly while the configuration HOLDS one: a
            // configuration with no variable in it has no environment to report, whether
            // the create named the member or an update emptied it, and the member is part
            // of no answer about it.
            environment: (!config.environment.is_empty()).then(|| EnvironmentResponse {
                variables: Some(
                    config
                        .environment
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect(),
                ),
                error: None,
            }),
            kms_key_arn: config.kms_key_arn.clone(),
            tracing_config: Some(TracingConfigResponse {
                mode: Some(config.tracing_mode.clone()),
            }),
            revision_id: Some(config.revision_id.clone()),
            // A function the caller named no layer for carries no `Layers` member at
            // all: the model gives it no default, so an empty list is not a value this
            // configuration holds.
            layers: (!config.layers.is_empty()).then(|| {
                config
                    .layers
                    .iter()
                    .map(|layer| Layer {
                        arn: Some(layer.arn.clone()),
                        code_size: layer.code_size,
                        // `UncompressedCodeSize` is the size of that layer version's
                        // archive once expanded, which is what a client sums its layers'
                        // sizes over to see how much of Lambda's unzipped-size limit the
                        // function has left; it is measured off the stored archive when the
                        // version is attached (`crate::state::FunctionLayer`).
                        uncompressed_code_size: layer.uncompressed_code_size,
                        ..Layer::default()
                    })
                    .collect()
            }),
            // `State` is the state of the configuration being reported: the unpublished
            // configuration of a function that runs on a capacity provider is not
            // directly invocable, so it settles into `ActiveNonInvocable` rather than
            // `Active` (see [`FunctionRecord::reported_state`]).
            state: Some(self.reported_state(version)),
            state_reason: self.state_reason.clone(),
            state_reason_code: self.state_reason_code.clone(),
            last_update_status,
            last_update_status_reason: update_in_flight
                .then(|| STATE_REASON_CREATING.to_string()),
            last_update_status_reason_code: update_in_flight
                .then(|| STATE_REASON_CODE_CREATING.to_string()),
            package_type: Some(config.package_type.clone()),
            architectures: Some(config.architectures.clone()),
            ephemeral_storage: Some(EphemeralStorage {
                size: config.ephemeral_storage_mb,
            }),
            // `VpcConfig` goes in and `VpcConfigResponse` comes out: the two differ only
            // by the `VpcId` of the VPC the subnets belong to, which lives in EC2 and is
            // resolved against that inventory by the write that applied the configuration
            // (`crate::state::vpc_interfaces`), so what is reported here is an id that was
            // looked up rather than one the request carried.
            vpc_config: config.vpc_config.as_ref().map(|vpc| VpcConfigResponse {
                subnet_ids: vpc.subnet_ids.clone(),
                security_group_ids: vpc.security_group_ids.clone(),
                vpc_id: config.vpc_id.clone(),
                // "Allows outbound IPv6 traffic on VPC functions that are connected to
                // dual-stack subnets" (`VpcConfigResponse$Ipv6AllowedForDualStack`): a
                // SETTING of the attachment, which every attachment therefore has — it is
                // off on one whose request did not turn it on, and is reported as off rather
                // than omitted, so a client reads the setting unconditionally instead of
                // inferring it from a missing member.
                ipv6_allowed_for_dual_stack: Some(
                    vpc.ipv6_allowed_for_dual_stack.unwrap_or(false),
                ),
            }),
            dead_letter_config: config.dead_letter_config.clone(),
            // A function the caller mounted no file system on carries no
            // `FileSystemConfigs` member at all, for the same reason as `Layers` above.
            file_system_configs: (!config.file_system_configs.is_empty())
                .then(|| config.file_system_configs.clone()),
            // `ImageConfigResponse` wraps the stored overrides together with the error
            // of resolving them; nothing here resolves an image, so there is no error
            // to report.
            image_config_response: config.image_config.clone().map(|image_config| {
                ImageConfigResponse {
                    image_config: Some(image_config),
                    error: None,
                }
            }),
            // "When you provide a qualified Amazon Resource Name (ARN), this response
            // element indicates whether SnapStart is activated for the specified function
            // version" — so every read reports `SnapStart`, and a function the caller set
            // no `ApplyOn` for carries the documented default (`None`: nothing is
            // snapshotted), exactly as AWS reports it.
            snap_start: {
                let apply_on = config
                    .snap_start_apply_on
                    .clone()
                    .unwrap_or_else(|| SNAP_START_APPLY_ON_NONE.to_string());
                Some(SnapStartResponse {
                    optimization_status: Some(
                        if version != VERSION_LATEST
                            && apply_on == SNAP_START_APPLY_ON_PUBLISHED_VERSIONS
                        {
                            SNAP_START_OPTIMIZATION_STATUS_ON
                        } else {
                            SNAP_START_OPTIMIZATION_STATUS_OFF
                        }
                        .to_string(),
                    ),
                    apply_on: Some(apply_on),
                })
            },
            // AWS reports a function's CloudWatch Logs configuration on every read, so a
            // function the caller gave none for reports the documented defaults rather
            // than no member: "The default log format is plain text", and "By default,
            // Lambda functions send logs to a default log group named
            // /aws/lambda/<function name>".
            //
            // The two LEVEL members are the filters the JSON format carries: AWS documents
            // them as available with that format, each defaulting to INFO, so a function on
            // the JSON format reports both — the levels really in effect. A function on the
            // plain-text format reports whatever is stored for them and nothing defaulted,
            // because that format filters by no level at all.
            //
            // Every one of the four is read through [`crate::state::invocation_logs`], the one
            // reading of this configuration, so what a read REPORTS and what a run's records
            // are DELIVERED under cannot disagree.
            logging_config: {
                let logging = config.logging_config.as_ref();
                Some(LoggingConfig {
                    application_log_level: configured_application_log_level(logging),
                    system_log_level: configured_system_log_level(logging),
                    log_group: Some(configured_log_group(logging, &config.name)),
                    log_format: Some(configured_log_format(logging).to_string()),
                })
            },
            // The runtime version the function is pinned to. A container image carries no
            // managed runtime, so there is no runtime version to report for one.
            runtime_version_config: config.runtime.as_deref().map(|runtime| {
                RuntimeVersionConfig {
                    runtime_version_arn: Some(runtime_version_arn(&self.region, runtime)),
                    error: None,
                }
            }),
            capacity_provider_config: config.capacity_provider_config.clone(),
            durable_config: config.durable_config.clone(),
            tenancy_config: config.tenancy_config.clone(),
            // "The ARN of the signing profile version" and "The ARN of the signing job" of
            // the deployment package this configuration holds: both are read off the
            // signature the package itself carried when it was deployed
            // ([`crate::state::verified_package_signature`]), so a function reports the
            // publisher of the code it is serving and an unsigned one reports neither
            // member.
            signing_profile_version_arn: config.signing_profile_version_arn.clone(),
            signing_job_arn: config.signing_job_arn.clone(),
            ..CreateFunctionOutput::default()
        }
    }

    /// The `FunctionConfiguration` reported by a READ addressed by `named_qualifier` —
    /// the version or alias the request itself named, through either half of the
    /// `FunctionName`/`Qualifier` pair ([`crate::state::named_qualifier`]).
    ///
    /// It is [`Self::configuration`] with the one member that depends on how the resource
    /// was ADDRESSED: `FunctionArn` targets `NameSpacedFunctionArn`, whose pattern admits a
    /// trailing qualifier (`…:function:<name>(:($LATEST(.PUBLISHED)?|<alias-or-version>))?`),
    /// and a read reports the qualified ARN exactly when the request qualified the
    /// identity it asked about — `$LATEST` named explicitly included, which is the one
    /// qualifier [`Self::versioned_arn`] cannot tell from an unqualified read, because
    /// that is the version an unqualified read resolves to. A request that named no
    /// qualifier addressed the function itself and reports its unqualified ARN.
    #[must_use]
    pub fn addressed_configuration(
        &self,
        named_qualifier: Option<&str>,
        version: &str,
        config: &FunctionConfig,
    ) -> CreateFunctionOutput {
        let mut reported = self.configuration(version, config);
        if let Some(named) = named_qualifier {
            reported.function_arn = Some(configured_function_arn(&self.arn, named));
        }
        reported
    }

    /// The `FunctionConfiguration` reported by a call whose SUBJECT is the FUNCTION — one
    /// addressed by a `FunctionName` carrying no qualifier, which acted on the function
    /// itself and published `version` only as a side effect of its own `Publish`
    /// parameter.
    ///
    /// It is [`Self::configuration`] with the one member that differs: `FunctionArn` is
    /// "The function's Amazon Resource Name (ARN)" — the unqualified ARN of the resource
    /// the call acted on — while the version the same call published is named by `Version`
    /// alone, so a caller that forms a version ARN qualifies what it was given once rather
    /// than twice. A call whose subject is a QUALIFIER instead (`PublishVersion`, which
    /// creates the version; a read or a listing entry addressed by a version or an alias)
    /// reports that qualifier's own ARN through [`Self::configuration`].
    #[must_use]
    pub fn unqualified_configuration(
        &self,
        version: &str,
        config: &FunctionConfig,
    ) -> CreateFunctionOutput {
        CreateFunctionOutput {
            function_arn: Some(self.arn.clone()),
            ..self.configuration(version, config)
        }
    }

    /// The runtime management configuration of the version `qualifier` names.
    ///
    /// A version nobody has configured carries the documented default
    /// ([`RuntimeManagementConfigRecord::default`]), so every version always has one —
    /// the configuration is a value, not a resource that may be absent. An alias reads
    /// the configuration of the version it invokes, because the configuration belongs
    /// to the version; see [`Self::resolved_version`].
    #[must_use]
    pub fn runtime_management_config(&self, qualifier: &str) -> RuntimeManagementConfigRecord {
        self.runtime_management_configs
            .get(&self.runtime_management_key(qualifier))
            .cloned()
            .unwrap_or_default()
    }

    /// Store `config` as the runtime management configuration of the version
    /// `qualifier` names, replacing whatever that version held: the configuration is
    /// one value a version always has rather than a list to append to.
    pub fn set_runtime_management_config(
        &mut self,
        qualifier: &str,
        config: RuntimeManagementConfigRecord,
    ) {
        let key = self.runtime_management_key(qualifier);
        self.runtime_management_configs.insert(key, config);
    }

    /// The version label a runtime management configuration is keyed by: the version
    /// `qualifier` resolves to, so a call addressed by an alias and one addressed by
    /// the version that alias invokes reach the same configuration.
    fn runtime_management_key(&self, qualifier: &str) -> String {
        self.resolved_version(qualifier)
            .unwrap_or_else(|| qualifier.to_string())
    }

    /// The deployment package an INVOCATION of `version` runs.
    ///
    /// It is the package of that version ([`Self::code_of`]) except inside a code update's
    /// window on the unpublished version, where the previous deployment keeps serving:
    /// "During this time, you can't modify the function, but you can still invoke it" (see
    /// [`Self::serving_code`]). A published version froze its own package, so no update of
    /// the unpublished version changes what one runs.
    ///
    /// Read it only after [`Self::materialize`], so an update the clock has already settled
    /// is not still serving the package it replaced.
    #[must_use]
    pub fn serving_code_of(&self, version: &str) -> &CodeRecord {
        match (self.published_code_of(version), &self.serving_code) {
            (Some(published), _) => published,
            (None, Some(serving)) => &serving.code,
            (None, None) => &self.code,
        }
    }

    /// The REVISION an invocation of `version` runs at, given the revision its resolved
    /// configuration is at.
    ///
    /// It is that revision, except inside a code update's window on the unpublished version:
    /// what serves there is the deployment the update replaced, so the invocation runs at the
    /// revision THAT deployment was made at. The revision is what says which execution
    /// environments may serve an invocation
    /// ([`crate::state::execution::warm_pool::EnvironmentKey`]), so naming the new one for a
    /// run of the old package would let an environment built from the old package serve
    /// invocations once the new one has taken over.
    #[must_use]
    pub fn serving_revision_of(&self, version: &str, resolved_revision: &str) -> String {
        match (self.published_code_of(version), &self.serving_code) {
            (None, Some(serving)) => serving.revision_id.clone(),
            _ => resolved_revision.to_string(),
        }
    }

    /// The deployment package of `version`: the one a published version froze, and the
    /// current one for the unpublished version. An alias reads the package of the version
    /// it invokes, because the package belongs to the version.
    #[must_use]
    pub fn code_of(&self, version: &str) -> &CodeRecord {
        self.published_code_of(version).unwrap_or(&self.code)
    }

    /// The package a PUBLISHED version froze, or `None` when `version` resolves to the
    /// unpublished one — whose package is the function's own current holding.
    ///
    /// One resolution, shared by [`Self::code_of`] and [`Self::serving_code_of`], so the two
    /// cannot disagree about which qualifiers an in-flight update can change.
    fn published_code_of(&self, version: &str) -> Option<&CodeRecord> {
        let resolved = self.resolved_version(version);
        resolved
            .filter(|resolved| resolved != VERSION_LATEST)
            .and_then(|resolved| resolved.parse::<u64>().ok())
            .and_then(|number| self.versions.get(&number))
            .map(|published| &published.code)
    }

    /// The `FunctionCodeLocation` `GetFunction` reports for the package of `version`.
    ///
    /// A published version keeps its own package and its own revision, so the location
    /// reported for it addresses THAT package and keeps serving it after the unpublished
    /// version's code is replaced.
    ///
    /// `ctx` is what the package's `Code$Location` is announced against: the member is "A
    /// presigned URL that you can use to download the deployment package", so it is
    /// presigned for the base URL this emulator is reached on (see [`code_location_url`]).
    #[must_use]
    pub fn code_location(
        &self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        version: &str,
    ) -> FunctionCodeLocation {
        let config = self.config_of(version).unwrap_or(&self.config);
        let code = self.code_of(version);
        if let Some(image_uri) = &code.image_uri {
            // A container image is a reference into the Amazon ECR registry, and
            // `ResolvedImageUri` is "The resolved URI for the image": the image the
            // function is frozen to, pinned to its content by digest rather than to the
            // mutable tag the caller named. See [`resolved_image_uri`].
            return FunctionCodeLocation {
                repository_type: Some(REPOSITORY_TYPE_ECR.to_string()),
                image_uri: Some(image_uri.clone()),
                resolved_image_uri: Some(resolved_image_uri(
                    image_uri,
                    code.resolved_image_digest.as_deref(),
                )),
                ..FunctionCodeLocation::default()
            };
        }
        FunctionCodeLocation {
            repository_type: Some(REPOSITORY_TYPE_S3.to_string()),
            location: Some(code_location_url(
                ctx,
                &self.region,
                &config.name,
                &config.revision_id,
            )),
            // "The resolved Amazon S3 object that contains the deployment package" — in
            // `REFERENCE` mode that object is the caller's, because "Lambda references
            // the deployment package from the specified Amazon S3 bucket". In the default
            // `COPY` mode Lambda "uploads a copy of your deployment package" and serves it
            // from storage of its own, so the caller's object is no longer where the
            // package is served from and none is reported — the same rule
            // [`crate::state::LayerVersionRecord::content`] applies to a layer archive.
            resolved_s3_object: code
                .s3_bucket
                .clone()
                .filter(|_| {
                    code.s3_object_storage_mode.as_deref()
                        == Some(S3_OBJECT_STORAGE_MODE_REFERENCE)
                })
                .map(|bucket| ResolvedS3Object {
                    s3_bucket: Some(bucket),
                    s3_key: code.s3_key.clone(),
                    s3_object_version: code.s3_object_version.clone(),
                }),
            source_kms_key_arn: code.source_kms_key_arn.clone(),
            ..FunctionCodeLocation::default()
        }
    }

    /// The deployment package this function holds at `revision_id`, or `None` when no
    /// identity of it is at that revision any more.
    ///
    /// A download location is reported for one revision of one identity — the unpublished
    /// configuration, or a published version, each of which has its own `RevisionId` —
    /// so the revision in the URL is what says WHICH package it addresses. A published
    /// version's package stays reachable after the unpublished one's code is replaced,
    /// which is what makes a version's location serve frozen bytes; the unpublished
    /// version's old location stops resolving, exactly as AWS stops serving a package
    /// that is no longer the function's.
    #[must_use]
    pub fn package_at_revision(&self, revision_id: &str) -> Option<&CodeRecord> {
        if self.config.revision_id == revision_id {
            return Some(&self.code);
        }
        self.versions
            .values()
            .find(|published| published.config.revision_id == revision_id)
            .map(|published| &published.code)
    }

    /// The provisioned concurrency this function has allocated across its aliases and
    /// versions, leaving out the one qualifier `exclude` names when it names one.
    ///
    /// `PutProvisionedConcurrencyConfig` REPLACES the configuration of the qualifier it
    /// addresses, so a caller checking what a new allocation would commit asks for the
    /// sum without that qualifier's current one.
    #[must_use]
    pub fn provisioned_concurrency_except(&self, exclude: Option<&str>) -> i32 {
        self.provisioned_concurrency_configs
            .iter()
            .filter(|(qualifier, _)| {
                exclude.is_none_or(|excluded| qualifier.as_str() != excluded)
            })
            .map(|(_, config)| config.requested)
            .fold(0i32, i32::saturating_add)
    }

    /// Whether an invocation addressed to `qualifiers` is served by an execution
    /// environment Lambda keeps INITIALISED: one of the qualifiers has a provisioned
    /// concurrency allocation that is ready.
    ///
    /// "AWS_LAMBDA_INITIALIZATION_TYPE – The initialization type of the execution
    /// environment … `provisioned-concurrency`", which is what a handler reads to tell a
    /// pre-warmed environment from one started for its own invocation. The status is read
    /// as stored, which the resolution of the function has already brought up to whatever
    /// the clock made due ([`crate::state::ProvisionedConcurrencyRecord::materialize`]).
    #[must_use]
    pub fn served_by_provisioned_concurrency(&self, qualifiers: &[&str]) -> bool {
        qualifiers.iter().any(|qualifier| {
            self.provisioned_concurrency_configs
                .get(*qualifier)
                .is_some_and(|config| {
                    config.status == crate::state::PROVISIONED_CONCURRENCY_READY
                })
        })
    }

    /// The `Concurrency` `GetFunction` reports, present only once reserved concurrency
    /// has been set on this function.
    #[must_use]
    pub fn concurrency(&self) -> Option<PutFunctionConcurrencyOutput> {
        self.reserved_concurrent_executions
            .map(|reserved| PutFunctionConcurrencyOutput {
                reserved_concurrent_executions: Some(reserved),
            })
    }
}
