// SPDX-License-Identifier: CC0-1.0

//! The scaling policies a CloudWatch ALARM drives: the notifications this service receives
//! from the alarms whose actions name its policies, and what receiving one does.
//!
//! # The rule this module implements
//!
//! A scaling policy is what a CloudWatch alarm acts THROUGH: an alarm whose action list names
//! a policy's ARN hands the transition to this service when it moves, and this service then
//! performs that policy — including a `TargetTrackingScaling` policy, which is performed by
//! the alarms it owns and by nothing else, from the metric value they graded against the
//! target it states ([`super::policy_execution::tracked_capacity`]) — the same performance
//! `ExecutePolicy` makes
//! ([`super::policy_execution`]), so the adjustment, the group's own `MinSize`..`MaxSize`
//! bounds and the scaling activity it records are the same whichever asked for it. The
//! activity names the alarm that drove it ([`super::policy_execution::alarm_execution_cause`]),
//! which is how a client reading the activity log tells the two apart.
//!
//! Three things decide whether a received notification is performed at all:
//!
//! * `AlarmNotification` — the scaling process that RECEIVES these notifications
//!   ([`super::suspended_processes::PROCESS_ALARM_NOTIFICATION`]). A group that has suspended
//!   it does not do what that process does, so the notification is declined and the group's
//!   capacity is left exactly where it is. A policy the same group's own `ExecutePolicy`
//!   request performs is a different process and is not affected.
//! * `Enabled` — "Indicates whether the scaling policy is enabled or disabled"
//!   (`PutScalingPolicyType$Enabled`): a disabled policy is not performed by the alarm that
//!   names it.
//! * the COOLDOWN — the period that follows the group's last capacity change. An
//!   alarm-driven execution waits for it ("Amazon EC2 Auto Scaling waits for the cooldown
//!   period to complete before executing the policy", `ExecutePolicyType$HonorCooldown`,
//!   which is the behaviour of a scaling policy driven by an alarm rather than by a manual
//!   request), so an alarm that keeps firing inside one window moves the capacity once. WHICH
//!   period that is belongs to the policy being performed: its own `Cooldown` overrides the
//!   group's default ([`super::policy_execution`]), so a policy that states a cooldown of zero
//!   is performed as soon as the alarm hands it over.
//!
//! # Where the notification comes from
//!
//! The alarms belong to CloudWatch, so the hand-over is taken out of the account's OWN
//! CloudWatch store through the shared engine — the same store `PutMetricAlarm` writes and
//! whose action execution queues it
//! ([`cloudwatch::state::alarm_actions::scaling_policy_actions`]). Only the hand-overs naming
//! a policy THIS store holds are taken; every other one is left queued for the service that
//! owns it, and a policy this store no longer holds leaves nothing behind.
//!
//! LOCKING. This service's own store is locked by the handler; CloudWatch's is taken here for
//! the one pass that takes the hand-overs and released before any policy is performed, and
//! nothing awaits while either is held. CloudWatch never reaches this service, so the two
//! locks are only ever taken in this order.

use std::time::SystemTime;

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

use super::policy_execution::{
    grades_a_metric_value, tracks_a_target, PolicyExecutionRequest, PolicyTrigger,
};
use super::suspended_processes::PROCESS_ALARM_NOTIFICATION;
use super::{AutoScalingState, PolicyKey};

impl AutoScalingState {
    /// Receive every alarm notification this store's policies have been handed and perform
    /// the policies that are to be performed, as of `now`.
    ///
    /// Owed on the clock like every other change this store materialises: the alarm moved
    /// whether or not anybody called, so the policy its action named was to be performed at
    /// that moment and is performed by the next pass that reaches it.
    pub fn receive_alarm_notifications<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        now: SystemTime,
    ) {
        let Some(alarms) = cloudwatch::state::service_state(engine) else {
            // No CloudWatch store on this engine holds an alarm, so none has fired an action
            // at a policy of this store.
            return;
        };
        let received = {
            let mut store = alarms.lock();
            cloudwatch::state::alarm_actions::scaling_policy_actions::take_invocations(
                &mut store,
                |policy_arn| {
                    self.scaling_policies
                        .values()
                        .any(|policy| policy.policy_arn == policy_arn)
                },
            )
        };

        for notification in received {
            let Some(key) = self.policy_of_arn(&notification.policy_arn) else {
                // The policy was taken as this store's and then deleted: there is nothing
                // left to perform.
                continue;
            };
            let Some((policy_type, enabled)) = self
                .scaling_policies
                .get(&key)
                .map(|policy| (policy.policy_type.clone(), policy.enabled))
            else {
                continue;
            };
            if !enabled {
                ::tracing::debug!(
                    alarm = %notification.alarm_name,
                    policy = %key.policy_name,
                    "the alarm fired a scaling policy that is disabled, so nothing is performed"
                );
                continue;
            }
            if self
                .groups
                .get(&key.auto_scaling_group_name)
                .is_some_and(|group| group.is_process_suspended(PROCESS_ALARM_NOTIFICATION))
            {
                ::tracing::debug!(
                    alarm = %notification.alarm_name,
                    policy = %key.policy_name,
                    group = %key.auto_scaling_group_name,
                    invoked_at = notification.invoked_at_secs,
                    "the group has suspended the process that receives alarm notifications, so \
                     this one is declined"
                );
                continue;
            }
            // The breach belongs to the policy types that are graded by one, and is "not
            // supported otherwise" (`ExecutePolicyType$MetricValue`): a simple policy carries
            // its own adjustment and is performed without either number.
            let graded = grades_a_metric_value(&policy_type);
            // A policy that tracks a TARGET is performed from the value its own alarm graded
            // as well, and reads what that value is to be off itself rather than off the
            // alarm's threshold ([`super::policy_execution::tracked_capacity`]).
            let tracks = tracks_a_target(&policy_type);
            let request = PolicyExecutionRequest {
                honor_cooldown: true,
                metric_value: (graded || tracks)
                    .then_some(notification.metric_value)
                    .flatten(),
                breach_threshold: graded.then_some(notification.breach_threshold).flatten(),
            };
            let trigger = PolicyTrigger::Alarm {
                alarm_name: &notification.alarm_name,
                state_value: &notification.state_value,
            };
            if let Err(refusal) = self.execute_scaling_policy(engine, &key, request, trigger, now)
            {
                // An alarm-driven execution has no caller to answer: what blocks it (the
                // group's cooldown, a policy type that states no adjustment of its own, a
                // breach no step covers) leaves the group where it is and is recorded
                // nowhere else, so it is logged.
                ::tracing::debug!(
                    alarm = %notification.alarm_name,
                    policy = %key.policy_name,
                    group = %key.auto_scaling_group_name,
                    refusal = %refusal.message(&key.policy_name, &key.auto_scaling_group_name),
                    "the policy this alarm fired was not performed"
                );
            }
        }
    }

    /// The policy of this store whose ARN is `policy_arn`.
    ///
    /// An ARN names one policy wherever it lives, which is what a hand-over carries: a policy
    /// NAME is unique only inside its group ([`super::policy_execution::resolve_policy`]).
    fn policy_of_arn(&self, policy_arn: &str) -> Option<PolicyKey> {
        self.scaling_policies
            .iter()
            .find(|(_, policy)| policy.policy_arn == policy_arn)
            .map(|(key, _)| key.clone())
    }
}
