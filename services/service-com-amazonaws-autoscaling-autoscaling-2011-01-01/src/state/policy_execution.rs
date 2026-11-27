// SPDX-License-Identifier: CC0-1.0

//! Performing a scaling policy of an Auto Scaling group: which policy a request names,
//! the capacity performing it moves the group to, and the cooldown it leaves behind.
//!
//! # The rule this module implements
//!
//! A scaling policy states an ADJUSTMENT of the capacity the group maintains, and
//! performing the policy applies that adjustment: `AdjustmentType` says how the number it
//! carries is read — "The valid values are ChangeInCapacity, ExactCapacity, and
//! PercentChangeInCapacity" (`AdjustmentType$AdjustmentType`) — and the resulting capacity
//! is brought inside the group's own `MinSize`..`MaxSize`, because a group never holds a
//! capacity outside the bounds it was configured with. From there the group converges the
//! way it does for every other change of its size
//! ([`AutoScalingState::apply_capacity`]): it launches what is missing and scales in what
//! is surplus, and each movement records its own scaling activity.
//!
//! Which number is adjusted by depends on the policy's type, and only the two types that
//! state one of their own can be performed at all ([`EXECUTABLE_POLICY_TYPES`]):
//!
//! * a `SimpleScaling` policy IS one adjustment — "Required if the policy type is
//!   SimpleScaling. (Not used with any other policy type.)"
//!   (`PutScalingPolicyType$ScalingAdjustment`) — so performing it applies that one;
//! * a `StepScaling` policy carries a step per interval of the difference between the
//!   metric and the alarm threshold, so performing it needs the two values that locate the
//!   metric in those intervals: "The metric value to compare to BreachThreshold … Required
//!   if the policy type is StepScaling and not supported otherwise"
//!   (`ExecutePolicyType$MetricValue`) and the threshold itself
//!   (`$BreachThreshold`). "If you specify a metric value that doesn't correspond to a step
//!   adjustment for the policy, the call returns an error."
//!
//! A `TargetTrackingScaling` or `PredictiveScaling` policy states a TARGET for the metric
//! its own alarms watch rather than an adjustment, so no adjustment can be read off it,
//! and `ExecutePolicy`'s own members carry none for it — which is why `ExecutePolicy`
//! refuses such a policy outright.
//!
//! A target-tracking policy is performed by the ALARMS it owns instead, and what they hand
//! it is the value of the metric it tracks ([`super::alarm_driven_policies`]). The target is
//! the value the metric is to sit AT — "the target value for the metric"
//! (`TargetTrackingConfiguration$TargetValue`), measured per unit of the capacity the group
//! maintains — so the capacity that holds the metric at or below the target is the smallest
//! one at which the load the metric reports divides down to it:
//! `ceil(current × metric ÷ target)` ([`tracked_capacity`]), inside the group's own bounds
//! like every other adjustment. A policy whose configuration disables scaling in never
//! moves the capacity DOWN for it: "If scaling in is disabled, the target tracking scaling
//! policy doesn't remove instances from the Auto Scaling group"
//! (`TargetTrackingConfiguration$DisableScaleIn`).
//!
//! # The cooldown
//!
//! ONE period, decided by the two members below, both the model's own sentences:
//!
//! * `HonorCooldown` — "Indicates whether Amazon EC2 Auto Scaling waits for the cooldown
//!   period to complete before executing the policy. Valid only if the policy type is
//!   SimpleScaling" — so a simple policy performed with it while that period is still
//!   running after the group's last capacity change is NOT performed: the request is REFUSED with the
//!   modelled `ScalingActivityInProgressFault`
//!   ([`PolicyExecutionRefusal::CooldownInProgress`]) and nothing of the group is touched,
//!   so a policy asked for N times inside one cooldown window moves the capacity once and
//!   the caller is told each further ask was blocked. A policy of any OTHER type honours no
//!   cooldown, so the member holds its change back for nothing and is not refused: "valid
//!   only if" is the same sentence `PutScalingPolicyType$Cooldown` carries, and a member of
//!   that class is kept by the type that owns it and ignored by the types that do not
//!   ([`super::scaling_policies::policy_members_of_type`]) rather than refusing the
//!   request. The members this operation documents as "not supported otherwise" ARE
//!   refused — that is a different sentence.
//! * `Cooldown` — "A cooldown period, in seconds, that applies to a specific simple scaling
//!   policy. When a cooldown period is specified here, it overrides the default cooldown"
//!   (`PutScalingPolicyType$Cooldown`) — so the period in force for an execution of THAT
//!   policy is the policy's own whenever it carries one, and the group's `DefaultCooldown`
//!   otherwise. It is the same period on both sides of the execution: the one the execution
//!   WAITS OUT above ([`super::StoredGroup::in_cooldown_of_span`], measured from the activity
//!   the group's cooldown follows) and the one the performed policy LEAVES BEHIND
//!   ([`AutoScalingState::override_cooldown`]). A policy whose own period has elapsed since
//!   that activity — a policy stating a period of zero, which waits for nothing — is
//!   therefore performed while the group's longer default would still be running, because
//!   that default decides nothing for a policy that overrode it.

use std::time::SystemTime;

use super::scaling_policies::{
    StoredScalingPolicy, DEFAULT_DISABLE_SCALE_IN, POLICY_TYPE_SIMPLE_SCALING,
    POLICY_TYPE_STEP_SCALING, POLICY_TYPE_TARGET_TRACKING,
};
use super::{AutoScalingState, PolicyKey};

/// The policy types a request can perform: the ones whose own configuration states the
/// adjustment to apply, in the order [`super::scaling_policies::POLICY_TYPES`] lists them.
pub const EXECUTABLE_POLICY_TYPES: &[&str] = &[POLICY_TYPE_STEP_SCALING, POLICY_TYPE_SIMPLE_SCALING];

/// Whether a policy of `policy_type` states an adjustment of its own, and so can be
/// performed from the policy document and the request alone.
#[must_use]
pub fn is_executable_policy_type(policy_type: &str) -> bool {
    EXECUTABLE_POLICY_TYPES.contains(&policy_type)
}

/// Whether a policy of `policy_type` is graded against the metric value a request carries:
/// the step policy's own members, "Required if the policy type is StepScaling and not
/// supported otherwise" (`ExecutePolicyType$MetricValue`, `$BreachThreshold`).
#[must_use]
pub fn grades_a_metric_value(policy_type: &str) -> bool {
    policy_type == POLICY_TYPE_STEP_SCALING
}

/// Whether a policy of `policy_type` states a TARGET for the metric it watches rather than
/// an adjustment, so performing it is computing the capacity that holds the metric at that
/// target.
#[must_use]
pub fn tracks_a_target(policy_type: &str) -> bool {
    policy_type == POLICY_TYPE_TARGET_TRACKING
}

/// The number of instances a `PercentChangeInCapacity` adjustment moves the capacity by at
/// least, when the policy states no `MinAdjustmentMagnitude` of its own.
///
/// The percentage of a capacity is a fraction, and the service takes it toward zero: "If
/// the calculated amount … is not an integer … values between 0 and 1 are rounded to 1"
/// (`AdjustmentType`'s scaling-adjustment-types documentation, which
/// `MinAdjustmentMagnitude` raises this floor over). So a non-zero percentage moves the
/// capacity by at least one instance in the direction it names, rather than reporting a
/// change it did not make.
pub const DEFAULT_MIN_ADJUSTMENT_MAGNITUDE: i32 = 1;

/// The breach a step policy is graded by: the metric value the request stated and the alarm
/// threshold it is compared to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetricBreach {
    /// `ExecutePolicyType$MetricValue`.
    pub metric_value: f64,
    /// `ExecutePolicyType$BreachThreshold`.
    pub breach_threshold: f64,
}

impl MetricBreach {
    /// How far the metric is from the threshold a step's interval is measured from.
    #[must_use]
    fn difference(self) -> f64 {
        self.metric_value - self.breach_threshold
    }

    /// Whether the metric is ABOVE the threshold, which decides which end of a step's
    /// interval is inclusive. At the threshold itself the metric is not below it, so it is
    /// graded as the smallest breach above — the end the lower bound includes.
    #[must_use]
    fn above(self) -> bool {
        self.metric_value >= self.breach_threshold
    }
}

/// WHAT asked for a policy to be performed, which is what the scaling activity it records
/// names as its cause.
///
/// One execution path ([`AutoScalingState::execute_scaling_policy`]) however the policy was
/// asked for, so the two askers cannot apply an adjustment differently; what differs is only
/// the cause the activity log carries, and a client reading that log can tell a policy a
/// request performed from one an alarm drove.
#[derive(Debug, Clone, Copy)]
pub enum PolicyTrigger<'a> {
    /// A request of this service's own API named the policy (`ExecutePolicy`).
    UserRequest,
    /// A CloudWatch alarm's action named the policy: the alarm, and the state it moved to
    /// (which is what selected the action list the policy was in).
    Alarm {
        /// The alarm whose transition fired the policy.
        alarm_name: &'a str,
        /// The state that alarm moved to.
        state_value: &'a str,
    },
}

impl PolicyTrigger<'_> {
    /// Whether one of the alarms watching the policy asked for it, rather than a request of
    /// this service's own API.
    ///
    /// What a target-tracking policy is performed BY: its own alarms are the ones that carry
    /// the metric value it tracks, and `ExecutePolicy` carries none for it.
    #[must_use]
    pub fn is_alarm(&self) -> bool {
        matches!(self, Self::Alarm { .. })
    }

    /// The cause the capacity change this trigger asks for records.
    #[must_use]
    pub fn cause(&self, at: SystemTime, policy_name: &str, from: i32, to: i32) -> String {
        match self {
            Self::UserRequest => policy_execution_cause(at, policy_name, from, to),
            Self::Alarm {
                alarm_name,
                state_value,
            } => alarm_execution_cause(at, alarm_name, state_value, policy_name, from, to),
        }
    }
}

/// What a request asks of the policy it names, beside the policy itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct PolicyExecutionRequest {
    /// `ExecutePolicyType$HonorCooldown`.
    pub honor_cooldown: bool,
    /// `ExecutePolicyType$MetricValue`.
    pub metric_value: Option<f64>,
    /// `ExecutePolicyType$BreachThreshold`.
    pub breach_threshold: Option<f64>,
}

/// What performing a policy did: the capacity the group moved between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyExecution {
    /// The desired capacity the group held before the policy was performed.
    pub from: i32,
    /// The desired capacity the policy asks for, inside the group's own bounds.
    pub to: i32,
}

/// Why a policy could not be performed, for the operation to state the refusal each
/// situation belongs to.
#[derive(Debug, Clone, PartialEq)]
pub enum PolicyExecutionRefusal {
    /// The policy is gone, or the group it belonged to is.
    NoSuchPolicy,
    /// The policy's type states no adjustment to apply; carries that type.
    UnexecutablePolicyType(String),
    /// A member this policy's type needs is absent; carries the member as the service names
    /// it in that refusal, and the policy's type.
    MemberRequired {
        /// The member the request left out, as the service names it in the sentence.
        member: &'static str,
        /// The type of the policy that needs it.
        policy_type: String,
    },
    /// A member this policy's type does not use was supplied; carries the member as the
    /// model names it, and the policy's type.
    MemberNotSupported {
        /// The member the request carried.
        member: &'static str,
        /// The type of the policy it was sent for.
        policy_type: String,
    },
    /// No step of the policy covers the metric value the request stated.
    NoMatchingStep(MetricBreach),
    /// The policy states an adjustment the service cannot read: no `AdjustmentType`, or one
    /// outside [`super::ADJUSTMENT_TYPES`].
    UnreadableAdjustment,
    /// The policy tracks a target the service cannot read: no `TargetTrackingConfiguration`,
    /// or one whose `TargetValue` no capacity can be computed from.
    UnreadableTarget,
    /// The group still holds instances from an earlier scaling activity, so the launches
    /// this change needs do not fit beside them yet.
    InstancesStillTerminating,
    /// The request asked for the group's cooldown to be honoured and the group is still
    /// inside one, so this execution is blocked by the activity that started it.
    CooldownInProgress,
}

/// `MetricValue` as the service names it in the sentence that asks for it on a policy whose
/// type needs one: the member's own words rather than its wire path, because the rule this
/// refusal states is the policy TYPE's and not the request shape's.
pub const METRIC_VALUE_SUBJECT: &str = "Metric value";

/// `BreachThreshold` in the same form, for the same sentence.
pub const BREACH_THRESHOLD_SUBJECT: &str = "Breach threshold";

/// `MetricValue` as the model names it, for the sentence that refuses it on a policy whose
/// type does not use it.
pub const METRIC_VALUE_MEMBER: &str = "MetricValue";

/// `BreachThreshold` as the model names it (same sentence).
pub const BREACH_THRESHOLD_MEMBER: &str = "BreachThreshold";

/// The refusal of a policy name nothing in the account answers to.
///
/// The service's own absent-policy sentence: the resource word, then the name asked for,
/// then that nothing answers to it. The name — "The name or Amazon Resource Name (ARN) of
/// the policy" (`DeletePolicyType$PolicyName`), taken verbatim from the request — is its
/// only variable part, and the rest is fixed text.
#[must_use]
pub fn absent_policy_message(policy_name: &str) -> String {
    format!("Policy {policy_name} not found")
}

/// The refusal of a request that names a policy by its NAME and carries no
/// `AutoScalingGroupName` beside it.
///
/// A policy name is unique within a group rather than within the account — the member is
/// "the name or ARN of the policy" (`ExecutePolicyType$PolicyName`) while the group is
/// optional — so a bare name names no one policy until the group is given, and the request
/// is refused for the member it is MISSING rather than reported as a policy nothing answers
/// to. Entirely fixed text: neither the name asked for nor the group that was left out
/// appears in it, so the sentence reads the same for every such request.
pub const GROUP_REQUIRED_FOR_POLICY_NAME_MESSAGE: &str =
    "The AutoScaling Group name is required when specifying a policy by its name";

/// The ARN namespace of a scaling policy, the vendor field of the ARNs this service mints
/// for one ([`super::scaling_policies::policy_arn`]).
const POLICY_ARN_VENDOR: &str = "autoscaling";

/// The resource TYPE of that ARN, the first field of its resource half
/// (`scalingPolicy:{id}:autoScalingGroupName/{group}:policyName/{policy}`).
const POLICY_ARN_RESOURCE_TYPE: &str = "scalingPolicy";

/// Whether `policy_name_or_arn` names the policy by its ARN rather than by its NAME.
///
/// "The name or ARN of the policy" admits either spelling, and only the ARN form carries
/// the group inside it, so this is what decides whether an `AutoScalingGroupName` is still
/// needed to resolve the value ([`group_required_for_policy_name`]). An ARN is read with
/// this repository's one caller-supplied-ARN parse, and it has to be an ARN of a scaling
/// POLICY: a value that is not a well-formed resource ARN of this service, and one that
/// names another of its resources (a group, a scheduled action), carries no policy to
/// resolve and is read as a name.
#[must_use]
pub fn names_policy_by_arn(policy_name_or_arn: &str) -> bool {
    ::service_common::arn::is_resource_arn_of(policy_name_or_arn, POLICY_ARN_VENDOR)
        && ::service_common::arn::resource_type(policy_name_or_arn)
            == Some(POLICY_ARN_RESOURCE_TYPE)
}

/// Whether a request naming ONE policy still needs its `AutoScalingGroupName` to resolve
/// it: the value is a policy NAME and the request carries no group.
///
/// Asked BEFORE the policy is looked up, by every operation that acts on one policy the
/// request names (`ExecutePolicy`, `DeletePolicy`), so a well-formed-but-incomplete request
/// is refused for the member it leaves out
/// ([`GROUP_REQUIRED_FOR_POLICY_NAME_MESSAGE`]) rather than reported as a resource that does
/// not exist. A group stated as whitespace alone states none.
#[must_use]
pub fn group_required_for_policy_name(
    group_name: Option<&str>,
    policy_name_or_arn: &str,
) -> bool {
    !names_policy_by_arn(policy_name_or_arn)
        && group_name.is_none_or(|group| group.trim().is_empty())
}

/// The refusal of a member, or of the operation itself, for a policy whose type does not
/// use it: `subject` is what was asked for and `policy_type` the type that does not take
/// it.
///
/// One sentence for every "not supported otherwise" and "valid only if the policy type is"
/// rule of this operation, so the family is worded once.
#[must_use]
pub fn not_supported_for_policy_type_message(subject: &str, policy_type: &str) -> String {
    format!("{subject} is not supported for scaling policies of type {policy_type}.")
}

/// The refusal of the OPERATION for a policy whose type states no adjustment to perform:
/// the operation is not allowed for a policy of that type at all, which is a different
/// sentence from a member that type does not take
/// ([`not_supported_for_policy_type_message`]).
///
/// The stored policy's type is its only variable part.
#[must_use]
pub fn execute_policy_not_allowed_message(policy_type: &str) -> String {
    format!("ExecutePolicy is not allowed for policy type {policy_type}")
}

/// The refusal of a request that leaves out a member the policy's TYPE needs: "Required if
/// the policy type is StepScaling and not supported otherwise"
/// (`ExecutePolicyType$MetricValue`, `$BreachThreshold`).
///
/// The rule is the policy type's rather than the request shape's — the model declares both
/// members optional — so the sentence names the member in the service's own words and the
/// type that requires it, instead of the model-level null-member constraint sentence
/// ([`super::scaling_policies::member_must_not_be_null_message`]). Both parts are variable:
/// the member asked for and the type of the stored policy the request named.
#[must_use]
pub fn member_required_for_policy_type_message(member: &str, policy_type: &str) -> String {
    format!("{member} must be specified for policy type {policy_type}")
}

/// The sentence an execution blocked by the group's running cooldown is refused with: the
/// scaling activity that started the cooldown is what blocks this one. Entirely fixed text
/// — it names neither the group, nor the policy, nor the cooldown's length.
pub const COOLDOWN_IN_PROGRESS_MESSAGE: &str =
    "Scaling activity is in progress and blocks the execution of this policy";

/// The refusal of a metric value no step of the policy covers: "If you specify a metric
/// value that doesn't correspond to a step adjustment for the policy, the call returns an
/// error" (`ExecutePolicyType$MetricValue`).
///
/// The two values come from the request; the rest of the sentence is fixed text.
#[must_use]
pub fn no_matching_step_message(breach: MetricBreach) -> String {
    format!(
        "No StepAdjustment of the policy matches the metric value {} for the breach threshold \
         {}.",
        breach.metric_value, breach.breach_threshold
    )
}

/// The sentence this operation refuses an unreadable adjustment with: the policy carries no
/// `AdjustmentType`, so there is no way to read the number it scales by.
pub const UNREADABLE_ADJUSTMENT_MESSAGE: &str =
    "The scaling policy states no AdjustmentType, so its scaling adjustment cannot be applied.";

/// The sentence an unreadable target is refused with: the policy carries no target value, so
/// there is no capacity for the metric it tracks to be divided down to.
pub const UNREADABLE_TARGET_MESSAGE: &str =
    "The scaling policy states no TargetValue, so the capacity it tracks cannot be computed.";

impl PolicyExecutionRefusal {
    /// The sentence this refusal carries, where `policy_name` is the policy the request
    /// named and `group_name` the group it belongs to.
    #[must_use]
    pub fn message(&self, policy_name: &str, group_name: &str) -> String {
        match self {
            Self::NoSuchPolicy => absent_policy_message(policy_name),
            Self::UnexecutablePolicyType(policy_type) => {
                execute_policy_not_allowed_message(policy_type)
            }
            Self::MemberRequired {
                member,
                policy_type,
            } => member_required_for_policy_type_message(member, policy_type),
            Self::MemberNotSupported {
                member,
                policy_type,
            } => not_supported_for_policy_type_message(member, policy_type),
            Self::NoMatchingStep(breach) => no_matching_step_message(*breach),
            Self::UnreadableAdjustment => UNREADABLE_ADJUSTMENT_MESSAGE.to_string(),
            Self::UnreadableTarget => UNREADABLE_TARGET_MESSAGE.to_string(),
            Self::InstancesStillTerminating => {
                super::instances_still_terminating_message(group_name)
            }
            Self::CooldownInProgress => COOLDOWN_IN_PROGRESS_MESSAGE.to_string(),
        }
    }
}

/// The policy a request names: "The name or ARN of the policy"
/// (`ExecutePolicyType$PolicyName`), resolved against what the store holds; `None` when
/// nothing the request may reach answers to it.
///
/// An ARN names the policy outright, wherever it lives. A plain NAME is unique only within
/// a group, so it is resolved inside the group the request names — which such a request
/// always carries, because a bare name with no group is refused for the missing member
/// before the lookup ([`group_required_for_policy_name`]). A group the request names that
/// holds no policy of that name resolves nothing, whatever other groups hold.
pub fn resolve_policy(
    state: &AutoScalingState,
    group_name: Option<&str>,
    policy_name_or_arn: &str,
) -> Option<PolicyKey> {
    let in_group = |key: &PolicyKey| {
        group_name.is_none_or(|group| key.auto_scaling_group_name == group)
    };
    state
        .scaling_policies
        .iter()
        .find(|(key, policy)| in_group(key) && policy.policy_arn == policy_name_or_arn)
        .map(|(key, _)| key.clone())
        .or_else(|| {
            state
                .scaling_policies
                .keys()
                .find(|key| in_group(key) && key.policy_name == policy_name_or_arn)
                .cloned()
        })
}

impl AutoScalingState {
    /// Perform the scaling policy `key` names, and answer the capacity it moved the group
    /// between.
    ///
    /// THE ONE place a policy is performed: the capacity it asks for — the adjustment read
    /// off the policy, or the ratio a target-tracking policy's own alarm implies
    /// ([`tracked_capacity`]) — is applied to the capacity the group maintains, held inside
    /// the group's own bounds and then
    /// converged on through [`AutoScalingState::apply_capacity`], so a policy-driven change
    /// launches and terminates the account's own Amazon EC2 instances and records its
    /// activities exactly as a manual capacity change does. The activity's cause names the
    /// policy that asked for it ([`policy_execution_cause`]).
    ///
    /// Nothing is written when the policy cannot be performed: every rule the request and
    /// the policy are taken against is checked before the group is touched.
    pub fn execute_scaling_policy<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        key: &PolicyKey,
        request: PolicyExecutionRequest,
        trigger: PolicyTrigger<'_>,
        now: SystemTime,
    ) -> Result<PolicyExecution, PolicyExecutionRefusal> {
        let Some(policy) = self.scaling_policies.get(key).cloned() else {
            return Err(PolicyExecutionRefusal::NoSuchPolicy);
        };
        let Some(group) = self.groups.get(&key.auto_scaling_group_name) else {
            return Err(PolicyExecutionRefusal::NoSuchPolicy);
        };
        let policy_type = policy.policy_type.clone();
        let from = group.desired_capacity;
        // A policy that tracks a target states no adjustment at all, so the capacity it asks
        // for is computed from the metric its own alarms graded against that target; every
        // other type reads an adjustment off the policy and applies it to the capacity the
        // group maintains.
        let asked = if tracks_a_target(&policy_type) && trigger.is_alarm() {
            tracked_capacity_asked(&policy, from, request.metric_value)?
        } else {
            if !is_executable_policy_type(&policy_type) {
                return Err(PolicyExecutionRefusal::UnexecutablePolicyType(policy_type));
            }
            let breach = metric_breach(&policy_type, request)?;

            // The adjustment the policy states for this request: its own for a simple policy,
            // and the step the breach falls in for a step policy.
            let adjustment = match breach {
                Some(breach) => applicable_step_adjustment(&policy.step_adjustments, breach)
                    .ok_or(PolicyExecutionRefusal::NoMatchingStep(breach))?,
                None => policy
                    .scaling_adjustment
                    .ok_or(PolicyExecutionRefusal::UnreadableAdjustment)?,
            };
            let adjustment_type = policy
                .adjustment_type
                .as_deref()
                .ok_or(PolicyExecutionRefusal::UnreadableAdjustment)?;
            adjusted_capacity(
                adjustment_type,
                adjustment,
                policy.min_adjustment_magnitude.or(policy.min_adjustment_step),
                from,
            )
            .ok_or(PolicyExecutionRefusal::UnreadableAdjustment)?
        };
        // A group never holds a capacity outside the bounds it was configured with, so a
        // policy that asks for more than its `MaxSize` or less than its `MinSize` moves it
        // to the bound rather than being refused. The two bounds are read LOWEST FIRST:
        // nothing on this path establishes their order, and a capacity change must not be
        // the request that panics on a group whose stored sizes are inverted.
        let (floor, ceiling) = if group.min_size <= group.max_size {
            (group.min_size, group.max_size)
        } else {
            (group.max_size, group.min_size)
        };
        let to = asked.clamp(floor, ceiling);

        // Instances from an earlier change are stored until they finish terminating, and
        // they occupy the group's ceiling while they do: a change whose launches would not
        // fit beside them cannot be started yet.
        if group.launches_needed(to) > group.launch_headroom() {
            return Err(PolicyExecutionRefusal::InstancesStillTerminating);
        }
        // The cooldown period in force for THIS execution: the policy's own when it states one,
        // which "overrides the default cooldown" (`PutScalingPolicyType$Cooldown`), and the
        // group's `DefaultCooldown` otherwise — one span, measured from the activity the group's
        // cooldown follows, for what the execution waits out as well as for what it leaves
        // behind ([`AutoScalingState::override_cooldown`] below).
        let in_cooldown = group.in_cooldown_of_span(policy.cooldown, now);

        // "Valid only if the policy type is SimpleScaling": only a simple policy honours the
        // cooldown, and a request that asks for it while one is still running is
        // BLOCKED by the scaling activity that started that cooldown — the change is not
        // applied now and not kept for later, so nothing of the group is touched.
        if request.honor_cooldown && policy_type == POLICY_TYPE_SIMPLE_SCALING && in_cooldown {
            return Err(PolicyExecutionRefusal::CooldownInProgress);
        }
        let cause = trigger.cause(now, &key.policy_name, from, to);

        self.apply_capacity(engine, &key.auto_scaling_group_name, to, &cause, now);
        // The cooldown the change leaves behind is the POLICY's when it carries one, which
        // "overrides the default cooldown period defined for the Auto Scaling group" the
        // convergence above started — through the one writer of that override.
        self.override_cooldown(&key.auto_scaling_group_name, policy.cooldown, now);
        Ok(PolicyExecution { from, to })
    }
}

/// The breach a policy of `policy_type` is graded by, read off the request.
///
/// `None` for a policy whose type carries its own adjustment. The two metric members are
/// "Required if the policy type is StepScaling and not supported otherwise", so each is
/// refused where it does not belong and missed where it does.
fn metric_breach(
    policy_type: &str,
    request: PolicyExecutionRequest,
) -> Result<Option<MetricBreach>, PolicyExecutionRefusal> {
    if !grades_a_metric_value(policy_type) {
        for (value, member) in [
            (request.metric_value, METRIC_VALUE_MEMBER),
            (request.breach_threshold, BREACH_THRESHOLD_MEMBER),
        ] {
            if value.is_some() {
                return Err(PolicyExecutionRefusal::MemberNotSupported {
                    member,
                    policy_type: policy_type.to_string(),
                });
            }
        }
        return Ok(None);
    }
    let Some(metric_value) = request.metric_value else {
        return Err(PolicyExecutionRefusal::MemberRequired {
            member: METRIC_VALUE_SUBJECT,
            policy_type: policy_type.to_string(),
        });
    };
    let Some(breach_threshold) = request.breach_threshold else {
        return Err(PolicyExecutionRefusal::MemberRequired {
            member: BREACH_THRESHOLD_SUBJECT,
            policy_type: policy_type.to_string(),
        });
    };
    Ok(Some(MetricBreach {
        metric_value,
        breach_threshold,
    }))
}

/// The capacity a target-tracking `policy` asks for from `from`, for the `metric_value` one
/// of its own alarms graded.
///
/// The policy's `TargetValue` is where the metric is to sit, so the capacity asked for is
/// [`tracked_capacity`] of the two — never below the capacity the group holds for a policy
/// whose `DisableScaleIn` bars it from removing an instance.
fn tracked_capacity_asked(
    policy: &StoredScalingPolicy,
    from: i32,
    metric_value: Option<f64>,
) -> Result<i32, PolicyExecutionRefusal> {
    let Some(metric_value) = metric_value else {
        return Err(PolicyExecutionRefusal::MemberRequired {
            member: METRIC_VALUE_SUBJECT,
            policy_type: policy.policy_type.clone(),
        });
    };
    let configuration = policy
        .target_tracking_configuration
        .as_ref()
        .ok_or(PolicyExecutionRefusal::UnreadableTarget)?;
    let asked = tracked_capacity(from, metric_value, configuration.target_value)
        .ok_or(PolicyExecutionRefusal::UnreadableTarget)?;
    let scale_in_disabled = configuration
        .disable_scale_in
        .unwrap_or(DEFAULT_DISABLE_SCALE_IN);
    Ok(if scale_in_disabled {
        asked.max(from)
    } else {
        asked
    })
}

/// The capacity at which a metric now reading `metric_value` over `current` units of
/// capacity sits at or below `target_value`.
///
/// The target is stated per unit of the capacity the group maintains, so the load the metric
/// reports over that capacity is `current × metric_value`, and the capacity that divides it
/// down to the target is `current × metric_value ÷ target_value` — taken UP to the next
/// whole instance, because a capacity below that quotient leaves the metric above the
/// target. Scaling in follows from the same quotient: a metric under the target divides down
/// to fewer instances than the group holds.
///
/// A group maintaining no capacity is scaled by no ratio of itself — there is nothing for the
/// metric to be divided over — so it stays where it is. `None` when the quotient cannot be
/// taken at all: a target of zero or a value that is not a finite number states no capacity.
#[must_use]
pub fn tracked_capacity(current: i32, metric_value: f64, target_value: f64) -> Option<i32> {
    if !metric_value.is_finite() || !target_value.is_finite() || target_value <= 0.0 {
        return None;
    }
    if current <= 0 {
        return Some(current);
    }
    let wanted = (f64::from(current) * metric_value / target_value).ceil();
    if !wanted.is_finite() {
        return None;
    }
    Some(wanted.clamp(0.0, f64::from(i32::MAX)) as i32)
}

/// The `ScalingAdjustment` of the step whose interval the breach falls in.
///
/// A step's interval is of the DIFFERENCE between the metric and the breach threshold, and
/// which end is inclusive follows the direction of the breach: above the threshold "the
/// lower bound is inclusive (the metric must be greater than or equal to the threshold plus
/// the lower bound)" and the upper bound is exclusive
/// (`StepAdjustment$MetricIntervalLowerBound`); below it the lower bound is exclusive and
/// the upper bound inclusive. A bound the step leaves out is unbounded in that direction,
/// and so is one that is not a finite number — nothing can be compared against it.
#[must_use]
pub fn applicable_step_adjustment(
    steps: &[crate::ops::types::StepAdjustment],
    breach: MetricBreach,
) -> Option<i32> {
    let finite = |bound: Option<f64>| bound.filter(|value| value.is_finite());
    let difference = breach.difference();
    steps
        .iter()
        .find(|step| {
            let lower = finite(step.metric_interval_lower_bound).unwrap_or(f64::NEG_INFINITY);
            let upper = finite(step.metric_interval_upper_bound).unwrap_or(f64::INFINITY);
            if breach.above() {
                difference >= lower && difference < upper
            } else {
                difference > lower && difference <= upper
            }
        })
        .map(|step| step.scaling_adjustment)
}

/// The capacity an adjustment asks for, from the capacity the group maintains now.
///
/// Each arm is one sentence of the service's own documentation of the adjustment types
/// ([`super::ADJUSTMENT_TYPES`]): `ChangeInCapacity` adds to the current capacity
/// ("a positive value adds to the current capacity while a negative number removes from
/// the current capacity", `StepAdjustment$ScalingAdjustment`), `ExactCapacity` IS the
/// capacity, and `PercentChangeInCapacity` moves it by a percentage of itself
/// ([`percentage_magnitude`]). `None` for an adjustment type outside that catalog, which
/// states no arithmetic at all.
#[must_use]
pub fn adjusted_capacity(
    adjustment_type: &str,
    adjustment: i32,
    min_adjustment_magnitude: Option<i32>,
    current: i32,
) -> Option<i32> {
    let capacity = match adjustment_type {
        super::ADJUSTMENT_TYPE_CHANGE_IN_CAPACITY => i64::from(current) + i64::from(adjustment),
        super::ADJUSTMENT_TYPE_EXACT_CAPACITY => i64::from(adjustment),
        super::ADJUSTMENT_TYPE_PERCENT_CHANGE_IN_CAPACITY => {
            i64::from(current)
                + percentage_magnitude(current, adjustment, min_adjustment_magnitude)
        }
        _ => return None,
    };
    Some(capacity.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
}

/// How many instances a `PercentChangeInCapacity` adjustment moves the capacity by.
///
/// The percentage is OF the capacity the group maintains, so a percentage of NOTHING is
/// nothing: a group maintaining no instance is not scaled out by a share of itself. For every
/// other capacity the number of instances the percentage comes to is a fraction: it is taken
/// toward zero, and never to nothing — a non-zero percentage of a non-zero capacity moves it
/// by at least [`DEFAULT_MIN_ADJUSTMENT_MAGNITUDE`] instance in the direction it names.
/// `MinAdjustmentMagnitude` raises that floor: "The minimum value to scale by when the
/// adjustment type is PercentChangeInCapacity. For example, suppose that you create a step
/// scaling policy to scale out an Auto Scaling group by 25 percent and you specify a
/// MinAdjustmentMagnitude of 2. If the group has 4 instances and the scaling policy is
/// performed, 25 percent of 4 is 1. However, because you specified a MinAdjustmentMagnitude
/// of 2, Amazon EC2 Auto Scaling scales out the group by 2 instances"
/// (`PutScalingPolicyType$MinAdjustmentMagnitude`).
#[must_use]
pub fn percentage_magnitude(
    current: i32,
    adjustment: i32,
    min_adjustment_magnitude: Option<i32>,
) -> i64 {
    if adjustment == 0 || current == 0 {
        return 0;
    }
    let exact = f64::from(current) * f64::from(adjustment) / 100.0;
    let floor = i64::from(
        min_adjustment_magnitude
            .unwrap_or(DEFAULT_MIN_ADJUSTMENT_MAGNITUDE)
            .max(DEFAULT_MIN_ADJUSTMENT_MAGNITUDE),
    );
    let magnitude = (exact.abs().trunc() as i64).max(floor);
    magnitude * i64::from(adjustment.signum())
}

/// The cause a policy-driven capacity change records: that a policy asked for it, which
/// policy, and the capacities it moved between.
///
/// The subject of [`AutoScalingState::capacity_change_cause`], as every other capacity
/// change of this service words its own.
#[must_use]
pub fn policy_execution_cause(at: SystemTime, policy_name: &str, from: i32, to: i32) -> String {
    AutoScalingState::capacity_change_cause(
        at,
        &format!("a user request executed policy {policy_name}"),
        from,
        to,
    )
}

/// The cause a capacity change an ALARM's action drove records: that a monitor alarm in a
/// state triggered the policy, which alarm and which policy, and the capacities it moved
/// between.
///
/// The same subject form as every other capacity change of this service
/// ([`AutoScalingState::capacity_change_cause`]), naming the alarm and the state it was in
/// instead of the request that asked — which is what tells a client reading the activity log
/// that this change was driven by the alarm it configured.
#[must_use]
pub fn alarm_execution_cause(
    at: SystemTime,
    alarm_name: &str,
    state_value: &str,
    policy_name: &str,
    from: i32,
    to: i32,
) -> String {
    AutoScalingState::capacity_change_cause(
        at,
        &format!(
            "a monitor alarm {alarm_name} in state {state_value} triggered policy {policy_name}"
        ),
        from,
        to,
    )
}
