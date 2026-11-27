// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice service.
//!
//! The whole service is one resource — the scaling plan — so this module owns the
//! stored record, the lifecycle clock that moves a plan out of its non-terminal
//! status, and the value helpers (timestamps and pagination cursors) plus the paged
//! walk itself ([`take_page`]) that more than one handler in `src/ops/` reaches.

/// The refusals this service words itself — the wire code, status and sentence of a
/// mutation that overlaps a plan's in-flight transition — and the error channel the
/// mutating operations carry them on.
pub mod wire_refusals;

/// The target tracking scaling policies a plan installs in the service that owns them.
pub mod application_auto_scaling;

/// The load metric history a predictive-scaling forecast is computed from, and the metric
/// identity a scaling instruction's load metric specification names.
pub mod load_metric_history;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::get_scaling_plan_resource_forecast_data::FORECAST_INTERVAL_SECONDS;
use crate::ops::types::{
    ApplicationSource, PredefinedScalingMetricSpecification, ScalingInstruction, ScalingPolicy,
    TagFilter, TargetTrackingConfiguration, LOAD_METRIC_TYPE_VALUES, METRIC_STATISTIC_VALUES,
    POLICY_TYPE_VALUES, PREDICTIVE_SCALING_MAX_CAPACITY_BEHAVIOR_VALUES,
    PREDICTIVE_SCALING_MODE_VALUES, SCALABLE_DIMENSION_VALUES, SCALING_METRIC_TYPE_VALUES,
    SCALING_POLICY_UPDATE_BEHAVIOR_VALUES, SERVICE_NAMESPACE_VALUES,
};

/// The only scaling-plan version this service models.
///
/// `CreateScalingPlanResponse$ScalingPlanVersion` is documented as "This value is
/// always 1. Currently, you cannot have multiple scaling plan versions."
pub const ONLY_VERSION: i64 = 1;

/// Documented `MaxResults` bounds for both Describe operations: "This value can be
/// between 1 and 50. The default value is 50."
/// (`smithy_query(query="doc",shape="DescribeScalingPlansRequest",member="MaxResults")`).
pub const MAX_RESULTS_MIN: i32 = 1;
/// Upper bound of the documented `MaxResults` range; also its documented default.
pub const MAX_RESULTS_MAX: i32 = 50;

/// How long a scaling plan stays in a non-terminal status before the clock moves it on.
///
/// CHOSEN, not documented: AWS publishes no duration for `CreationInProgress` ->
/// `Active`, so this emulator uses a fixed span inside its 250-750 ms band. A caller
/// acting straight after `CreateScalingPlan` therefore still observes the transient
/// status and the state errors that go with it.
pub const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// How long after an in-place update the plan has APPLIED the configuration that request
/// asked for.
///
/// CHOSEN, not documented: AWS publishes no duration for applying a new configuration to
/// the resources a plan covers, so this emulator owes it on the clock at a fixed span of a
/// few seconds from the request. The span is an order of magnitude longer than
/// [`SETTLE_AFTER`], the span of the plan's OWN status transition, because the two are
/// different things: the plan is back in its applied status as soon as the request it
/// accepted is in flight, while what it reports as applied is what has reached the
/// resources it covers. A caller reading the plan after an update — even one that waited
/// for the plan to be applied again — therefore still sees the configuration in force,
/// under the narrative of the application in force, and converges on what it asked for
/// only once the plan has applied it.
pub const APPLY_AFTER: Duration = Duration::from_secs(5);

/// The most scaling plans this emulator will hold at once, enforced through the
/// modelled `LimitExceededException` ("Your account exceeded a limit. This exception
/// is thrown when a per-account resource limit is exceeded.").
///
/// AWS enforces a per-account quota here but does not publish its value in the model,
/// so the NUMBER is chosen; what is modelled is that the store is bounded, so a caller
/// cannot grow it without limit.
pub const MAX_SCALING_PLANS: usize = 50;

/// `ScalingPlanStatusCode` value of a plan whose configuration is applied.
pub const STATUS_ACTIVE: &str = "Active";
/// `ScalingPlanStatusCode` value of a plan that is still being created.
pub const STATUS_CREATION_IN_PROGRESS: &str = "CreationInProgress";
/// `ScalingPlanStatusCode` value of a plan that is still being updated.
pub const STATUS_UPDATE_IN_PROGRESS: &str = "UpdateInProgress";
/// `ScalingPlanStatusCode` value of a plan that is still being deleted.
pub const STATUS_DELETION_IN_PROGRESS: &str = "DeletionInProgress";

/// `ScalingStatusCode` value of a resource whose scaling configuration is applied.
pub const SCALING_STATUS_ACTIVE: &str = "Active";
/// `ScalingStatusCode` value of a resource whose scaling configuration is not applied
/// yet — "the scaling plan is being created or the scaling configuration could not be
/// applied" (`ScalingPlanResource$ScalingStatusCode` doc).
pub const SCALING_STATUS_INACTIVE: &str = "Inactive";
/// `ScalingStatusCode` value of a resource whose configuration is only partly applied —
/// "because the scaling plan is being created or deleted" (same doc).
pub const SCALING_STATUS_PARTIALLY_ACTIVE: &str = "PartiallyActive";

// ---------------------------------------------------------------------------
// What a described plan and its resources report about status
//
// `ScalingPlan$StatusMessage` is "A simple message about the current status of the
// scaling plan" and `ScalingPlanResource$ScalingStatusMessage` "A simple message about
// the current scaling status of the resource", and both status-code docs tell a caller
// to "Check the status message for more information" — so the narrative is a fixed
// sentence per status, saying what the service did or is doing to reach it, and is
// reported on EVERY described plan and resource.
//
// One row per `ScalingPlanStatusCode` this emulator puts a plan in, so the scaling
// status a resource reports and the two narratives that explain the pair cannot
// disagree about one status, and a status added later has to bring its row.
//
// A resource's narrative also has to account for the one thing an instruction changes
// about whether target tracking scaling policies reach its resource at all
// ([`configures_dynamic_scaling`]), so each row carries the sentence for a resource
// that CARRIES such policies and the sentence for one that carries none: the narrative
// of a resource with no policy speaks of the scaling configuration the status code
// itself speaks of, never of policies that resource does not have.
// ---------------------------------------------------------------------------

/// What a plan in one `ScalingPlanStatusCode` reports, on both describe envelopes.
#[derive(Debug)]
pub struct ReportedStatus {
    /// `ScalingPlan$StatusMessage`.
    pub status_message: &'static str,
    /// The `ScalingStatusCode` every resource of such a plan reports.
    pub scaling_status_code: &'static str,
    /// `ScalingPlanResource$ScalingStatusMessage` of a resource whose instruction
    /// configures dynamic scaling, so target tracking scaling policies are what the
    /// plan applies to, updates on or removes from it.
    pub scaling_status_message: &'static str,
    /// `ScalingPlanResource$ScalingStatusMessage` of a resource whose instruction turned
    /// dynamic scaling off: no target tracking scaling policy ever reaches it, so what
    /// this status says the service did or is doing is said of the resource's scaling
    /// configuration instead — the subject the `ScalingStatusCode` doc itself uses ("The
    /// scaling configuration is active", "is not active because the scaling plan is being
    /// created").
    pub scaling_status_message_without_policies: &'static str,
}

/// What a plan whose configuration is applied to its resources reports.
const APPLIED_STATUS: ReportedStatus = ReportedStatus {
    status_message: "Scaling plan has been created and applied to all resources.",
    scaling_status_code: SCALING_STATUS_ACTIVE,
    scaling_status_message: "Target tracking scaling policies have been applied to the resource.",
    scaling_status_message_without_policies: "The scaling configuration has been applied to the \
                                              resource, which has no target tracking scaling \
                                              policies because dynamic scaling is disabled.",
};

/// What a plan reports once an in-place UPDATE of it has been applied.
///
/// `UpdateScalingPlan` "Updates the specified scaling plan" in place: neither
/// `ScalingPlanVersion` ("This value is always 1") nor `CreationTime` moves, so the
/// narrative of the applied status is what says the update took — the one observable a
/// caller polling the plan after an update can converge on. The status, the scaling
/// status and both resource narratives are the applied ones
/// ([`APPLIED_STATUS`]): what the update changed is the configuration in force, not what
/// has reached the resources.
///
/// It is reported only once the plan HAS applied what the update asked for
/// ([`ScalingPlanRecord::apply_staged`]) — the narrative explains the application in
/// force, so until the requested configuration is in force the plan still reports the
/// narrative of the application that is.
const APPLIED_AFTER_UPDATE_MESSAGE: &str = "Scaling plan has been updated successfully.";

/// What a plan reports in a status this emulator never sets.
///
/// `ScalingPlanStatusCode` lists four more values, each of which says the configuration
/// did not fully take (`ActiveWithProblems`, `CreationFailed`, `DeletionFailed`,
/// `UpdateFailed`); no path here puts a plan in one, and [`ScalingPlanRecord::reported`]
/// asserts as much in a debug build. Should one ever be set, what a resource reports is
/// `PartiallyActive` — "the scaling configuration could not be fully applied. Check the
/// status message for more information" — never "applied". It carries no status code of
/// its own: it is the row for a status [`REPORTED_STATUS`] does not list, so it is never
/// looked up BY code.
const UNAPPLIED_STATUS: ReportedStatus = ReportedStatus {
    status_message: "Scaling plan could not be fully applied to all resources.",
    scaling_status_code: SCALING_STATUS_PARTIALLY_ACTIVE,
    scaling_status_message: "Target tracking scaling policies could not be fully applied to the \
                             resource.",
    scaling_status_message_without_policies: "The scaling configuration could not be fully \
                                              applied to the resource.",
};

/// Every status this emulator puts a plan in, with what each one reports.
const REPORTED_STATUS: &[(&str, ReportedStatus)] = &[
    (STATUS_ACTIVE, APPLIED_STATUS),
    (
        STATUS_CREATION_IN_PROGRESS,
        ReportedStatus {
            status_message: "Scaling plan is being created.",
            // Nothing has reached the resource yet, which is what `Inactive` reports.
            scaling_status_code: SCALING_STATUS_INACTIVE,
            scaling_status_message: "Target tracking scaling policies have not been applied to \
                                     the resource yet because the scaling plan is being created.",
            scaling_status_message_without_policies: "The scaling configuration has not been \
                                                      applied to the resource yet because the \
                                                      scaling plan is being created.",
        },
    ),
    (
        STATUS_UPDATE_IN_PROGRESS,
        ReportedStatus {
            status_message: "Scaling plan is being updated.",
            scaling_status_code: SCALING_STATUS_PARTIALLY_ACTIVE,
            scaling_status_message: "Target tracking scaling policies are being updated on the \
                                     resource.",
            scaling_status_message_without_policies: "The scaling configuration is being updated \
                                                      on the resource.",
        },
    ),
    (
        STATUS_DELETION_IN_PROGRESS,
        ReportedStatus {
            status_message: "Scaling plan is being deleted.",
            scaling_status_code: SCALING_STATUS_PARTIALLY_ACTIVE,
            scaling_status_message: "Target tracking scaling policies are being removed from the \
                                     resource.",
            scaling_status_message_without_policies: "The scaling configuration is being removed \
                                                      from the resource.",
        },
    ),
];

/// The only `PolicyType` the model lists, and so the type of every policy a scaling
/// instruction's target tracking configuration produces.
pub const POLICY_TYPE_TARGET_TRACKING: &str = POLICY_TYPE_VALUES[0];

/// Which mutation a plan's APPLIED status is the result of.
///
/// `ScalingPlan$StatusMessage` is "A simple message about the current status of the
/// scaling plan", and a plan reaches the one applied status (`Active`) either by being
/// created or by being updated in place — the version and the creation time do not move
/// on an update, so this narrative is the only thing that tells the two apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppliedMutation {
    /// The plan was created, and its configuration applied to its resources.
    Created,
    /// The plan was updated in place, and the new configuration applied.
    Updated,
}

/// What the clock owes a plan that is in a non-terminal status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingOutcome {
    /// The plan becomes `Active`.
    Active,
    /// The plan is removed from state.
    Removed,
}

/// A transition the clock already owes at `due_at`, whether or not anybody calls.
#[derive(Debug, Clone, Copy)]
pub struct PendingTransition {
    /// The instant the transition became (or becomes) due.
    pub due_at: SystemTime,
    /// What the plan looks like once it is applied.
    pub outcome: PendingOutcome,
}

/// The configuration an in-place update ASKED a plan for, which the plan has not applied
/// yet.
///
/// `UpdateScalingPlan`'s output shape is empty, so what an update took is observable only
/// through the plan's own read-back — and what that read reports is what the plan has
/// applied. The members a request carried therefore wait here until the application the
/// clock owes comes due ([`APPLY_AFTER`], [`ScalingPlanRecord::apply_staged`]); a member
/// the request did not carry is `None` and leaves what is in force alone, exactly as the
/// request left it.
#[derive(Debug, Clone)]
pub struct StagedConfiguration {
    /// The `ApplicationSource` the update asked for, if it carried one.
    pub application_source: Option<ApplicationSource>,
    /// The `ScalingInstructions` the update asked for, if it carried them.
    pub scaling_instructions: Option<Vec<ScalingInstruction>>,
    /// The instant the application became (or becomes) due.
    pub due_at: SystemTime,
}

/// One stored scaling plan: the members the `ScalingPlan` structure reports, so
/// `DescribeScalingPlans` is a pure projection of this record. That structure carries one
/// member the published shape does not declare — the last-mutating-request instant below
/// — which is why it is written by hand in `crate::ops::types::scaling_plan_wire`.
#[derive(Debug, Clone)]
pub struct ScalingPlanRecord {
    /// `ScalingPlanName` — the primary key.
    pub name: String,
    /// `ScalingPlanVersion`; always [`ONLY_VERSION`].
    pub version: i64,
    /// `ApplicationSource` in force — the one the plan has APPLIED, stored as the caller
    /// supplied it.
    pub application_source: ApplicationSource,
    /// `ScalingInstructions` in force — the ones the plan has APPLIED to the resources it
    /// covers, stored as the caller supplied them. What a later update asks for waits in
    /// [`Self::staged`] until the plan applies it.
    pub scaling_instructions: Vec<ScalingInstruction>,
    /// `StatusCode` — a `ScalingPlanStatusCode` value.
    pub status_code: String,
    /// `StatusStartTime` — when the plan entered `status_code`.
    pub status_start_time: SystemTime,
    /// `CreationTime`.
    pub creation_time: SystemTime,
    /// When the last request that MUTATED this plan was made — the create that minted
    /// it, or the latest update or delete applied to it.
    ///
    /// Distinct from [`Self::status_start_time`], which the clock also moves when a
    /// transition it owed comes due: this instant moves only when a caller writes.
    pub last_mutating_request_time: SystemTime,
    /// Which mutation the plan's applied status reports — the create it was minted by,
    /// or the last in-place update applied to it.
    pub applied_mutation: AppliedMutation,
    /// The clock-owed transition out of a non-terminal `status_code`, if any.
    pub pending: Option<PendingTransition>,
    /// What an update asked this plan for and the plan has not applied yet, if anything.
    pub staged: Option<StagedConfiguration>,
}

impl ScalingPlanRecord {
    /// A freshly created plan: `CreationInProgress`, with the move to `Active` already
    /// owed at `now + SETTLE_AFTER`.
    pub fn creating(
        name: String,
        application_source: ApplicationSource,
        scaling_instructions: Vec<ScalingInstruction>,
        now: SystemTime,
    ) -> Self {
        Self {
            name,
            version: ONLY_VERSION,
            application_source,
            scaling_instructions,
            status_code: STATUS_CREATION_IN_PROGRESS.to_string(),
            status_start_time: now,
            creation_time: now,
            last_mutating_request_time: now,
            applied_mutation: AppliedMutation::Created,
            pending: Some(PendingTransition {
                due_at: now + SETTLE_AFTER,
                outcome: PendingOutcome::Active,
            }),
            staged: None,
        }
    }

    /// Enter a non-terminal `status`, owing `outcome` at `now + SETTLE_AFTER`.
    ///
    /// Entered only from a MUTATING request — the update and the delete paths — so this
    /// is also where the last-mutating-request stamp of an already-stored plan is taken;
    /// the create takes it in [`Self::creating`]. The clock's own work ([`
    /// ComAmazonawsAutoscalingplansAnyscalescalingplannerfrontendserviceState::settle_due`])
    /// goes nowhere near it: a transition coming due is not a request.
    pub fn begin(&mut self, status: &str, outcome: PendingOutcome, now: SystemTime) {
        if outcome == PendingOutcome::Removed {
            // A plan being withdrawn applies nothing further: a configuration an update
            // asked for and the plan never applied goes with the plan.
            self.staged = None;
        }
        self.status_code = status.to_string();
        self.status_start_time = now;
        self.last_mutating_request_time = now;
        self.pending = Some(PendingTransition {
            due_at: now + SETTLE_AFTER,
            outcome,
        });
    }

    /// Begin applying an in-place UPDATE of this plan: the members the request carried are
    /// STAGED, to be applied when the clock says the plan has applied them.
    ///
    /// A plan reports the configuration it has APPLIED to the resources it covers, so the
    /// members a request carries do not become the configuration in force at the instant
    /// the request is accepted: they wait in [`Self::staged`] until
    /// [`Self::apply_staged`] puts them in force, which this owes at `now + APPLY_AFTER`.
    /// Until then every read of the plan — its instruction set, its application source and
    /// the narrative of its status alike — reports the application in force, which is what
    /// lets a caller tell an update that has taken from one that has not.
    ///
    /// A member this request does not carry leaves what an earlier staged update asked for
    /// alone, exactly as it leaves what is in force alone: the configuration the plan is
    /// converging on is what the latest request to carry each member asked for.
    pub fn begin_update(
        &mut self,
        application_source: Option<ApplicationSource>,
        scaling_instructions: Option<Vec<ScalingInstruction>>,
        now: SystemTime,
    ) {
        let superseded = self.staged.take();
        self.staged = Some(StagedConfiguration {
            application_source: application_source.or_else(|| {
                superseded
                    .as_ref()
                    .and_then(|staged| staged.application_source.clone())
            }),
            scaling_instructions: scaling_instructions.or_else(|| {
                superseded
                    .as_ref()
                    .and_then(|staged| staged.scaling_instructions.clone())
            }),
            due_at: now + APPLY_AFTER,
        });
        self.begin(STATUS_UPDATE_IN_PROGRESS, PendingOutcome::Active, now);
    }

    /// Put the configuration a staged update asked for in force, as the clock owes at
    /// [`StagedConfiguration::due_at`].
    ///
    /// This is where an in-place update TAKES: the members the request carried replace
    /// what the plan applies, the ones it omitted are left intact, and the narrative of
    /// the applied status reports the update ([`APPLIED_AFTER_UPDATE_MESSAGE`]) rather
    /// than the create — which, since an update moves neither `ScalingPlanVersion` nor
    /// `CreationTime`, is what makes the update observable to a caller reading the plan
    /// back.
    ///
    /// `ScalingPlan$StatusMessage` is "A simple message about the current status of the
    /// scaling plan", and the applied status is reached by applying a configuration, so an
    /// update that left the configuration in force exactly as it was applied nothing and
    /// leaves the narrative of the application in force alone
    /// ([`Self::applied_footprint`]).
    pub fn apply_staged(&mut self) {
        let Some(staged) = self.staged.take() else {
            return;
        };
        let applied_before = self.applied_footprint(&self.scaling_instructions);
        let mut applied_change = false;
        if let Some(source) = staged.application_source {
            applied_change |= source != self.application_source;
            self.application_source = source;
        }
        if let Some(instructions) = staged.scaling_instructions {
            self.scaling_instructions = instructions;
        }
        applied_change |= self.applied_footprint(&self.scaling_instructions) != applied_before;
        if applied_change {
            self.applied_mutation = AppliedMutation::Updated;
        }
    }

    /// The instruction set the plan has last ASKED the resources it covers for: the one a
    /// staged update carries, or the one in force when nothing is staged.
    ///
    /// This is what the plan has installed in the service that owns its target tracking
    /// scaling policies ([`application_auto_scaling`]) — the plan asks for a policy when
    /// it accepts the request, and reports the configuration as applied once the
    /// application comes due — so it is also the set a further install diffs against, and
    /// the set a delete retires.
    pub fn requested_instructions(&self) -> &[ScalingInstruction] {
        self.staged
            .as_ref()
            .and_then(|staged| staged.scaling_instructions.as_deref())
            .unwrap_or(&self.scaling_instructions)
    }

    /// Whether this plan claims the application source whose identity is `source_key`:
    /// the source in force, or one a staged update has asked for and the plan has not
    /// applied yet.
    ///
    /// "You can create one scaling plan per application source", so a source this plan is
    /// converging on is no more claimable by another plan than the one it applies now.
    pub fn claims_source(&self, source_key: &str) -> bool {
        [
            Some(&self.application_source),
            self.staged
                .as_ref()
                .and_then(|staged| staged.application_source.as_ref()),
        ]
        .into_iter()
        .flatten()
        .any(|source| application_source_key(source).is_some_and(|key| key == source_key))
    }

    /// True while the plan is being created, updated or deleted — the state in which
    /// `UpdateScalingPlan` and `DeleteScalingPlan` answer `ConcurrentUpdateException`
    /// ("You cannot update a scaling plan if it is in the process of being created,
    /// updated, or deleted").
    pub fn is_in_progress(&self) -> bool {
        self.pending.is_some()
    }

    /// The `ScalingStatusCode` every resource of this plan reports, derived from the
    /// plan's own status: applied only once the plan itself is applied.
    pub fn scaling_status_code(&self) -> &'static str {
        self.reported().scaling_status_code
    }

    /// `StatusMessage`: the narrative of the status this plan reports.
    ///
    /// The applied status is reached by two different mutations, and the narrative says
    /// which one it was applying ([`AppliedMutation`]); every other status names the
    /// transition it is in the middle of, so one row per status answers it.
    pub fn status_message(&self) -> &'static str {
        if self.status_code == STATUS_ACTIVE && self.applied_mutation == AppliedMutation::Updated {
            return APPLIED_AFTER_UPDATE_MESSAGE;
        }
        self.reported().status_message
    }

    /// `ScalingStatusMessage`: the narrative of the scaling status the resource that
    /// `instruction` applies to reports, derived — like the status code itself — from
    /// what the plan is doing to that resource, and from whether the instruction in
    /// force puts target tracking scaling policies on it at all.
    ///
    /// The narrative and [`Self::applied_policies`] read ONE predicate about that,
    /// [`configures_dynamic_scaling`], so a resource whose instruction turned dynamic
    /// scaling off cannot be told that policies have been applied to, are being updated
    /// on, or are being removed from it while the very same read reports it carrying
    /// none; what such a resource reports is what the plan did to its scaling
    /// configuration.
    pub fn scaling_status_message(&self, instruction: &ScalingInstruction) -> &'static str {
        let reported = self.reported();
        if configures_dynamic_scaling(instruction) {
            reported.scaling_status_message
        } else {
            reported.scaling_status_message_without_policies
        }
    }

    /// The row of [`REPORTED_STATUS`] for the status this plan is in.
    fn reported(&self) -> &'static ReportedStatus {
        let reported = REPORTED_STATUS
            .iter()
            .find(|(status_code, _)| *status_code == self.status_code)
            .map(|(_, reported)| reported);
        debug_assert!(
            reported.is_some(),
            "status {:?} needs its own REPORTED_STATUS row",
            self.status_code
        );
        reported.unwrap_or(&UNAPPLIED_STATUS)
    }

    /// The scaling policies AWS Auto Scaling has applied to the resource `instruction`
    /// names, in the order the instruction lists their configurations.
    ///
    /// `ScalingInstruction` documents that "AWS Auto Scaling creates target tracking
    /// scaling policies based on the scaling instructions", and `DisableDynamicScaling`
    /// that "When dynamic scaling is enabled, AWS Auto Scaling creates target tracking
    /// scaling policies based on the specified target tracking configurations" — so an
    /// applied resource carries one `TargetTrackingScaling` policy per target tracking
    /// configuration of its instruction, echoing that configuration, and an instruction
    /// that turned dynamic scaling off carries none.
    ///
    /// Empty while the plan is still being created: no policy has reached the resource
    /// yet, which is exactly what its `Inactive` scaling status reports.
    ///
    /// [`Self::scaling_status_message`] reads the same predicate about the instruction,
    /// so the narrative reported beside this list never speaks of policies this list
    /// does not hold.
    ///
    /// A policy is named after the IDENTITIES its state gives it — the resource
    /// `instruction` applies to and the metric each configuration tracks — so a rewrite
    /// of [`Self::scaling_instructions`] that leaves a resource's instruction alone
    /// leaves the names that resource reports alone too. See [`policy_name`].
    ///
    /// The policies are PROJECTED per read rather than stored beside the instruction
    /// they come from, so one record cannot disagree with another about what a resource
    /// tracks; the configuration clone that costs is bounded by the documented
    /// [`MAX_TARGET_TRACKING_CONFIGURATIONS`] per resource, and the page by `MaxResults`.
    pub fn applied_policies(&self, instruction: &ScalingInstruction) -> Vec<ScalingPolicy> {
        if self.scaling_status_code() == SCALING_STATUS_INACTIVE
            || !configures_dynamic_scaling(instruction)
        {
            return Vec::new();
        }
        instruction
            .target_tracking_configurations
            .iter()
            .enumerate()
            .map(|(position, configuration)| ScalingPolicy {
                policy_name: derived_policy_name(
                    &self.name,
                    instruction,
                    configuration,
                    position,
                ),
                policy_type: POLICY_TYPE_TARGET_TRACKING.to_string(),
                target_tracking_configuration: Some(configuration.clone()),
            })
            .collect()
    }

    /// What this plan has APPLIED to the resources it covers, under the instruction set
    /// `instructions`: per resource, the capacity band the plan registered for it and the
    /// target tracking scaling policies it put on it.
    ///
    /// This is what an update is compared against ([`Self::apply_staged`]): the narrative
    /// of the applied status reports what the plan last applied to its resources, so an
    /// update that leaves this footprint exactly as it was is not reported as having
    /// updated the plan, however the request spelled the instructions it re-sent.
    ///
    /// Keyed by resource identity rather than by list position, so re-sending the same
    /// instructions in another order is the same footprint.
    pub fn applied_footprint(
        &self,
        instructions: &[ScalingInstruction],
    ) -> BTreeMap<String, (i32, i32, Vec<ScalingPolicy>)> {
        instructions
            .iter()
            .map(|instruction| {
                (
                    resource_identity_key(instruction),
                    (
                        instruction.min_capacity,
                        instruction.max_capacity,
                        self.applied_policies(instruction),
                    ),
                )
            })
            .collect()
    }
}

/// Which refusal the mutating-plan admission rule answers with.
///
/// The rule decides WHICH refusal applies; the operation that asked answers it on the
/// wire — `ObjectNotFoundException`, in the operation's own modelled shape, carrying the
/// text the rule worded it with for [`Self::NotFound`], and the service's own
/// overlapping-mutation refusal ([`wire_refusals::WireRefusal::concurrent_update`]) for
/// [`Self::InProgress`], on both write paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanMutationRefusal {
    /// Nothing is stored under the requested name and version: [`plan_not_found_message`].
    NotFound(String),
    /// The plan has a lifecycle transition in flight. The refusal carries no text of its
    /// own: its sentence is fixed ([`wire_refusals::CONCURRENT_UPDATE_MESSAGE`]) and
    /// interpolates nothing about the plan or the request.
    InProgress,
}

/// The name the plan `plan_name` derives for the policy one target tracking
/// `configuration` of `instruction` is installed under.
///
/// The ONE answer this crate gives that question: the policies a resource REPORTS
/// ([`ScalingPlanRecord::applied_policies`]) and the policies the plan installs in the
/// service that owns them ([`application_auto_scaling`]) are named here, so a caller that
/// reads a policy name off a plan's resource finds that policy where it lives.
///
/// `position` only keeps the names of one resource apart for a configuration that
/// identifies no metric at all — one naming neither a predefined nor a customized metric,
/// which both write paths refuse before it can be stored.
pub fn derived_policy_name(
    plan_name: &str,
    instruction: &ScalingInstruction,
    configuration: &TargetTrackingConfiguration,
    position: usize,
) -> String {
    policy_name(
        plan_name,
        &resource_identity_key(instruction),
        &target_metric_identity(configuration).unwrap_or_else(|| position.to_string()),
    )
}

/// The literal prefix of the name AWS Auto Scaling gives a policy it mints for a
/// scaling plan.
const POLICY_NAME_PREFIX: &str = "AutoScaling-";

/// The name AWS Auto Scaling gives the policy it mints from one target tracking
/// configuration.
///
/// The service generates the name, out of the plan it belongs to: it is
/// [`POLICY_NAME_PREFIX`], then the `ScalingPlanName` verbatim, then `-` and a UUID the
/// service mints for that policy. The plan name is what joins a policy back to the
/// plan that installed it — `DescribeScalingPolicies`, `PutScalingPolicy` and the
/// CloudWatch alarms of a target tracking policy are all keyed on the policy name — so a
/// client can select a plan's policies by the derivation alone, and the whole value is
/// one flat token: no `/`, which is also why a `ScalingPlanName` may carry none.
/// `ScalingPolicy$PolicyName` only constrains the result: a printable string of 1-256
/// characters.
///
/// The UUID is minted from the state that identifies the policy inside its plan — the
/// resource the policy was applied to and the configuration within that resource's
/// instruction it came from ([`policy_uuid`]) — so no two policies of one plan share a
/// name, and the name a resource reports does not change between reads or when an update
/// rewrites the instruction list around it.
///
/// `resource` is the [`resource_identity_key`] of the instruction the policy was minted
/// from and `configuration` the [`target_metric_identity`] of the configuration inside
/// it — the two IDENTITIES, never the positions those values happen to hold in a list,
/// because an update rewrites the instruction list in the order the request carries it
/// and so moves every position in it.
fn policy_name(plan_name: &str, resource: &str, configuration: &str) -> String {
    format!(
        "{POLICY_NAME_PREFIX}{plan_name}-{}",
        policy_uuid(resource, configuration)
    )
}

/// How many BYTES of an identity's SHA-256 digest the policy's UUID is laid out from.
///
/// A UUID is 16 bytes, rendered as 36 characters, so a `ScalingPlanName` at its modelled
/// maximum of 128 characters, plus the fixed prefix and separator, occupies 177 of the
/// 256 characters `ScalingPolicy$PolicyName` allows, whatever the identities are spelled
/// with; 122 bits of digest reach the UUID, which is why two identities of one plan do
/// not render one name.
const UUID_BYTES: usize = 16;

/// The UUID the plan mints for the policy of one identity pair.
///
/// Both identities are built out of caller-supplied text the model lets run long — a
/// `ResourceId` alone reaches 1600 characters and a customized metric's namespace, name
/// and dimensions are plain strings — so they reach the fixed-width UUID as the leading
/// [`UUID_BYTES`] of their SHA-256 digest: the name stays a function of the identities,
/// and of nothing else, at a length the plan name bounds. Equal pairs render equal,
/// different pairs render different, and the rendering is the same on every read because
/// it reads nothing but the pair. The resource's own length goes into the digest ahead of
/// the two keys, so a character moved from the end of one key to the start of the other
/// is a different pair.
///
/// The digest bytes are laid out as a UUID rather than printed as a digest, because what
/// the policy carries is a UUID. The layout itself is the family's
/// (`service_common::uuid_from_random_bytes`), which renders 16 opaque bytes a service
/// minted in the hyphenated form with the version and variant bits a UUID carries.
fn policy_uuid(resource: &str, configuration: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update((resource.len() as u64).to_be_bytes());
    hasher.update(resource.as_bytes());
    hasher.update(configuration.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; UUID_BYTES];
    bytes.copy_from_slice(&digest[..UUID_BYTES]);
    ::service_common::uuid_from_random_bytes(bytes)
}

/// Per-service state for com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAutoscalingplansAnyscalescalingplannerfrontendserviceState {
    /// Every stored plan, keyed by `ScalingPlanName`. A `BTreeMap` so both Describe
    /// operations page over a stable, deterministic ordering.
    pub scaling_plans: BTreeMap<String, ScalingPlanRecord>,
}

impl ComAmazonawsAutoscalingplansAnyscalescalingplannerfrontendserviceState {
    /// Apply every transition the clock already owes at `now`.
    ///
    /// This is the ONLY write a reporting operation makes: the work was due at `now`
    /// whether or not anybody called, so running it here cannot make a read change
    /// what a later caller sees.
    ///
    /// A plan's own status transition is settled before the configuration a staged update
    /// asked for is put in force ([`ScalingPlanRecord::apply_staged`]): the application
    /// reaches the resources the plan covers after the plan is back in its applied status,
    /// so the two come due in that order and are settled in it.
    pub fn settle_due(&mut self, now: SystemTime) {
        let due: Vec<(String, PendingOutcome, SystemTime)> = self
            .scaling_plans
            .values()
            .filter_map(|plan| {
                let pending = plan.pending?;
                (pending.due_at <= now)
                    .then(|| (plan.name.clone(), pending.outcome, pending.due_at))
            })
            .collect();
        for (name, outcome, due_at) in due {
            match outcome {
                PendingOutcome::Removed => {
                    self.scaling_plans.remove(&name);
                }
                PendingOutcome::Active => {
                    if let Some(plan) = self.scaling_plans.get_mut(&name) {
                        plan.status_code = STATUS_ACTIVE.to_string();
                        // The plan entered `Active` when the transition came due, not
                        // when somebody happened to look.
                        plan.status_start_time = due_at;
                        plan.pending = None;
                    }
                }
            }
        }
        let applying: Vec<String> = self
            .scaling_plans
            .values()
            .filter(|plan| {
                plan.staged
                    .as_ref()
                    .is_some_and(|staged| staged.due_at <= now)
            })
            .map(|plan| plan.name.clone())
            .collect();
        for name in applying {
            if let Some(plan) = self.scaling_plans.get_mut(&name) {
                plan.apply_staged();
            }
        }
    }

    /// The plan stored under `name` whose version is `version`.
    pub fn plan(&self, name: &str, version: i64) -> Option<&ScalingPlanRecord> {
        self.scaling_plans
            .get(name)
            .filter(|plan| plan.version == version)
    }

    /// Mutable access to the plan stored under `name` whose version is `version`.
    pub fn plan_mut(&mut self, name: &str, version: i64) -> Option<&mut ScalingPlanRecord> {
        self.scaling_plans
            .get_mut(name)
            .filter(|plan| plan.version == version)
    }

    /// Admit a request that is about to WRITE the plan it names, or answer the refusal
    /// that stands in its way.
    ///
    /// One rule, in one place, for every mutating operation that identifies a plan by
    /// `ScalingPlanName` + `ScalingPlanVersion`: the plan is resolved first, so a name
    /// and version that name no stored plan are refused as not found
    /// ([`plan_not_found_message`]); a plan that is resolved but still has a lifecycle
    /// transition in flight ([`ScalingPlanRecord::is_in_progress`]) is refused as a
    /// concurrent update. Only a plan past both is handed back, mutably, for the caller
    /// to write.
    ///
    /// The refusal order is part of the rule: a request is answered about whether the
    /// plan it addresses exists before it is answered about what that plan is doing, and
    /// before any rule about OTHER plans.
    pub fn admit_plan_mutation(
        &mut self,
        name: &str,
        version: i64,
    ) -> Result<&mut ScalingPlanRecord, PlanMutationRefusal> {
        let plan = self
            .plan_mut(name, version)
            .ok_or_else(|| PlanMutationRefusal::NotFound(plan_not_found_message(name, version)))?;
        if plan.is_in_progress() {
            return Err(PlanMutationRefusal::InProgress);
        }
        Ok(plan)
    }

    /// The stored plan — other than `excluding` — that already claims the application
    /// source whose identity is `source_key`.
    ///
    /// `ApplicationSource` is documented as "You can create one scaling plan per
    /// application source", so EVERY path that sets a plan's application source refuses
    /// a source this answers `Some` for. `excluding` names the plan the call itself
    /// writes, which of course may keep claiming the source it already claims.
    ///
    /// A plan whose removal is only OWED still counts: it is still stored, and both
    /// Describe operations still report it, so its source is claimable again only once
    /// the clock has actually removed it (callers run [`Self::settle_due`] first). So does
    /// a source a staged update has asked for and the plan has not applied yet
    /// ([`ScalingPlanRecord::claims_source`]): the plan is converging on it.
    pub fn plan_covering_source(
        &self,
        source_key: &str,
        excluding: Option<&str>,
    ) -> Option<&ScalingPlanRecord> {
        self.scaling_plans.values().find(|plan| {
            excluding != Some(plan.name.as_str()) && plan.claims_source(source_key)
        })
    }
}

/// A modelled `TimestampType` from a stored wall-clock instant.
///
/// Clamped at the epoch rather than panicking: a host clock before 1970 is not a
/// client error.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

// ---------------------------------------------------------------------------
// Identity keys built out of caller-supplied text
//
// Two rules of this service are decided on the identity of a composite value — the
// application a scaling plan covers, and the metric a target tracking configuration
// tracks — and each identity is rendered as one flat string, so it can be compared and
// held in a set. The parts those strings are built from are the caller's own text,
// whose modelled constraints admit every punctuation character a separator could be, so
// a part reaches a key only through [`escape_key_part`]: inside a part, a separator of
// the composite is spelled as a literal, which is what keeps two composites that differ
// in their parts apart in their keys.
// ---------------------------------------------------------------------------

/// The characters an identity key in this module uses to separate the parts a
/// composite value is built from.
const KEY_SEPARATORS: [char; 6] = ['=', ',', ';', '/', '[', ']'];

/// The character that takes the separator meaning away from the one after it.
const KEY_ESCAPE: char = '\\';

/// One part of an identity key, with every [`KEY_SEPARATORS`] character it contains —
/// and every [`KEY_ESCAPE`] — prefixed by [`KEY_ESCAPE`].
///
/// So no separator a composite key relies on occurs unescaped inside a part, the parts
/// of a key can always be told apart again, and a part that holds no separator is
/// written out as it came in.
fn escape_key_part(part: &str) -> String {
    let mut escaped = String::with_capacity(part.len());
    for character in part.chars() {
        if character == KEY_ESCAPE || KEY_SEPARATORS.contains(&character) {
            escaped.push(KEY_ESCAPE);
        }
        escaped.push(character);
    }
    escaped
}

/// The parts of an identity key that stands for a SET of values: ordered, and with
/// repeated entries collapsed to one.
///
/// A set is the entries it holds, not how many times each is written, so neither the
/// ORDER nor the MULTIPLICITY of the entries may reach the rendered key: a request that
/// repeats an entry names the same set as one that states it once, and so has to render
/// the same identity. The repeats are collapsed after the ordering, which is what puts
/// equal entries next to each other.
fn set_key_parts<Parts: IntoIterator<Item = String>>(parts: Parts) -> Vec<String> {
    let mut parts: Vec<String> = parts.into_iter().collect();
    parts.sort();
    parts.dedup();
    parts
}

/// The identity of an `ApplicationSource`, used to enforce "You can create one
/// scaling plan per application source" (`ApplicationSource` member doc).
///
/// A tag-filter source is identified by the SET of tags it names — `TagFilters` is "A
/// set of tags (up to 50)" and each filter's `Values` are the tag's values — so the
/// filters and each filter's values reach the key through [`set_key_parts`]: two
/// spellings of one set, differing in the order or in a repeated entry, are one source.
/// Each part is escaped, so a tag key or tag value that itself contains a separator
/// stays distinguishable from the separator (`TagFilter$Key` and `TagValues`' member
/// admit every character in `\u0020-\uD7FF…`, punctuation included).
///
/// `None` when the source names neither a stack nor a tag, so it identifies no
/// application at all — which a tag-filter list does whenever no filter in it names a
/// tag ([`named_tag_key`]). A source that names BOTH is refused before this is reached —
/// see [`validate_application_source`].
pub fn application_source_key(source: &ApplicationSource) -> Option<String> {
    if let Some(arn) = named_stack_arn(source) {
        // A stack is named by ONE part, and no tag-filter key starts with this prefix,
        // so there is no separator to confuse here.
        return Some(format!("stack:{arn}"));
    }
    let filters = source.tag_filters.as_ref()?;
    let rendered = set_key_parts(filters.iter().filter_map(|filter| {
        // A filter that names no tag names no part of the set, so it contributes nothing
        // to the identity — never an empty key-part, which would make a source that
        // merely carries such a filter identify another application.
        let key = escape_key_part(named_tag_key(filter)?);
        let values = set_key_parts(
            filter
                .values
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(String::as_str)
                .map(escape_key_part),
        );
        Some(format!("{key}={}", values.join(",")))
    }));
    if rendered.is_empty() {
        return None;
    }
    Some(format!("tags:{}", rendered.join(";")))
}

/// Whether a tag-filter list identifies anything: a list that is absent, empty, or holds
/// no filter that names a tag selects nothing.
fn names_tag_filters(source: &ApplicationSource) -> bool {
    source
        .tag_filters
        .as_ref()
        .is_some_and(|filters| filters.iter().any(|f| named_tag_key(f).is_some()))
}

/// The tag key a `TagFilter` names, or `None` when it names no tag.
///
/// A `TagFilter` "Represents a tag" and its `Key` is "The tag key", modelled
/// `length {min: 1, max: 128}`, so a key that is absent — or present but empty — names no
/// tag at all. This is the single place that decides what "names a tag" means, so the
/// identity key ([`application_source_key`]), the exclusivity rule that key feeds and the
/// per-filter check in [`validate_application_source`] cannot disagree about one filter —
/// the same way [`named_stack_arn`] decides it for the other side of the alternative.
fn named_tag_key(filter: &TagFilter) -> Option<&str> {
    filter.key.as_deref().filter(|key| !key.is_empty())
}

/// The CloudFormation stack ARN an `ApplicationSource` names, or `None` when it names
/// no stack.
///
/// A `CloudFormationStackARN` that is absent, or present but empty, names nothing. This
/// is the single place that decides what "names a stack" means, so the identity key
/// ([`application_source_key`]), the exclusivity rule that key feeds and the ARN-format
/// check in [`validate_application_source`] cannot disagree about one source.
fn named_stack_arn(source: &ApplicationSource) -> Option<&str> {
    source
        .cloud_formation_stack_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())
}

/// Whether `arn` is the Amazon Resource Name of a CloudFormation stack.
///
/// `ApplicationSource$CloudFormationStackARN` is documented as "The Amazon Resource
/// Name (ARN) of a AWS CloudFormation stack", and an ARN is
/// `arn:<partition>:<service>:<region>:<account>:<resource>` — so a stack ARN names the
/// `cloudformation` service and a `stack/…` resource.
fn is_cloud_formation_stack_arn(arn: &str) -> bool {
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some("cloudformation"),
        Some(region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return false;
    };
    !partition.is_empty()
        && !region.is_empty()
        && !account.is_empty()
        && resource
            .strip_prefix("stack/")
            .is_some_and(|name| !name.is_empty())
}

/// Documented maximum entry count of `ApplicationSource$TagFilters`: "A set of tags
/// (up to 50)."
pub const MAX_TAG_FILTERS: usize = 50;

/// What `ApplicationSource$TagFilters` counts, as its own documentation names them ("A
/// set of tags"), for the refusal that states its bound.
pub const TAG_FILTER_ENTRIES: &str = "tag filters";

/// Documented maximum entry count of `TagFilter$Values`: "The tag values (0 to 20)."
pub const MAX_TAG_VALUES: usize = 20;

/// What `TagFilter$Values` counts, as its own documentation names them ("The tag
/// values"), for the refusal that states its bound.
pub const TAG_VALUE_ENTRIES: &str = "tag values";

/// The refusal for a request list member that holds more entries than its own
/// documentation allows.
///
/// A list member whose documentation states a maximum entry count is bounded by it even
/// when the list shape carries no modelled `@length`, so nothing before the handler
/// applies it: the bound is the service's to apply and the refusal its own sentence,
/// which states the maximum and what the member counts and interpolates nothing else
/// from the request. This is the single place the whole crate words it. `entries` names
/// what the member counts, as that member's own documentation names them, and `max` is
/// the count it states. Separate from [`check_entry_count`] so a caller that reaches the
/// bound from inside a nested walk renders it only once the bound is actually broken.
pub fn entry_count_message(entries: &str, max: usize) -> String {
    format!("At most {max} {entries} are allowed")
}

/// The refusal for a tag filter that names no tag, worded and pathed the way the
/// framework words a member that is not there — so a filter whose `Key` is ABSENT is
/// reported exactly like one whose `Key` is present and empty, which the modelled
/// `length {min: 1, max: 128}` on `TagFilter$Key` has the framework refuse before a
/// handler runs.
///
/// `position` is the filter's 1-based position, because that is how a list element names
/// itself in a validation path (`::aws_smithy_schema_server::constraints::path`, the one
/// place a path segment is spelled).
fn keyless_tag_filter_message(position: usize) -> String {
    use ::aws_smithy_schema_server::constraints::path;

    ::aws_smithy_schema_server::constraints::missing_member_message(&path::join(&[
        path::member("ApplicationSource"),
        path::member("TagFilters"),
        path::element(position),
        path::member("Key"),
    ]))
}

/// Refuse a request list member that holds more entries than its own documentation
/// allows; a list within its bound is accepted. `entries_named` is what the member
/// counts, as [`entry_count_message`] states it.
pub fn check_entry_count(entries_named: &str, entries: usize, max: usize) -> Result<(), String> {
    if entries > max {
        return Err(entry_count_message(entries_named, max));
    }
    Ok(())
}

/// Validate an `ApplicationSource` a caller wrote, returning its identity key.
///
/// `ApplicationSource` is documented as "A CloudFormation stack or a set of tags" —
/// the two are ALTERNATIVES, so naming neither identifies no application and naming
/// both is contradictory, and a stack is named by its ARN. `Err` carries the refusal
/// message naming the member at fault.
///
/// The tag-filter side carries two documented entry counts of its own — on the filter
/// list and on each filter's value list — and a list past its bound is refused for its
/// LENGTH before any member of it is read, so those come first. Each entry of that list
/// then has to be a tag, which is what its key names, so a filter naming none is refused
/// for that member rather than entering the set of tags the source is identified by —
/// before the either/or check below, because a member of the request is answered on its
/// own terms before what two members mean TOGETHER is decided (a source that writes both
/// a stack ARN and a filter naming no tag is therefore reported for that filter).
pub fn validate_application_source(source: &ApplicationSource) -> Result<String, String> {
    if let Some(filters) = source.tag_filters.as_ref() {
        check_entry_count(TAG_FILTER_ENTRIES, filters.len(), MAX_TAG_FILTERS)?;
        if filters
            .iter()
            .any(|filter| filter.values.as_ref().map_or(0, Vec::len) > MAX_TAG_VALUES)
        {
            return Err(entry_count_message(TAG_VALUE_ENTRIES, MAX_TAG_VALUES));
        }
        // `TagFilters` is "A set of tags", and a tag is named by its key, so an entry
        // carrying none names no tag and is reported for the member it leaves out — at
        // the position of the first such entry.
        if let Some(index) = filters
            .iter()
            .position(|filter| named_tag_key(filter).is_none())
        {
            return Err(keyless_tag_filter_message(index + 1));
        }
    }
    let stack_arn = named_stack_arn(source);
    if let Some(arn) = stack_arn {
        if !is_cloud_formation_stack_arn(arn) {
            return Err(format!("Invalid CloudFormation stack ARN: {arn}"));
        }
    }
    match (stack_arn.is_some(), names_tag_filters(source)) {
        (true, true) => Err(AMBIGUOUS_APPLICATION_SOURCE.to_string()),
        (false, false) => Err(EMPTY_APPLICATION_SOURCE.to_string()),
        _ => application_source_key(source).ok_or_else(|| EMPTY_APPLICATION_SOURCE.to_string()),
    }
}

/// The refusal for an `ApplicationSource` that names BOTH alternatives.
///
/// "A CloudFormation stack or set of tags" — a source naming both identifies no one
/// application, and the refusal says exactly that: the whole sentence is fixed text and
/// interpolates nothing from the request. `pub` so every path that takes an application
/// source words it once.
pub const AMBIGUOUS_APPLICATION_SOURCE: &str = "Ambiguous application source";

/// The refusal for an `ApplicationSource` that names NEITHER alternative.
///
/// The counterpart of [`AMBIGUOUS_APPLICATION_SOURCE`]: a source that carries neither a
/// stack ARN nor a tag that identifies one is empty, whichever of the two members it
/// wrote and left blank. Fixed text, interpolating nothing from the request.
pub const EMPTY_APPLICATION_SOURCE: &str = "Empty application source";

// ---------------------------------------------------------------------------
// ScalingInstruction semantics
//
// The model constrains each member's TYPE, but a scaling instruction also has to hold
// together across its members, and `CreateScalingPlan` / `UpdateScalingPlan` both
// model `ValidationException` ("An exception was thrown for a validation issue. Review
// the parameters provided.") for input that does not. Every constant and relation
// below comes from the model, from its documentation, or from the refusal the service
// itself answers with — never from the values of one particular request.
// ---------------------------------------------------------------------------

/// Documented bound on `ScalingInstruction$TargetTrackingConfigurations`: "The target
/// tracking configurations (up to 10)."
pub const MAX_TARGET_TRACKING_CONFIGURATIONS: usize = 10;

/// What `ScalingInstruction$TargetTrackingConfigurations` counts, as its own
/// documentation names them ("The target tracking configurations"), for the refusal that
/// states its bound.
pub const TARGET_TRACKING_ENTRIES: &str = "target tracking configurations";

// `ScalingInstruction$MinCapacity` and `$MaxCapacity` both target `ResourceCapacity`,
// declared `{"type": "integer"}` with no `@range`
// (`smithy_query(query="shape",shape="ResourceCapacity")`), so the MODEL bounds neither
// member and the floor is the service's own: a scaling instruction registers a band a
// scalable resource is kept inside, and a capacity counts units of that resource, so no end
// of the band can be NEGATIVE — there is no such thing as minus one task, instance or
// provisioned unit. A minimum below that floor is refused in the service's own prose
// ([`MINIMUM_CAPACITY_BELOW_FLOOR`]). The other relation the pair has to satisfy is the
// cross-member one, stated beside it in [`validate_scaling_instruction`].

/// The least capacity a scaling instruction may set as the floor of the band it registers.
///
/// ZERO is a capacity like any other: the ordinary configuration of a resource that scales to
/// nothing (an Auto Scaling group whose `MinSize` is 0), and necessarily so for a plan whose
/// `PredictiveScalingMode` only forecasts, which scales nothing at all — so a zero minimum is
/// ADMITTED and only a negative one is refused.
pub const MIN_RESOURCE_CAPACITY: i32 = 0;

/// The sentence a `MinCapacity` below [`MIN_RESOURCE_CAPACITY`] is refused with, naming the
/// member in prose the way this service words a capacity relation.
///
/// Entirely FIXED text: no part of it is interpolated from the request or from state, and the
/// literal 1 in it is the service's own wording of the refusal rather than the floor the
/// refusal is drawn at — a minimum of 0 is accepted and answers nothing at all.
pub const MINIMUM_CAPACITY_BELOW_FLOOR: &str = "Minimum capacity cannot be less than 1";

/// The sentence an inverted capacity band is refused with, worded the same way.
pub const MAXIMUM_CAPACITY_BELOW_MINIMUM: &str =
    "Maximum capacity cannot be less than minimum capacity";

/// Lower bound of `ScalingInstruction$PredictiveScalingMaxCapacityBuffer`: "The range
/// is 1-100."
pub const MAX_CAPACITY_BUFFER_MIN: i32 = 1;
/// Upper bound of the same documented range.
pub const MAX_CAPACITY_BUFFER_MAX: i32 = 100;

/// The `PredictiveScalingMaxCapacityBehavior` value that makes
/// `PredictiveScalingMaxCapacityBuffer` required: "Required if the
/// PredictiveScalingMaxCapacityBehavior is set to SetMaxCapacityAboveForecastCapacity,
/// and cannot be used otherwise."
pub const BEHAVIOR_ABOVE_FORECAST_CAPACITY: &str = "SetMaxCapacityAboveForecastCapacity";

/// The only `ScalingMetricType` a `PredefinedScalingMetricSpecification$ResourceLabel`
/// may accompany: "You can't specify a resource label unless the metric type is
/// ALBRequestCountPerTarget".
pub const ALB_SCALING_METRIC_TYPE: &str = "ALBRequestCountPerTarget";

/// The same rule for `PredefinedLoadMetricSpecification$ResourceLabel`: "You can't
/// specify a resource label unless the metric type is ALBTargetGroupRequestCount".
pub const ALB_LOAD_METRIC_TYPE: &str = "ALBTargetGroupRequestCount";

/// The format BOTH `ResourceLabel` members document for the value they carry, in their
/// own words: "You create the resource label by appending the final portion of the load
/// balancer ARN and the final portion of the target group ARN into a single value,
/// separated by a forward slash (/). The format is
/// app/<load-balancer-name>/<load-balancer-id>/targetgroup/<target-group-name>/<target-group-id>"
/// (`smithy_query(query="doc",shape="PredefinedScalingMetricSpecification",member="ResourceLabel")`
/// and the identical paragraph on `PredefinedLoadMetricSpecification$ResourceLabel`).
///
/// `pub` so the refusal and any caller that has to name the format spell it once.
pub const RESOURCE_LABEL_FORMAT: &str =
    "app/<load-balancer-name>/<load-balancer-id>/targetgroup/<target-group-name>/<target-group-id>";

/// The marker the load balancer's portion of a resource label opens with — the first
/// segment of an Application Load Balancer's ARN resource path, hence of its "final
/// portion", as the format above spells it.
const LOAD_BALANCER_MARKER: &str = "app";

/// The marker the target group's portion opens with, from the same format.
const TARGET_GROUP_MARKER: &str = "targetgroup";

/// `CustomizedLoadMetricSpecification$Statistic`: "The only valid value is Sum."
pub const LOAD_METRIC_STATISTIC: &str = "Sum";

/// The documented bound on `TargetTrackingConfiguration$TargetValue`: "Although this
/// property accepts numbers of type Double, it won't accept values that are either too
/// small or too large." The service states the two bounds itself, in the refusal it
/// answers with: "For target tracking scaling, target value must be between
/// '8.51592E-109' and '1.174271E108'".
pub const TARGET_VALUE_MIN: f64 = 8.515_92e-109;
/// Upper bound of that same range.
pub const TARGET_VALUE_MAX: f64 = 1.174_271e108;

/// The two bounds as the service renders them in its refusal text.
const TARGET_VALUE_MIN_TEXT: &str = "8.51592E-109";
const TARGET_VALUE_MAX_TEXT: &str = "1.174271E108";

/// What each `ScalableDimension` scales, and what can be tracked for it.
///
/// Columns: the scalable dimension; where the resource id of a resource it applies to
/// starts, taken from the worked examples in `ScalingInstruction$ResourceId`'s own
/// documentation (`smithy_query(query="doc",shape="ScalingInstruction",member="ResourceId")`)
/// — `autoScalingGroup/my-asg`, `service/default/sample-webapp`,
/// `spot-fleet-request/sfr-73fbd2ce-…`, `table/my-table`,
/// `table/my-table/index/my-table-index`, `cluster:my-db-cluster`; and the
/// `ScalingMetricType`s that measure that resource, each named for what it measures
/// (`ASG…` an Auto Scaling group, `ECSService…` an ECS service, `DynamoDBRead…` /
/// `DynamoDBWrite…` the matching capacity of a table or index, `RDSReader…` an Aurora
/// replica, `EC2SpotFleetRequest…` a Spot Fleet request), with
/// `ALBRequestCountPerTarget` documented as applying "only to Auto Scaling groups, Spot
/// Fleet requests, and ECS services"
/// (`smithy_query(query="doc",shape="PredefinedScalingMetricSpecification",member="PredefinedScalingMetricType")`).
const SCALABLE_DIMENSIONS: &[(&str, &str, &[&str])] = &[
    (
        "autoscaling:autoScalingGroup:DesiredCapacity",
        "autoScalingGroup/",
        &[
            "ASGAverageCPUUtilization",
            "ASGAverageNetworkIn",
            "ASGAverageNetworkOut",
            ALB_SCALING_METRIC_TYPE,
        ],
    ),
    (
        "ecs:service:DesiredCount",
        "service/",
        &[
            "ECSServiceAverageCPUUtilization",
            "ECSServiceAverageMemoryUtilization",
            ALB_SCALING_METRIC_TYPE,
        ],
    ),
    (
        "ec2:spot-fleet-request:TargetCapacity",
        "spot-fleet-request/",
        &[
            "EC2SpotFleetRequestAverageCPUUtilization",
            "EC2SpotFleetRequestAverageNetworkIn",
            "EC2SpotFleetRequestAverageNetworkOut",
            ALB_SCALING_METRIC_TYPE,
        ],
    ),
    (
        "rds:cluster:ReadReplicaCount",
        "cluster:",
        &[
            "RDSReaderAverageCPUUtilization",
            "RDSReaderAverageDatabaseConnections",
        ],
    ),
    (
        "dynamodb:table:ReadCapacityUnits",
        "table/",
        &["DynamoDBReadCapacityUtilization"],
    ),
    (
        "dynamodb:table:WriteCapacityUnits",
        "table/",
        &["DynamoDBWriteCapacityUtilization"],
    ),
    (
        "dynamodb:index:ReadCapacityUnits",
        "table/",
        &["DynamoDBReadCapacityUtilization"],
    ),
    (
        "dynamodb:index:WriteCapacityUnits",
        "table/",
        &["DynamoDBWriteCapacityUtilization"],
    ),
];

/// The row of [`SCALABLE_DIMENSIONS`] for `dimension`, if the model lists it.
fn scalable_dimension(dimension: &str) -> Option<(&'static str, &'static [&'static str])> {
    SCALABLE_DIMENSIONS
        .iter()
        .find(|(name, _, _)| *name == dimension)
        .map(|(_, prefix, metrics)| (*prefix, *metrics))
}

/// How the service refuses a `(ServiceNamespace, ResourceId, ScalableDimension)` triple
/// that does not describe one scalable resource: "Unsupported service namespace,
/// resource type or scalable dimension".
///
/// `pub` because every operation that names a scalable resource — the two write paths
/// through [`validate_scaling_instructions`] and the forecast read in
/// `crate::ops::get_scaling_plan_resource_forecast_data` — refuses such a triple with
/// this one sentence.
pub const UNSUPPORTED_RESOURCE: &str =
    "Unsupported service namespace, resource type or scalable dimension";

/// The segment a DynamoDB global secondary index's resource id carries, from the same
/// doc: "the resource type is `index` and the unique identifier is the resource ID.
/// Example: `table/my-table/index/my-table-index`".
const INDEX_SEGMENT: &str = "/index/";

/// The identity of the one scalable resource a scaling instruction applies to.
///
/// "Each scaling instruction applies to one resource" (`ScalingInstruction` doc), and
/// that resource is named by the `(ServiceNamespace, ResourceId, ScalableDimension)`
/// triple — so this is the identity [`validate_scaling_instructions`] holds one
/// instruction per, the one a policy of that resource is named after, and the one the
/// forecast read
/// takes an instruction by.
pub type ResourceIdentity<'a> = (&'a str, &'a str, &'a str);

/// The [`ResourceIdentity`] of `instruction`, rendered as one flat string.
///
/// A paged read of the resources a plan covers has to address one resource by something
/// that stays with it while the plan's instruction list is rewritten, and this is that
/// address: the same triple [`validate_scaling_instructions`] holds one instruction per
/// and a policy of that resource is named after, written out as a single comparable value
/// so it can both ORDER the resources a plan covers and travel in a `NextToken`.
///
/// A `ResourceId` is caller-supplied text under no modelled pattern beyond
/// `^[\u0020-\uD7FF…]*$`, separator characters included, so each part reaches the key
/// through [`escape_key_part`] and two different resources cannot render one key —
/// which, since the key orders the walk, is what keeps the order total.
pub fn resource_identity_key(instruction: &ScalingInstruction) -> String {
    let (namespace, resource_id, dimension) = resource_identity(instruction);
    format!(
        "{}/{}/{}",
        escape_key_part(namespace),
        escape_key_part(resource_id),
        escape_key_part(dimension)
    )
}

/// The [`ResourceIdentity`] of `instruction`.
///
/// Public because the forecast path asks the same question: the forecast of a resource is
/// the forecast of the instruction whose identity is the triple the request names.
pub fn resource_identity(instruction: &ScalingInstruction) -> ResourceIdentity<'_> {
    (
        instruction.service_namespace.as_str(),
        instruction.resource_id.as_str(),
        instruction.scalable_dimension.as_str(),
    )
}

/// The identity of the metric a target tracking configuration tracks.
///
/// "Each of these structures must specify a unique scaling metric and a target value
/// for the metric" (`ScalingInstruction$TargetTrackingConfigurations`), so a
/// configuration is identified within its instruction by that metric: the predefined
/// metric type with the resource label it is measured on, or the namespace, name,
/// statistic and dimensions of a customized metric — in both cases EVERY modelled part
/// that says which metric is tracked. `None` for a configuration naming neither metric —
/// which both write paths refuse before it can be stored.
///
/// A customized metric's parts are caller-supplied text under no modelled pattern at
/// all (`MetricNamespace`, `MetricName`, `MetricDimensionName` and
/// `MetricDimensionValue` are plain strings), so each is escaped on the way into the
/// key and two different metrics cannot render one identity. The dimensions are the set
/// of dimensions the metric was published with, so they reach the key through
/// [`set_key_parts`]: a reordered or a repeated dimension is the same set, hence the
/// same metric.
///
/// A predefined metric is identified by the metric TYPE — one part, and one the model
/// constrains to its own value list — together with the `ResourceLabel` that names what
/// the metric is measured on where the specification carries one: the label "identifies
/// the resource associated with the metric type", being "the final portion of the load
/// balancer ARN and the final portion of the target group ARN", so two specifications of
/// one metric type naming two labels track two different resources' metrics and must not
/// render one identity. The label is caller-supplied text (1-1023 characters, the forward
/// slash its own format relies on included), so it reaches the key through
/// [`escape_key_part`] like every other such part; a specification that carries no label
/// renders the type alone, and so stays a third identity.
pub fn target_metric_identity(configuration: &TargetTrackingConfiguration) -> Option<String> {
    if let Some(predefined) = &configuration.predefined_scaling_metric_specification {
        let metric_type = escape_key_part(&predefined.predefined_scaling_metric_type);
        return Some(match &predefined.resource_label {
            Some(label) => format!("{metric_type}/{}", escape_key_part(label)),
            None => metric_type,
        });
    }
    let customized = configuration
        .customized_scaling_metric_specification
        .as_ref()?;
    let dimensions = set_key_parts(
        customized
            .dimensions
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|dimension| {
                format!(
                    "{}={}",
                    escape_key_part(&dimension.name),
                    escape_key_part(&dimension.value)
                )
            }),
    );
    Some(format!(
        "{}/{}/{}[{}]",
        escape_key_part(&customized.namespace),
        escape_key_part(&customized.metric_name),
        escape_key_part(&customized.statistic),
        dimensions.join(",")
    ))
}

/// The refusal for a write whose `ScalingInstructions` list carries no entry.
///
/// The list is `@required` but carries no modelled `@length`, so nothing before a
/// handler applies the "at least one instruction" rule; the whole sentence is fixed and
/// interpolates nothing from the request. `pub` so both write paths word it once.
pub const EMPTY_SCALING_INSTRUCTIONS: &str = "Must provide at least 1 scaling instruction";

/// What `ScalingInstructions` counts, as its own documentation names them ("The scaling
/// instructions"), for the refusal that states the bound.
pub const SCALING_INSTRUCTION_ENTRIES: &str = "scaling instructions";

/// The most scaling instructions one plan may hold.
///
/// "Each scaling instruction applies to one resource", so this is also the number of
/// scalable resources one plan covers, and — with the documented
/// [`MAX_TARGET_TRACKING_CONFIGURATIONS`] per resource — what bounds everything one plan
/// makes the service do: the policies it installs, the resources both describes page over,
/// and the work of applying its configuration.
///
/// AWS enforces a per-plan quota here but does not publish its value in the model, so the
/// NUMBER is chosen, as [`MAX_SCALING_PLANS`]'s is; what is modelled is that one plan is
/// bounded, so a single request cannot make the plan — or the work of applying it — grow
/// without limit.
pub const MAX_SCALING_INSTRUCTIONS: usize = 40;

/// Validate the `ScalingInstructions` a caller wrote.
///
/// `CreateScalingPlan` and `UpdateScalingPlan` both take this list and both model
/// `ValidationException`, so both refuse the same input the same way, before touching
/// state. `Err` carries the message naming the offending member and the constraint it
/// broke.
///
/// The LIST's own bounds come first — a list with no entry at all, and one past
/// [`MAX_SCALING_INSTRUCTIONS`] — so neither the walk below nor anything a write path does
/// with the list afterwards is driven by a count this service does not hold.
pub fn validate_scaling_instructions(instructions: &[ScalingInstruction]) -> Result<(), String> {
    if instructions.is_empty() {
        return Err(EMPTY_SCALING_INSTRUCTIONS.to_string());
    }
    check_entry_count(
        SCALING_INSTRUCTION_ENTRIES,
        instructions.len(),
        MAX_SCALING_INSTRUCTIONS,
    )?;
    // "Each scaling instruction applies to one resource" (`ScalingInstruction` doc), so
    // two instructions for one scalable resource contradict each other.
    let mut covered: BTreeSet<ResourceIdentity<'_>> = BTreeSet::new();
    for (index, instruction) in instructions.iter().enumerate() {
        validate_scaling_instruction(index, instruction)?;
        if !covered.insert(resource_identity(instruction)) {
            return Err(format!(
                "ScalingInstructions[{index}]: a scaling plan can specify only one scaling \
                 instruction per scalable resource, and '{}' ({}) is already covered",
                instruction.resource_id, instruction.scalable_dimension
            ));
        }
    }
    Ok(())
}

/// Every relation one `ScalingInstruction` has to satisfy on its own.
fn validate_scaling_instruction(
    index: usize,
    instruction: &ScalingInstruction,
) -> Result<(), String> {
    let at = |member: &str| format!("ScalingInstructions[{index}].{member}");

    one_of(
        &at("ServiceNamespace"),
        &instruction.service_namespace,
        SERVICE_NAMESPACE_VALUES,
    )?;
    one_of(
        &at("ScalableDimension"),
        &instruction.scalable_dimension,
        SCALABLE_DIMENSION_VALUES,
    )?;
    validate_resource_triple(
        &instruction.service_namespace,
        &instruction.resource_id,
        &instruction.scalable_dimension,
    )?;

    // "The minimum capacity of the resource" / "The maximum capacity of the resource": the
    // floor of the band an instruction registers is [`MIN_RESOURCE_CAPACITY`], and the band
    // itself is not inverted. Each refusal names the members in prose, the way the service
    // words them.
    if instruction.min_capacity < MIN_RESOURCE_CAPACITY {
        return Err(MINIMUM_CAPACITY_BELOW_FLOOR.to_string());
    }
    if instruction.max_capacity < instruction.min_capacity {
        return Err(MAXIMUM_CAPACITY_BELOW_MINIMUM.to_string());
    }

    // "The amount of time, in seconds, to buffer the run time of scheduled scaling
    // actions": the model already bounds this at 0, which is checked again here so the
    // same refusal holds however the value reaches the handler; the upper bound and the
    // mode condition the same paragraph states are enforced beside it.
    if let Some(seconds) = instruction.scheduled_action_buffer_time {
        let member = at("ScheduledActionBufferTime");
        if seconds < 0 {
            return Err(format!("{member}: a time in seconds cannot be negative"));
        }
        // "The value must be less than the forecast interval duration of 3600 seconds
        // (60 minutes)."
        if i64::from(seconds) >= FORECAST_INTERVAL_SECONDS {
            return Err(format!(
                "{member}: Value '{seconds}' failed to satisfy constraint: Member must be \
                 less than the forecast interval duration of {FORECAST_INTERVAL_SECONDS} \
                 seconds"
            ));
        }
        // "Only valid when configuring predictive scaling."
        if !configures_predictive_scaling(instruction) {
            return Err(predictive_scaling_only(&member));
        }
    }
    if let Some(behavior) = instruction.scaling_policy_update_behavior.as_deref() {
        one_of(
            &at("ScalingPolicyUpdateBehavior"),
            behavior,
            SCALING_POLICY_UPDATE_BEHAVIOR_VALUES,
        )?;
        // "Only valid when configuring dynamic scaling."
        if !configures_dynamic_scaling(instruction) {
            return Err(dynamic_scaling_only(&at("ScalingPolicyUpdateBehavior")));
        }
    }

    validate_target_tracking_configurations(index, instruction)?;
    validate_predictive_scaling(index, instruction)
}

/// A `(ServiceNamespace, ResourceId, ScalableDimension)` triple has to describe ONE
/// scalable resource, whichever member carries it.
///
/// Two relations make the triple describe a resource, and nothing else about it is a
/// request error:
///
/// 1. every `ScalableDimension` value is prefixed with the namespace of the service that
///    owns it (`smithy_query(query="shape",shape="ScalableDimension")`), so a dimension
///    from another service than the `ServiceNamespace` beside it names no resource;
/// 2. the `ResourceId` identifies a resource of the kind that dimension scales
///    ([`validate_resource_id`]).
///
/// `pub` because every operation that names a scalable resource asks this same question:
/// the two write paths through [`validate_scaling_instructions`] about a scaling
/// instruction's triple, and the forecast read in
/// `crate::ops::get_scaling_plan_resource_forecast_data` about its own — so one triple is
/// admitted, and refused, the same way wherever it is carried. WHICH resources this
/// service actually holds a forecast or a policy for is decided by the stored plan, not
/// here.
pub fn validate_resource_triple(
    service_namespace: &str,
    resource_id: &str,
    scalable_dimension: &str,
) -> Result<(), String> {
    let dimension_namespace = scalable_dimension.split(':').next().unwrap_or_default();
    if dimension_namespace != service_namespace {
        return Err(UNSUPPORTED_RESOURCE.to_string());
    }
    validate_resource_id(resource_id, scalable_dimension)
}

/// A resource id has to identify a resource of the kind its scalable dimension scales.
///
/// "This string consists of the resource type and unique identifier"
/// (`ScalingInstruction$ResourceId`), so an id names a resource only when the resource
/// type prefix of its scalable dimension is followed by a unique identifier that is not
/// empty — a bare prefix carries no identifier and names nothing, exactly as a present
/// but empty value names nothing everywhere else in this module. Where that identifier
/// addresses a resource THROUGH a parent — a DynamoDB global secondary index, whose
/// identifier is its table then its index ("the unique identifier is the resource ID.
/// Example: `table/my-table/index/my-table-index`") — neither part may be empty either,
/// since each one names a resource of its own.
///
/// `pub` so the forecast read applies the same rule to its own `ResourceId` as the two
/// write paths apply to a scaling instruction's.
pub fn validate_resource_id(resource_id: &str, dimension: &str) -> Result<(), String> {
    // A dimension the model does not list is refused by its own enum check, not here.
    let Some((prefix, _)) = scalable_dimension(dimension) else {
        return Ok(());
    };
    let resource_type = dimension.split(':').nth(1).unwrap_or_default();
    let identifies = resource_id
        .strip_prefix(prefix)
        .is_some_and(|identifier| match resource_type {
            // A global secondary index is addressed through its table, a table is not.
            "index" => identifier
                .split_once(INDEX_SEGMENT)
                .is_some_and(|(table, index)| !table.is_empty() && !index.is_empty()),
            "table" => !identifier.is_empty() && !resource_id.contains(INDEX_SEGMENT),
            _ => !identifier.is_empty(),
        });
    if !identifies {
        return Err(UNSUPPORTED_RESOURCE.to_string());
    }
    Ok(())
}

/// The target tracking configurations of one instruction: "(up to 10). Each of these
/// structures must specify a unique scaling metric and a target value for the metric."
fn validate_target_tracking_configurations(
    index: usize,
    instruction: &ScalingInstruction,
) -> Result<(), String> {
    let member = format!("ScalingInstructions[{index}].TargetTrackingConfigurations");
    let configurations = &instruction.target_tracking_configurations;
    if configurations.is_empty() {
        return Err(format!(
            "{member}: at least one target tracking configuration is required"
        ));
    }
    // A documented entry count is worded by [`check_entry_count`], the one rendering the
    // crate gives this refusal, so this list past its bound reads exactly as every other
    // list past its own does.
    check_entry_count(
        TARGET_TRACKING_ENTRIES,
        configurations.len(),
        MAX_TARGET_TRACKING_CONFIGURATIONS,
    )?;
    let mut metrics: BTreeSet<String> = BTreeSet::new();
    for (position, configuration) in configurations.iter().enumerate() {
        let at = format!("{member}[{position}]");
        let metric = validate_target_tracking_configuration(
            &at,
            &instruction.scalable_dimension,
            configuration,
        )?;
        if !metrics.insert(metric.clone()) {
            return Err(format!(
                "{at}: each target tracking configuration must specify a unique scaling \
                 metric, and '{metric}' is already specified"
            ));
        }
    }
    Ok(())
}

/// One target tracking configuration, answering the identity of the metric it tracks so
/// the caller can enforce uniqueness across the instruction.
fn validate_target_tracking_configuration(
    at: &str,
    dimension: &str,
    configuration: &TargetTrackingConfiguration,
) -> Result<String, String> {
    let target_value = configuration.target_value;
    if !(TARGET_VALUE_MIN..=TARGET_VALUE_MAX).contains(&target_value) {
        return Err(format!(
            "For target tracking scaling, target value must be between \
             '{TARGET_VALUE_MIN_TEXT}' and '{TARGET_VALUE_MAX_TEXT}', but was \
             '{target_value:?}'."
        ));
    }
    // "The amount of time, in seconds, …" — none of the three is a negative span.
    for (name, seconds) in [
        ("ScaleOutCooldown", configuration.scale_out_cooldown),
        ("ScaleInCooldown", configuration.scale_in_cooldown),
        (
            "EstimatedInstanceWarmup",
            configuration.estimated_instance_warmup,
        ),
    ] {
        if seconds.is_some_and(|value| value < 0) {
            return Err(format!("{at}.{name}: a time in seconds cannot be negative"));
        }
    }

    // "You can specify either a predefined metric or a customized metric": one of the
    // two, never neither and never both.
    match (
        &configuration.predefined_scaling_metric_specification,
        &configuration.customized_scaling_metric_specification,
    ) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "{at}: specify either PredefinedScalingMetricSpecification or \
                 CustomizedScalingMetricSpecification, not both"
            ))
        }
        (None, None) => return Err(metric_specification_required(at)),
        (Some(predefined), None) => validate_predefined_scaling_metric(at, dimension, predefined)?,
        (None, Some(customized)) => one_of(
            &format!("{at}.CustomizedScalingMetricSpecification.Statistic"),
            &customized.statistic,
            METRIC_STATISTIC_VALUES,
        )?,
    }
    // The identity of the metric this configuration tracks has ONE definition, shared
    // with the policy name a resource's configuration is reported under.
    target_metric_identity(configuration).ok_or_else(|| metric_specification_required(at))
}

/// How the service refuses a target tracking configuration that names no metric: "You
/// can specify either a predefined metric or a customized metric", so one of the two is
/// required.
fn metric_specification_required(at: &str) -> String {
    format!(
        "{at}: specify either PredefinedScalingMetricSpecification or \
         CustomizedScalingMetricSpecification"
    )
}

/// Whether `label` is a resource label, i.e. is in the format both `ResourceLabel`
/// members document ([`RESOURCE_LABEL_FORMAT`]).
///
/// The value is "the final portion of the load balancer ARN and the final portion of the
/// target group ARN" joined "by a forward slash (/)": the load balancer's portion is its
/// `app/<name>/<id>` resource path and the target group's is its
/// `targetgroup/<name>/<id>` one, so a label is exactly six slash-separated segments —
/// the two markers in their documented places, each followed by a name and an id that
/// name something. The modelled `length {min: 1, max: 1023}` is what bounds the value's
/// SIZE, so nothing here repeats that.
fn is_resource_label(label: &str) -> bool {
    let segments: Vec<&str> = label.split('/').collect();
    let [marker, lb_name, lb_id, group_marker, group_name, group_id] = segments.as_slice() else {
        return false;
    };
    *marker == LOAD_BALANCER_MARKER
        && *group_marker == TARGET_GROUP_MARKER
        && [lb_name, lb_id, group_name, group_id]
            .iter()
            .all(|segment| !segment.is_empty())
}

/// Refuse a `ResourceLabel` a caller wrote that is not in the format the member's own
/// documentation states; a label in that format, and a specification carrying none, are
/// accepted.
///
/// Both members that carry a resource label document ONE format for it, so both write
/// paths check it here and word the refusal the same way: `member` is the wire path of
/// the specification the label sits in, as this crate's validation failures name it.
fn check_resource_label(member: &str, label: Option<&str>) -> Result<(), String> {
    match label {
        Some(label) if !is_resource_label(label) => Err(format!(
            "{member}.ResourceLabel: Value '{label}' failed to satisfy constraint: Member \
             must be in the format {RESOURCE_LABEL_FORMAT}"
        )),
        _ => Ok(()),
    }
}

/// A predefined scaling metric: a modelled metric type that measures the resource the
/// instruction scales, and a resource label only where one is allowed.
fn validate_predefined_scaling_metric(
    at: &str,
    dimension: &str,
    specification: &PredefinedScalingMetricSpecification,
) -> Result<(), String> {
    let member = format!("{at}.PredefinedScalingMetricSpecification");
    let metric = specification.predefined_scaling_metric_type.as_str();
    one_of(
        &format!("{member}.PredefinedScalingMetricType"),
        metric,
        SCALING_METRIC_TYPE_VALUES,
    )?;
    // A predefined metric measures one kind of resource, so only the metrics of the
    // resource this instruction scales can be tracked for it.
    if let Some((_, supported)) = scalable_dimension(dimension) {
        if !supported.contains(&metric) {
            return Err(format!(
                "Scalable dimension {dimension} only supports the following predefined \
                 metric types: {}",
                supported.join(", ")
            ));
        }
    }
    if specification.resource_label.is_some() && metric != ALB_SCALING_METRIC_TYPE {
        return Err(format!(
            "{member}.ResourceLabel: a resource label can only be specified when the metric \
             type is {ALB_SCALING_METRIC_TYPE}"
        ));
    }
    // A label that may be specified still has to BE a resource label: it identifies the
    // target group the metric is measured on, and only the documented format does.
    check_resource_label(&member, specification.resource_label.as_deref())
}

/// Whether this instruction configures predictive scaling.
///
/// "The predefined load metric to use for predictive scaling. This parameter or a
/// CustomizedLoadMetricSpecification is required when configuring predictive scaling,
/// and cannot be used otherwise" — so a load metric specification is exactly what turns
/// predictive scaling on, and every member documented "Only valid when configuring
/// predictive scaling" is refused without one.
///
/// Public because the forecast path asks the same question: an instruction configuring
/// no predictive scaling has no forecast to report.
pub fn configures_predictive_scaling(instruction: &ScalingInstruction) -> bool {
    instruction.predefined_load_metric_specification.is_some()
        || instruction.customized_load_metric_specification.is_some()
}

/// Whether this instruction configures dynamic scaling.
///
/// `DisableDynamicScaling` "Controls whether dynamic scaling by AWS Auto Scaling is
/// disabled… The default is enabled (`false`)", so dynamic scaling is configured unless
/// the caller turned it off, and every member documented "Only valid when configuring
/// dynamic scaling" is refused once it is off.
///
/// Public because the one predicate also decides which resources of a plan carry target
/// tracking scaling policies at all — the policies this crate reports
/// ([`ScalingPlanRecord::applied_policies`]) and the ones it installs in the service that
/// owns them ([`application_auto_scaling`]).
pub fn configures_dynamic_scaling(instruction: &ScalingInstruction) -> bool {
    !instruction.disable_dynamic_scaling.unwrap_or(false)
}

/// How the service refuses a member documented "Only valid when configuring predictive
/// scaling" on an instruction that configures none, naming the member at fault.
fn predictive_scaling_only(member: &str) -> String {
    format!(
        "{member}: only valid when configuring predictive scaling, which requires a load \
         metric specification"
    )
}

/// The same refusal for a member documented "Only valid when configuring dynamic
/// scaling" on an instruction that disabled it.
fn dynamic_scaling_only(member: &str) -> String {
    format!(
        "{member}: only valid when configuring dynamic scaling, which is disabled by \
         DisableDynamicScaling"
    )
}

/// The predictive-scaling members of one instruction, each of which is documented as
/// valid only when a load metric specification configures predictive scaling.
fn validate_predictive_scaling(
    index: usize,
    instruction: &ScalingInstruction,
) -> Result<(), String> {
    let at = |member: &str| format!("ScalingInstructions[{index}].{member}");
    let predefined = instruction.predefined_load_metric_specification.as_ref();
    let customized = instruction.customized_load_metric_specification.as_ref();

    // "This parameter or a CustomizedLoadMetricSpecification is required when
    // configuring predictive scaling, and cannot be used otherwise."
    if predefined.is_some() && customized.is_some() {
        return Err(format!(
            "{}: specify either PredefinedLoadMetricSpecification or \
             CustomizedLoadMetricSpecification, not both",
            at("PredefinedLoadMetricSpecification")
        ));
    }
    if let Some(specification) = predefined {
        let member = at("PredefinedLoadMetricSpecification");
        one_of(
            &format!("{member}.PredefinedLoadMetricType"),
            &specification.predefined_load_metric_type,
            LOAD_METRIC_TYPE_VALUES,
        )?;
        if specification.resource_label.is_some()
            && specification.predefined_load_metric_type != ALB_LOAD_METRIC_TYPE
        {
            return Err(format!(
                "{member}.ResourceLabel: a resource label can only be specified when the \
                 metric type is {ALB_LOAD_METRIC_TYPE}"
            ));
        }
        // The same documented format decides what a label on this member may be.
        check_resource_label(&member, specification.resource_label.as_deref())?;
    }
    if let Some(specification) = customized {
        let member = format!("{}.Statistic", at("CustomizedLoadMetricSpecification"));
        // The member targets the modelled `MetricStatistic` enum, so a value that enum
        // does not list is refused by [`one_of`] like every other enum-typed member of
        // this service — BEFORE the narrower rule below, which speaks about a statistic
        // the enum does list.
        one_of(&member, &specification.statistic, METRIC_STATISTIC_VALUES)?;
        // "The statistic of the metric. The only valid value is Sum."
        if specification.statistic != LOAD_METRIC_STATISTIC {
            return Err(format!(
                "{member}: the only valid value is {LOAD_METRIC_STATISTIC}"
            ));
        }
    }

    let predictive = configures_predictive_scaling(instruction);
    if let Some(mode) = instruction.predictive_scaling_mode.as_deref() {
        one_of(&at("PredictiveScalingMode"), mode, PREDICTIVE_SCALING_MODE_VALUES)?;
    }
    let behavior = instruction
        .predictive_scaling_max_capacity_behavior
        .as_deref();
    if let Some(behavior) = behavior {
        one_of(
            &at("PredictiveScalingMaxCapacityBehavior"),
            behavior,
            PREDICTIVE_SCALING_MAX_CAPACITY_BEHAVIOR_VALUES,
        )?;
        // "Only valid when configuring predictive scaling."
        if !predictive {
            return Err(predictive_scaling_only(&at(
                "PredictiveScalingMaxCapacityBehavior",
            )));
        }
    }

    // "Required if the PredictiveScalingMaxCapacityBehavior is set to
    // SetMaxCapacityAboveForecastCapacity, and cannot be used otherwise. The range is
    // 1-100."
    let above_forecast = behavior == Some(BEHAVIOR_ABOVE_FORECAST_CAPACITY);
    match instruction.predictive_scaling_max_capacity_buffer {
        // "Only valid when configuring predictive scaling."
        Some(_) if !predictive => Err(predictive_scaling_only(&at(
            "PredictiveScalingMaxCapacityBuffer",
        ))),
        Some(_) if !above_forecast => Err(format!(
            "{}: can only be specified when PredictiveScalingMaxCapacityBehavior is \
             {BEHAVIOR_ABOVE_FORECAST_CAPACITY}",
            at("PredictiveScalingMaxCapacityBuffer")
        )),
        Some(buffer)
            if !(MAX_CAPACITY_BUFFER_MIN..=MAX_CAPACITY_BUFFER_MAX).contains(&buffer) =>
        {
            Err(format!(
                "{}: Value '{buffer}' failed to satisfy constraint: Member must be between \
                 {MAX_CAPACITY_BUFFER_MIN} and {MAX_CAPACITY_BUFFER_MAX}",
                at("PredictiveScalingMaxCapacityBuffer")
            ))
        }
        None if above_forecast => Err(format!(
            "{}: required when PredictiveScalingMaxCapacityBehavior is \
             {BEHAVIOR_ABOVE_FORECAST_CAPACITY}",
            at("PredictiveScalingMaxCapacityBuffer")
        )),
        _ => Ok(()),
    }
}

/// Refuse a value the model's own enum does not list, naming the member and the values
/// it accepts.
///
/// `pub` so every handler refuses an off-enum value with the same sentence.
pub fn one_of(member: &str, value: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "{member}: Value '{value}' failed to satisfy constraint: Member must satisfy enum \
         value set: [{}]",
        allowed.join(", ")
    ))
}

/// `ScalingPlanName`'s modelled `@pattern`, as a constraint-violation refusal states it.
///
/// The model writes the pattern anchored (`^[\p{Print}&&[^|:/]]+$`,
/// `smithy_query(query="shape",shape="ScalingPlanName")`); a refusal states the pattern
/// BODY — the character class and its repetition — because the anchoring is how the
/// model says "the whole value", not part of the expression a caller has to satisfy.
pub const SCALING_PLAN_NAME_PATTERN: &str = r"[\p{Print}&&[^|:/]]+";

/// The characters `ScalingPlanName`'s `@pattern` subtracts from the printable set:
/// "Names cannot contain vertical bars, colons, or forward slashes"
/// (`smithy_query(query="doc",shape="CreateScalingPlanRequest",member="ScalingPlanName")`).
///
/// They are the separators this crate derives identifiers with — the policy names and
/// the resource-identity keys a plan name travels in — so a name carrying one of them
/// would make two different plans render one identifier.
pub const SCALING_PLAN_NAME_EXCLUDED: [char; 3] = ['|', ':', '/'];

/// The validation path of a top-level `ScalingPlanName` member, as a refusal names it.
pub fn scaling_plan_name_path() -> String {
    ::aws_smithy_schema_server::constraints::path::member("ScalingPlanName")
}

/// The validation path of the `position`-th (1-based) entry of a `ScalingPlanNames`
/// list member, as a refusal names it.
pub fn scaling_plan_names_path(position: usize) -> String {
    use ::aws_smithy_schema_server::constraints::path;

    path::join(&[path::member("ScalingPlanNames"), path::element(position)])
}

/// Whether `value` is inside the character set `ScalingPlanName`'s `@pattern` admits.
///
/// The pattern's set is the printable characters with three delimiters subtracted, and
/// it requires at least one of them. "Printable" is the class the pattern names: a
/// character that is not a control character — so a name carrying a line break, a tab or
/// any other control character is outside it, as is the empty name (which the member's
/// modelled `@length` minimum of 1 refuses before a handler runs).
fn satisfies_scaling_plan_name_pattern(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| {
                !character.is_control() && !SCALING_PLAN_NAME_EXCLUDED.contains(&character)
            })
}

/// Refuse a scaling-plan name outside `ScalingPlanName`'s modelled `@pattern`, naming
/// the member at `member_path`.
///
/// The constraint belongs to the SHAPE, so it holds wherever the shape is carried —
/// every operation takes a `ScalingPlanName`, and one of them takes a list of them — and
/// `pub` is what lets each of them apply it through this one function. The sentence is
/// rendered by the framework's own constraint renderer, so a pattern violation reads
/// exactly like the `@length` violation on the same member that the framework itself
/// refuses before a handler runs, and `Err` carries the whole
/// `N validation error(s) detected: …` envelope those refusals are serialized in.
pub fn check_scaling_plan_name(member_path: &str, value: &str) -> Result<(), String> {
    use ::aws_smithy_schema_server::constraints::{envelope, Violation, ViolationKind};

    if satisfies_scaling_plan_name_pattern(value) {
        return Ok(());
    }
    let violation = Violation {
        path: member_path.to_string(),
        kind: ViolationKind::Pattern(SCALING_PLAN_NAME_PATTERN.to_string()),
        // The offending value, echoed from the request: a name past the member's
        // modelled maximum never reaches here, since the framework refuses its
        // `@length` first.
        value: Some(value.to_string()),
        sensitive: false,
    };
    // One member outside one pattern is one violation, so the envelope always renders.
    Err(envelope(&[violation]).unwrap_or_default())
}

/// AWS's wording for "the name and version in this request name no stored plan",
/// built from the request's own `ScalingPlanName` and `ScalingPlanVersion`.
///
/// Every operation that looks a plan up by name and version refuses with this one
/// text, whatever error shape it models the refusal in (`ObjectNotFoundException`
/// where the operation models it, `ValidationException` where it does not), so a
/// client that discriminates not-found variants by message reads the same sentence
/// on every path.
pub fn plan_not_found_message(name: &str, version: i64) -> String {
    format!("No scaling plan found for scaling plan name: {name}, version: {version}")
}

/// The refusal wording for an application source another plan already claims, built
/// from the name of the plan that claims it.
///
/// Both write paths (`CreateScalingPlan` and `UpdateScalingPlan`) refuse with this one
/// text, so the single-plan-per-application-source rule reads the same however the
/// source was about to be claimed.
pub fn application_source_covered_message(covered_by: &str) -> String {
    format!("The application source is already covered by the scaling plan '{covered_by}'")
}

/// Encode a pagination cursor as an opaque token.
///
/// The `kind` prefix is part of the encoded value, so a token minted by one
/// operation does not decode for another.
pub fn encode_cursor(kind: &str, value: &str) -> String {
    hex::encode(format!("{kind}:{value}"))
}

/// Decode a cursor minted by [`encode_cursor`] for the same `kind`.
///
/// `None` for anything this service did not mint — the caller answers
/// `InvalidNextTokenException` with [`INVALID_NEXT_TOKEN`].
///
/// A token is accepted only when it is the EXACT output [`encode_cursor`] would have
/// produced for the value read back out of it, so decoding is the inverse of minting
/// rather than a prefix test on a decoded string. That rejects two shapes a prefix test
/// lets through: a token carrying an EMPTY value, which no minting site can produce
/// (every cursor value this service mints is a plan name, which is modelled non-empty,
/// or a resource identity key, which always carries its separators), and a spelling of
/// the hex that differs from the one this service hands out (`hex::decode` also accepts
/// upper case). Either one would otherwise resume a walk from a position the service
/// never offered.
pub fn decode_cursor(kind: &str, token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    let value = decoded.strip_prefix(kind)?.strip_prefix(':')?;
    if value.is_empty() || encode_cursor(kind, value) != token {
        return None;
    }
    Some(value.to_string())
}

/// The refusal wording for a `NextToken` this service cannot decode.
///
/// The whole sentence is fixed text: it names the member and states that its value is
/// not valid, and echoes nothing of the rejected token. `pub` and used by every
/// paginated read, so the two operations that accept a `NextToken` cannot drift into two
/// spellings of one refusal.
pub const INVALID_NEXT_TOKEN: &str = "The NextToken value is not valid";

/// The page size to use, honouring the documented 1..=50 range and the documented
/// default of 50.
///
/// `Err` carries the refusal message naming the member and the constraint it broke.
pub fn effective_max_results(requested: Option<i32>) -> Result<usize, String> {
    match requested {
        None => Ok(MAX_RESULTS_MAX as usize),
        Some(value) if (MAX_RESULTS_MIN..=MAX_RESULTS_MAX).contains(&value) => Ok(value as usize),
        Some(value) => Err(format!(
            "1 validation error detected: Value '{value}' at 'maxResults' failed to satisfy \
             constraint: Member must be between {MAX_RESULTS_MIN} and {MAX_RESULTS_MAX}"
        )),
    }
}

/// Take one page out of an ordered walk, and mint the token that continues it.
///
/// The whole page rule lives here, once: `entries` is walked in the order it already
/// has, resumption starts STRICTLY AFTER the key the previous page stopped at (`after`,
/// as [`decode_cursor`] handed it back), one entry MORE than the page is taken so "is
/// there another page?" is answered from state rather than guessed, and the token —
/// minted by [`encode_cursor`] under `kind`, so it decodes only for the operation that
/// minted it — carries the key of the last entry KEPT.
///
/// `key` reads the key an entry is ordered and resumed by; `entries` must already be in
/// that key's order, since the walk resumes by comparing keys rather than by counting
/// positions. `pub` so every paginated read in `src/ops/` pages the same way: a change
/// to how a page boundary is decided is made here, not once per handler.
pub fn take_page<T, I, K>(
    kind: &str,
    entries: I,
    after: Option<&str>,
    page_size: usize,
    key: K,
) -> (Vec<T>, Option<String>)
where
    I: IntoIterator<Item = T>,
    K: Fn(&T) -> &str,
{
    let mut page: Vec<T> = entries
        .into_iter()
        .filter(|entry| after.is_none_or(|last| key(entry) > last))
        .take(page_size + 1)
        .collect();
    if page.len() <= page_size {
        return (page, None);
    }
    page.truncate(page_size);
    let next_token = page.last().map(|entry| encode_cursor(kind, key(entry)));
    (page, next_token)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAutoscalingplansAnyscalescalingplannerfrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAutoscalingplansAnyscalescalingplannerfrontendserviceState>)
}
