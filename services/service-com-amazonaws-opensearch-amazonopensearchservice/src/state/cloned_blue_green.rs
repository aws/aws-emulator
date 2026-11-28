// SPDX-License-Identifier: CC0-1.0

//! The CLONED BLUE/GREEN upgrade of a domain, as this emulator persists it: the split of read
//! traffic between the primary cluster and the cloned one, the cutover, and the retention
//! period for the cluster the domain cut over from.
//!
//! An `UpgradeDomain` whose `DeploymentStrategyOptions` names
//! [`DEPLOYMENT_STRATEGY_CLONED_BLUE_GREEN`] "creates a cloned cluster on the target version
//! next to the primary cluster so that you can validate it and shift traffic to it before you
//! cut over" (`DeploymentStrategyOverrideOptions$DeploymentStrategy`). `UpdateClone` is what
//! manages that upgrade afterwards — shifting read traffic, cutting over either way, and
//! changing or ending the retention period — and `DescribeDomainChangeProgress` reports its
//! state as the `ClonedBlueGreenUpgradeDetails` of the changes that belong to it.
//!
//! The record lives ON the [`Domain`] it belongs to, because the clone is a second cluster of
//! that one domain rather than a resource of its own: releasing the domain releases it, and
//! the ONE record is what every operation that reads or writes the upgrade reaches, so no two
//! of them can report it differently.
//!
//! Every move is asynchronous and recorded as a due instant the clock makes fall
//! ([`ClonedBlueGreenUpgrade::settle`]); nothing sleeps.

use super::*;

/// `DeploymentStrategyOverride`'s only value: the strategy that clones the cluster.
pub const DEPLOYMENT_STRATEGY_CLONED_BLUE_GREEN: &str = "ClonedBlueGreen";

/// `CutoverTarget$ClonedCluster`: cut the domain over TO the cloned cluster.
pub const CUTOVER_TARGET_CLONED_CLUSTER: &str = "ClonedCluster";

/// `CutoverTarget$PrimaryCluster`: cut the domain back to the cluster it started on.
pub const CUTOVER_TARGET_PRIMARY_CLUSTER: &str = "PrimaryCluster";

/// `CutoverStatus$InProgress` / `TrafficSplitStatus$InProgress`: the move is under way.
pub const CLONE_MOVE_IN_PROGRESS: &str = "InProgress";

/// `CutoverStatus$Completed` / `TrafficSplitStatus$Completed`: the move has happened.
pub const CLONE_MOVE_COMPLETED: &str = "Completed";

/// `TrafficStrategy`'s only value: shift a percentage of read traffic to the clone.
pub const TRAFFIC_STRATEGY_SHIFT_READ_TRAFFIC_PERCENT: &str = "ShiftReadTrafficPercent";

/// `RetentionAction$RollbackToPrimaryCluster`: end the retention period by going back to the
/// cluster the domain started on.
pub const RETENTION_ACTION_ROLLBACK_TO_PRIMARY_CLUSTER: &str = "RollbackToPrimaryCluster";

/// `RetentionAction$DeletePrimaryCluster` and
/// `ClonedBlueGreenScheduledActionType$DeletePrimaryCluster`: end the retention period by
/// letting go of the cluster the domain cut over from.
pub const RETENTION_ACTION_DELETE_PRIMARY_CLUSTER: &str = "DeletePrimaryCluster";

/// The whole of a domain's read traffic, which the two clusters' shares add up to.
const WHOLE_TRAFFIC_PERCENT: i32 = 100;

/// One hour, the unit `RetentionInHours` is stated in.
const HOUR: Duration = Duration::from_secs(60 * 60);

/// How long one move of a cloned blue/green upgrade takes before the clock owes its outcome.
///
/// [`DOMAIN_WORK`], the span this control plane gives work on a domain, for the reason that
/// span states: a traffic shift and a cutover each leave the upgrade in `InProgress`, which is
/// the status a client POLLS for, so the window has to stay open for as long as a client takes
/// to make its next call. AWS takes many minutes.
pub const CLONE_MOVE: Duration = DOMAIN_WORK;

/// A cloned blue/green upgrade of one domain.
#[derive(Debug, Clone)]
pub struct ClonedBlueGreenUpgrade {
    /// The instant the `UpgradeDomain` that asked for this strategy was accepted.
    pub started_at: SystemTime,
    /// The share of read traffic each cluster serves NOW — `TrafficPercents`, reported as the
    /// `TrafficSplit` of the upgrade and as the `Current` of a traffic shift.
    pub current_clone_percent: i32,
    /// The share the shift in flight is moving to, and the same as
    /// [`Self::current_clone_percent`] when none is.
    pub target_clone_percent: i32,
    /// `TrafficSplitStatus` for the last shift this upgrade was asked for.
    pub traffic_status: String,
    /// When the clock owes the shift in flight; `None` once it has landed.
    pub traffic_due: Option<SystemTime>,
    /// `CutoverOptions$Target`: which cluster the last cutover this upgrade was asked for
    /// moves the domain to, and `None` while none has been asked for.
    pub cutover_target: Option<String>,
    /// `CutoverOptions$Status` for that cutover.
    pub cutover_status: Option<String>,
    /// When the clock owes it; `None` once it has landed.
    pub cutover_due: Option<SystemTime>,
    /// The instant the cutover to the CLONED cluster completed, which is when the retention
    /// period for the cluster the domain cut over from starts running.
    pub cut_over_at: Option<SystemTime>,
    /// `RetentionInHours`: "the retention period, in hours, for the cluster that you cut over
    /// from", as the request that set it last asked for; `None` once the period has ended.
    pub retention_in_hours: Option<i32>,
    /// The `RetentionAction` the period ENDED with, and `None` while it has not ended.
    pub retention_action: Option<String>,
    /// The instant this upgrade concluded — the retention period ended, either because a
    /// caller ended it or because the action the service had scheduled fell due — and `None`
    /// while it is still being managed.
    pub concluded_at: Option<SystemTime>,
}

impl ClonedBlueGreenUpgrade {
    /// The upgrade an accepted `UpgradeDomain` carrying this strategy starts.
    ///
    /// All read traffic is on the primary cluster to begin with: the clone exists to be
    /// validated, and nothing is served from it until a caller shifts traffic to it.
    #[must_use]
    pub fn started(retention_in_hours: Option<i32>, now: SystemTime) -> Self {
        Self {
            started_at: now,
            current_clone_percent: 0,
            target_clone_percent: 0,
            traffic_status: CLONE_MOVE_COMPLETED.to_string(),
            traffic_due: None,
            cutover_target: None,
            cutover_status: None,
            cutover_due: None,
            cut_over_at: None,
            retention_in_hours,
            retention_action: None,
            concluded_at: None,
        }
    }

    /// Whether this upgrade is still one `UpdateClone` manages: "manages a cloned blue/green
    /// upgrade that is IN PROGRESS on a domain".
    #[must_use]
    pub fn in_progress(&self) -> bool {
        self.concluded_at.is_none()
    }

    /// Shift read traffic so the cloned cluster serves `clone_percent` of it.
    ///
    /// The shift is `InProgress` until the clock makes it due, and the split a read reports
    /// stays the one in force until then: traffic that has not moved yet is not reported as
    /// moved. A shift to the share already in force moves nothing, so it is `Completed` at
    /// once — there is nothing for the clock to owe it.
    pub fn shift_read_traffic(&mut self, clone_percent: i32, now: SystemTime) {
        self.target_clone_percent = clone_percent.clamp(0, WHOLE_TRAFFIC_PERCENT);
        if self.traffic_due.is_none() && self.target_clone_percent == self.current_clone_percent {
            self.traffic_status = CLONE_MOVE_COMPLETED.to_string();
            return;
        }
        self.traffic_status = CLONE_MOVE_IN_PROGRESS.to_string();
        self.traffic_due = Some(now + CLONE_MOVE);
    }

    /// Cut the domain over to `target`.
    ///
    /// A cutover decides which cluster serves the domain, so it carries the read traffic with
    /// it: once it lands, the cluster cut over to serves all of it. A cutover to the cluster
    /// the domain has already been cut over to moves nothing and is `Completed` at once.
    pub fn begin_cutover(&mut self, target: &str, now: SystemTime) {
        let landed = self.cutover_due.is_none()
            && self.cutover_target.as_deref() == Some(target)
            && self.cutover_status.as_deref() == Some(CLONE_MOVE_COMPLETED);
        self.cutover_target = Some(target.to_string());
        if landed {
            return;
        }
        self.cutover_status = Some(CLONE_MOVE_IN_PROGRESS.to_string());
        self.cutover_due = Some(now + CLONE_MOVE);
    }

    /// Change the retention period for the cluster the domain cut over from.
    pub fn set_retention_in_hours(&mut self, hours: i32) {
        self.retention_in_hours = Some(hours);
        self.retention_action = None;
    }

    /// End the retention period with `action`, which is what concludes the upgrade.
    ///
    /// Each action has its own effect on which cluster serves the domain, and both are
    /// reported through the same members a cutover is:
    /// [`RETENTION_ACTION_ROLLBACK_TO_PRIMARY_CLUSTER`] moves the domain back to the cluster
    /// it started on, and [`RETENTION_ACTION_DELETE_PRIMARY_CLUSTER`] leaves the cloned
    /// cluster serving it alone.
    pub fn end_retention(&mut self, action: &str, now: SystemTime) {
        self.retention_in_hours = None;
        self.retention_action = Some(action.to_string());
        self.concluded_at = Some(now);
        match action {
            RETENTION_ACTION_ROLLBACK_TO_PRIMARY_CLUSTER => {
                self.begin_cutover(CUTOVER_TARGET_PRIMARY_CLUSTER, now);
            }
            RETENTION_ACTION_DELETE_PRIMARY_CLUSTER => {
                self.begin_cutover(CUTOVER_TARGET_CLONED_CLUSTER, now);
            }
            // Unreachable: `RetentionAction` declares the two values above and the request
            // boundary refuses any other.
            _ => {}
        }
    }

    /// Materialise whatever the clock owes this upgrade at `now`: a traffic shift and a
    /// cutover each land once their own due instant has passed, and the action the service
    /// scheduled ends the retention period once its scheduled time has.
    ///
    /// Each outcome is dated at the DUE instant rather than at `now`, so what a caller reads
    /// does not move with the moment it happened to look.
    pub(super) fn settle(&mut self, now: SystemTime) {
        if self.traffic_due.is_some_and(|due| now >= due) {
            self.current_clone_percent = self.target_clone_percent;
            self.traffic_status = CLONE_MOVE_COMPLETED.to_string();
            self.traffic_due = None;
        }
        if let Some(due) = self.cutover_due.filter(|due| now >= *due) {
            self.cutover_status = Some(CLONE_MOVE_COMPLETED.to_string());
            self.cutover_due = None;
            // The cluster the domain was cut over to serves all of its read traffic.
            let cut_over_to_clone =
                self.cutover_target.as_deref() == Some(CUTOVER_TARGET_CLONED_CLUSTER);
            self.current_clone_percent = if cut_over_to_clone {
                WHOLE_TRAFFIC_PERCENT
            } else {
                0
            };
            self.target_clone_percent = self.current_clone_percent;
            self.traffic_status = CLONE_MOVE_COMPLETED.to_string();
            self.traffic_due = None;
            if cut_over_to_clone {
                // The retention period for the cluster the domain cut over FROM runs from
                // here ([`Self::scheduled_action`]).
                self.cut_over_at = Some(due);
            } else {
                // The domain is back on the cluster it started on: the clone it was being
                // upgraded onto is no longer what serves it, so the upgrade is over.
                self.cut_over_at = None;
                self.concluded_at = Some(due);
            }
        }
        if let Some((_, at)) = self.scheduled_action().filter(|(_, at)| now >= *at) {
            self.retention_in_hours = None;
            self.retention_action = Some(RETENTION_ACTION_DELETE_PRIMARY_CLUSTER.to_string());
            self.concluded_at = Some(at);
        }
    }

    /// The action the service takes automatically if the caller does not act before the
    /// scheduled time, and the instant it takes it.
    ///
    /// There is one exactly while a retention period is running: the period is "how long to
    /// keep the cluster that you cut over from AFTER CUTOVER COMPLETES"
    /// (`ClonedBlueGreenOptions$RetentionOptions`), so it starts when the cutover to the
    /// cloned cluster lands and the action at the end of it is the one that lets go of the
    /// cluster the domain cut over from. Before that cutover, and once the period has ended,
    /// nothing is scheduled and the member is omitted — "members that don't apply are
    /// omitted" (`ClonedBlueGreenUpgradeDetails`).
    fn scheduled_action(&self) -> Option<(&'static str, SystemTime)> {
        let hours = u32::try_from(self.retention_in_hours?).ok()?;
        let at = self.cut_over_at?.checked_add(HOUR.checked_mul(hours)?)?;
        Some((RETENTION_ACTION_DELETE_PRIMARY_CLUSTER, at))
    }

    /// The split of read traffic in force, as the modelled `TrafficPercents`.
    #[must_use]
    pub fn traffic_split(&self) -> TrafficPercents {
        TrafficPercents {
            primary_read_traffic_percent: WHOLE_TRAFFIC_PERCENT - self.current_clone_percent,
            clone_read_traffic_percent: self.current_clone_percent,
        }
    }

    /// The split the shift in flight is moving to, and the one in force when none is.
    #[must_use]
    fn target_traffic_split(&self) -> TrafficPercents {
        TrafficPercents {
            primary_read_traffic_percent: WHOLE_TRAFFIC_PERCENT - self.target_clone_percent,
            clone_read_traffic_percent: self.target_clone_percent,
        }
    }

    /// This upgrade's traffic split as `UpdateClone` reports it: where the traffic is, where
    /// it is going, and whether it has got there.
    #[must_use]
    pub fn to_traffic_split_options(&self) -> TrafficSplitOptions {
        TrafficSplitOptions {
            current: self.traffic_split(),
            target: self.target_traffic_split(),
            status: self.traffic_status.clone(),
        }
    }

    /// The cutover this upgrade was last asked for, and `None` while it has been asked for
    /// none.
    #[must_use]
    pub fn to_cutover_options(&self) -> Option<CutoverOptions> {
        Some(CutoverOptions {
            target: self.cutover_target.clone()?,
            status: self
                .cutover_status
                .clone()
                .unwrap_or_else(|| CLONE_MOVE_COMPLETED.to_string()),
        })
    }

    /// The retention settings in force: the period while one is running, the action it ended
    /// with once it has, and nothing when this upgrade was started without one.
    #[must_use]
    pub fn to_retention_options(&self) -> Option<RetentionOptions> {
        match (self.retention_in_hours, self.retention_action.as_ref()) {
            (Some(hours), _) => Some(RetentionOptions::RetentionInHours(hours)),
            (None, Some(action)) => Some(RetentionOptions::RetentionAction(action.clone())),
            (None, None) => None,
        }
    }

    /// This upgrade as the `ClonedBlueGreenUpgradeDetails` a change of it reports.
    #[must_use]
    pub fn to_details(&self) -> ClonedBlueGreenUpgradeDetails {
        ClonedBlueGreenUpgradeDetails {
            traffic_split: Some(self.traffic_split()),
            scheduled_action: self.scheduled_action().map(|(action, at)| {
                ClonedBlueGreenScheduledAction {
                    action: Some(action.to_string()),
                    scheduled_time: Some(datetime(at)),
                }
            }),
            retention_in_hours: self.retention_in_hours,
        }
    }
}

/// What this service says of an `UpdateClone` on a domain with no cloned blue/green upgrade to
/// manage — one that was never upgraded with that strategy, and one whose upgrade has already
/// concluded.
#[must_use]
pub fn no_clone_in_progress(domain_name: &str) -> BaseException {
    BaseException {
        message: Some(format!(
            "No cloned blue/green upgrade is in progress on domain: {domain_name}"
        )),
    }
}

/// What it says of a request that does not name exactly one of the three option blocks:
/// "specify exactly one of `TrafficSplitOptions`, `CutoverOptions`, or `RetentionOptions` in
/// each request" (`smithy_query(query="doc",shape="UpdateClone")`).
#[must_use]
pub fn one_clone_option_required() -> ValidationException {
    ValidationException {
        message: Some(
            "Specify exactly one of TrafficSplitOptions, CutoverOptions, or RetentionOptions"
                .to_string(),
        ),
    }
}

/// What `UpgradeDomain` says of a request that asks to START a cloned blue/green upgrade with
/// a retention ACTION: "when you start the upgrade, only `RetentionInHours` is accepted"
/// (`ClonedBlueGreenOptions$RetentionOptions`).
#[must_use]
pub fn only_retention_in_hours_at_start() -> ValidationException {
    ValidationException {
        message: Some(
            "Only RetentionInHours is accepted in ClonedBlueGreenOptions when you start the \
             upgrade"
                .to_string(),
        ),
    }
}
