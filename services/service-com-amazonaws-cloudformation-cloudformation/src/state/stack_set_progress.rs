// SPDX-License-Identifier: CC0-1.0

//! The PROGRESS a stack-set operation reports: the stages it runs through, the targets it
//! resolved and how far each Region has got.
//!
//! ## The rule
//!
//! A stack-set operation reports its progress as STAGES — "the role provisioning stage and
//! the stack instances deployment stage" — beside the totals it resolved: `TotalAccountsCount`
//! and `TotalStackInstancesCount` are populated "after it resolves the target accounts" and
//! "after it resolves the target stack instances. Until then, this field is not present", so
//! an operation that has resolved no target reports neither total and no stage detail at all.
//!
//! Everything reported here is COUNTED off the operation's own results — one per account and
//! Region the operation acted on ([`StoredStackSet::operation_results`]) — so this detail,
//! `ListStackSetOperationResults` and the instances themselves cannot disagree about what the
//! operation did. Nothing is stored for it: a count that is derived is derived once, where it
//! is read.
//!
//! ## The two stages
//!
//! * ROLE PROVISIONING is provisioning the execution role in each target account. This
//!   emulator provisions no role in a target account — a stack set carries no custom execution
//!   role template here — and the stage's documented reading for exactly that case is
//!   "operations that do not provision roles: this stage is present with a `Status` of
//!   `NOT_APPLICABLE`", with its per-account counts omitted. So the stage is reported, and
//!   reports that it does not apply, rather than inventing accounts it provisioned.
//! * STACK INSTANCES DEPLOYMENT is the deployment of the operation's targets, which this
//!   store does hold: each target's result status says whether it succeeded, failed, was
//!   canceled or has yet to finish ([`Progress`]). An operation that deploys no instance at
//!   all — a drift detection, "such as a drift detection operation" being the documented
//!   `NOT_APPLICABLE` case — reports the stage as not applicable too.
//!
//! `SkippedStackInstancesCount` is "the number of stack instances that were skipped because
//! role provisioning failed for their target account": nothing here provisions roles, so no
//! target is skipped for that reason and the count is zero rather than absent.
//!
//! ## Per Region
//!
//! `StackSetOperation$RegionalStatusDetails` is "the progress detail for each Region of the
//! operation, keyed by Region", and "this field is returned only by DescribeStackSetOperation.
//! It is not included in the ListStackSetOperations summary" — so the map is built here and
//! the summary read leaves it out. Each entry is the same deployment progress, counted over
//! the results of that one Region, plus that Region's own target total.

use ::std::collections::BTreeMap;

use super::stack_sets::{
    operation_action, operation_status, result_status, StoredOperationResult, StoredStackSet,
    StoredStackSetOperation,
};
use crate::ops::types::{
    RegionalStackInstancesDeployment, RegionalStages, RegionalStatusDetail, RoleProvisioning,
    StackInstancesDeployment, StackSetOperationStatusDetails, Stages,
};

/// The model's `StageStatus`: the status one stage of a stack-set operation is in
/// (`smithy_query(query="shape", shape="StageStatus")`).
pub mod stage_status {
    /// "NOT_APPLICABLE: The operation does not provision roles" / "does not deploy stack
    /// instances, such as a drift detection operation."
    pub const NOT_APPLICABLE: &str = "NOT_APPLICABLE";
    /// "NOT_STARTED: … The operation is still provisioning roles, has not begun, or failed or
    /// stopped before this stage was reached."
    pub const NOT_STARTED: &str = "NOT_STARTED";
    /// "IN_PROGRESS: At least one stack instance has started deploying."
    pub const IN_PROGRESS: &str = "IN_PROGRESS";
    /// "SUCCEEDED: Deployment finished and succeeded for all stack instances that were not
    /// skipped."
    pub const SUCCEEDED: &str = "SUCCEEDED";
    /// "PARTIAL_SUCCESS: Deployment finished with a mix of succeeded, failed, and canceled
    /// stack instances."
    pub const PARTIAL_SUCCESS: &str = "PARTIAL_SUCCESS";
    /// "FAILED: Deployment finished and every stack instance that was not skipped failed or
    /// was canceled."
    pub const FAILED: &str = "FAILED";
}

/// How far a set of an operation's results has got: one count per outcome, as the stage's own
/// members report them.
///
/// Counted over the results handed in, so the same type answers the operation as a whole and
/// one Region of it — the members mean the same thing either way.
#[derive(Debug, Clone, Copy, Default)]
pub struct Progress {
    /// Every target counted: `TotalStackInstancesCount`.
    pub total: usize,
    /// "The number of stack instances that deployed successfully."
    pub succeeded: usize,
    /// "The number of stack instances that failed to deploy."
    pub failed: usize,
    /// "The number of stack instances that were canceled because the operation exceeded its
    /// failure tolerance or was stopped."
    pub cancelled: usize,
    /// "The number of stack instances that were skipped because role provisioning failed for
    /// their target account" — nothing here provisions roles, so nothing is skipped for it.
    pub skipped: usize,
    /// Whether the operation itself has concluded, which is what decides a stage that has
    /// nothing left to do from one that has not started.
    pub concluded: bool,
}

impl Progress {
    /// The progress `results` show, for an operation whose own status says whether it has
    /// concluded.
    pub fn of(results: &[StoredOperationResult], concluded: bool) -> Self {
        let count = |wanted: &str| {
            results
                .iter()
                .filter(|result| result.status == wanted)
                .count()
        };
        Self {
            total: results.len(),
            succeeded: count(result_status::SUCCEEDED),
            failed: count(result_status::FAILED),
            cancelled: count(result_status::CANCELLED),
            skipped: 0,
            concluded,
        }
    }

    /// "The number of stack instances that have reached a final state. This count includes
    /// stack instances that succeeded, failed, were skipped, or were canceled."
    pub fn completed(&self) -> usize {
        self.succeeded + self.failed + self.cancelled + self.skipped
    }

    /// "The number of stack instances that have not reached a final state. This is the total
    /// number of stack instances minus the completed count."
    pub fn pending(&self) -> usize {
        self.total.saturating_sub(self.completed())
    }

    /// Whether every target has reached a final state — which, for an operation with no
    /// target at all, is true exactly when the operation itself has concluded.
    fn finished(&self) -> bool {
        if self.total == 0 {
            return self.concluded;
        }
        self.completed() == self.total
    }

    /// "The percentage of stack instances that have reached a final state."
    pub fn percentage_completed(&self) -> f64 {
        if self.total == 0 {
            return if self.finished() { 100.0 } else { 0.0 };
        }
        100.0 * self.completed() as f64 / self.total as f64
    }

    /// The [`stage_status`] the deployment stage is in, for an operation that DEPLOYS stack
    /// instances at all.
    ///
    /// Read off the counts, in the order the values are documented in: a stage with nothing
    /// left to do reports what it ended as — everything that was not skipped succeeded, a
    /// mix, or none of it — and one with work outstanding reports whether any of it has
    /// started.
    pub fn stage_status(&self) -> &'static str {
        if !self.finished() {
            return if self.completed() == 0 {
                stage_status::NOT_STARTED
            } else {
                stage_status::IN_PROGRESS
            };
        }
        if self.failed + self.cancelled == 0 {
            stage_status::SUCCEEDED
        } else if self.succeeded > 0 {
            stage_status::PARTIAL_SUCCESS
        } else {
            stage_status::FAILED
        }
    }

    /// `i32` is what every count member is modelled as, and a stack set holds far fewer
    /// instances than `i32::MAX` ([`super::stack_sets::MAX_INSTANCES_PER_STACK_SET`] bounds
    /// them), so the saturating conversion is unreachable rather than a silently wrong count.
    fn counted(value: usize) -> Option<i32> {
        Some(i32::try_from(value).unwrap_or(i32::MAX))
    }
}

/// Whether an operation of this ACTION deploys stack instances at all.
///
/// `NOT_APPLICABLE` is "the operation does not deploy stack instances, such as a drift
/// detection operation": a detection reads the instances it covers and deploys none, while
/// every other action creates, updates or deletes them.
fn deploys_stack_instances(action: &str) -> bool {
    action != operation_action::DETECT_DRIFT
}

/// The deployment stage of one operation, as the modelled `StackInstancesDeployment`.
fn stack_instances_deployment(action: &str, progress: &Progress) -> StackInstancesDeployment {
    if !deploys_stack_instances(action) {
        // The stage is PRESENT and reports that it does not apply; its per-instance counts
        // are omitted, since nothing was deployed to count.
        return StackInstancesDeployment {
            status: Some(stage_status::NOT_APPLICABLE.to_string()),
            ..StackInstancesDeployment::default()
        };
    }
    StackInstancesDeployment {
        status: Some(progress.stage_status().to_string()),
        succeeded_stack_instances_count: Progress::counted(progress.succeeded),
        failed_stack_instances_count: Progress::counted(progress.failed),
        completed_stack_instances_count: Progress::counted(progress.completed()),
        pending_stack_instances_count: Progress::counted(progress.pending()),
        cancelled_stack_instances_count: Progress::counted(progress.cancelled),
        skipped_stack_instances_count: Progress::counted(progress.skipped),
        percentage_completed: Some(progress.percentage_completed()),
    }
}

/// The two stages of one operation, as the modelled `Stages`.
fn stages(action: &str, progress: &Progress) -> Stages {
    Stages {
        // "For operations that do not provision roles, this stage is present with a Status of
        // NOT_APPLICABLE": no stack set here carries a custom execution role template, so no
        // operation of it provisions a role in a target account, and the per-account counts
        // are omitted rather than reported as zero accounts provisioned.
        role_provisioning: Some(RoleProvisioning {
            status: Some(stage_status::NOT_APPLICABLE.to_string()),
            ..RoleProvisioning::default()
        }),
        stack_instances_deployment: Some(stack_instances_deployment(action, progress)),
    }
}

/// How many distinct accounts the operation's results cover: `TotalAccountsCount`, "the total
/// number of accounts targeted by this StackSet operation".
fn targeted_accounts(results: &[StoredOperationResult]) -> usize {
    results
        .iter()
        .map(|result| result.account.as_str())
        .collect::<::std::collections::BTreeSet<&str>>()
        .len()
}

/// One operation's `StatusDetails`, or `None` for an operation this store holds no detail of
/// at all.
///
/// The totals and the stages are reported once the operation has RESOLVED its targets, which
/// is what the two totals' own documentation conditions them on ("CloudFormation populates
/// this value after it resolves the target accounts / the target stack instances. Until then,
/// this field is not present"); the failed-instance count is the one the operation's
/// conclusion recorded, so an operation that concluded reports it whether or not it had
/// targets.
pub fn status_details(
    stack_set: &StoredStackSet,
    operation: &StoredStackSetOperation,
) -> Option<StackSetOperationStatusDetails> {
    let results = stack_set.operation_results(operation);
    let resolved = !results.is_empty();
    if !resolved && operation.failed_stack_instances_count.is_none() {
        return None;
    }
    let progress = Progress::of(&results, !operation_status::is_in_flight(&operation.status));
    Some(StackSetOperationStatusDetails {
        stages: resolved.then(|| stages(&operation.action, &progress)),
        total_accounts_count: resolved
            .then(|| Progress::counted(targeted_accounts(&results)))
            .flatten(),
        total_stack_instances_count: resolved
            .then(|| Progress::counted(progress.total))
            .flatten(),
        // "The number of stack instances for which the StackSet operation failed", as the
        // operation's own record carries it — absent for one whose details the service holds
        // none of.
        failed_stack_instances_count: operation.failed_stack_instances_count,
    })
}

/// One operation's `RegionalStatusDetails`: the deployment progress of each Region it acted
/// in, keyed by Region.
///
/// `None` while the operation has resolved no target, for the same reason the totals are
/// absent then. The map is ordered by Region, so a response is stable across reads of the
/// same operation.
pub fn regional_status_details(
    stack_set: &StoredStackSet,
    operation: &StoredStackSetOperation,
) -> Option<::std::collections::HashMap<String, RegionalStatusDetail>> {
    let results = stack_set.operation_results(operation);
    if results.is_empty() {
        return None;
    }
    let concluded = !operation_status::is_in_flight(&operation.status);
    let mut per_region: BTreeMap<&str, Vec<StoredOperationResult>> = BTreeMap::new();
    for result in &results {
        per_region
            .entry(result.region.as_str())
            .or_default()
            .push(result.clone());
    }
    Some(
        per_region
            .into_iter()
            .map(|(region, of_region)| {
                let progress = Progress::of(&of_region, concluded);
                (
                    region.to_string(),
                    RegionalStatusDetail {
                        stages: Some(RegionalStages {
                            regional_stack_instances_deployment: Some(
                                regional_deployment(&operation.action, &progress),
                            ),
                        }),
                    },
                )
            })
            .collect(),
    )
}

/// The deployment stage of ONE Region, as the modelled `RegionalStackInstancesDeployment`:
/// the same counts the operation-wide stage reports, over that Region's results, plus that
/// Region's own target total.
fn regional_deployment(action: &str, progress: &Progress) -> RegionalStackInstancesDeployment {
    let deployment = stack_instances_deployment(action, progress);
    RegionalStackInstancesDeployment {
        status: deployment.status,
        succeeded_stack_instances_count: deployment.succeeded_stack_instances_count,
        failed_stack_instances_count: deployment.failed_stack_instances_count,
        completed_stack_instances_count: deployment.completed_stack_instances_count,
        pending_stack_instances_count: deployment.pending_stack_instances_count,
        cancelled_stack_instances_count: deployment.cancelled_stack_instances_count,
        skipped_stack_instances_count: deployment.skipped_stack_instances_count,
        // The one member the operation-wide stage has no counterpart for: the Region's own
        // share of the targets.
        total_stack_instances_count: deploys_stack_instances(action)
            .then(|| Progress::counted(progress.total))
            .flatten(),
        percentage_completed: deployment.percentage_completed,
    }
}
