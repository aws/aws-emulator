// SPDX-License-Identifier: CC0-1.0

//! The VERSION UPGRADE of one domain, and the configuration changes a domain records.
//!
//! An upgrade runs one of two deployment strategies, and both are ONE record on the
//! domain ([`DomainUpgrade`]):
//!
//! * the default, in-place strategy — the domain moves onto the target release when the
//!   clock makes the change due;
//! * `ClonedBlueGreen` — the service stands a CLONED cluster up on the target release
//!   beside the primary one ([`CloneUpgrade`]), and the caller then drives it with
//!   `UpdateClone`: shift a share of the search traffic onto the clone, cut over to the
//!   clone (or back to the primary), and keep the cluster that was cut over from for a
//!   retention period. The domain moves onto the target release when the cutover to the
//!   cloned cluster completes, and never when the caller cuts back.
//!
//! Every answer about an upgrade is COMPUTED from the instants this record stores —
//! when the clone is ready, when a requested traffic shift lands, when a cutover
//! completes, when the retention period ends — so reporting one never writes and two
//! reads of an unchanged record agree.
//!
//! One [`DomainChange`] is recorded per call that changes the domain: the upgrade
//! itself and each `UpdateClone`. That is what gives a caller a `ChangeId` to track
//! the change by, and what `ChangeProgressDetails` and `DescribeDomainChangeProgress`
//! report. The list is bounded ([`MAX_RECORDED_CHANGES`]), since the caller chooses
//! how many changes it makes.

use std::time::SystemTime;

use super::*;

/// The deployment strategy that runs an upgrade as a cloned blue/green one —
/// `DeploymentStrategyOverride`'s only value.
pub const DEPLOYMENT_STRATEGY_CLONED_BLUE_GREEN: &str = "ClonedBlueGreen";

/// `CutoverTarget`'s `ClonedCluster`: the cluster a cutover moves the domain ONTO,
/// which is the one running the upgrade's target release.
pub const CUTOVER_TARGET_CLONED_CLUSTER: &str = "ClonedCluster";

/// `CutoverTarget`'s `PrimaryCluster`: the cluster the domain started on, which a
/// cutover back to ends the upgrade on.
pub const CUTOVER_TARGET_PRIMARY_CLUSTER: &str = "PrimaryCluster";

/// `RetentionAction`'s `RollbackToPrimaryCluster`: "moves the domain back to the
/// primary cluster" — the one action that takes the domain off the cloned cluster
/// again, which is why the release it was carried onto stops being the one it runs.
pub const RETENTION_ACTION_ROLLBACK: &str = "RollbackToPrimaryCluster";

/// `ClonedBlueGreenScheduledActionType`'s `DeletePrimaryCluster`, the action the
/// service takes by itself: "deletes the primary cluster when the retention period
/// ends".
pub const SCHEDULED_ACTION_DELETE_PRIMARY: &str = "DeletePrimaryCluster";

/// The whole of a domain's search traffic, in percent — what the primary and the
/// cloned cluster's shares add up to (`TrafficPercents` carries both, each
/// `@range(0..100)`).
pub const WHOLE_TRAFFIC_PERCENT: i32 = 100;

/// The shortest retention period the service accepts, in hours — straight from
/// `RetentionOptions$RetentionInHours`: "The minimum is 1 hour."
pub const MIN_RETENTION_HOURS: i32 = 1;

/// Seconds in one hour, for turning a retention period in hours into an instant.
const SECONDS_PER_HOUR: u64 = 60 * 60;

/// `InitiatedBy`'s `CUSTOMER`: every change recorded here was asked for by a caller,
/// never started by the service on its own.
pub const INITIATED_BY_CUSTOMER: &str = "CUSTOMER";

/// How many changes one domain keeps.
///
/// A caller chooses how many changes it makes — every `UpdateClone` records one — so
/// this is what bounds that growth site. The oldest entry is dropped once the list is
/// full, which keeps the MOST RECENT change (the one a request naming no `ChangeId` is
/// answered with) and the ids a caller just received resolvable.
pub const MAX_RECORDED_CHANGES: usize = 20;

/// The modelled member a version upgrade changes, as a change's property list names it.
pub const VERSION_PROPERTY: &str = "ElasticsearchVersion";

/// The modelled member a traffic shift changes, as a change's property list names it.
pub const TRAFFIC_SPLIT_PROPERTY: &str = "TrafficSplitOptions";

/// The modelled member a cutover changes.
pub const CUTOVER_PROPERTY: &str = "CutoverOptions";

/// The modelled member a retention change changes.
pub const RETENTION_PROPERTY: &str = "RetentionOptions";

/// The `ChangeId` of the configuration change `seq` names: a `GUID` as this model's
/// pattern writes one (`^\p{XDigit}{8}-\p{XDigit}{4}-…$`).
///
/// The store's monotonic change counter occupies the last six bytes, so an id is unique
/// BY CONSTRUCTION — never merely unlikely to repeat — and the leading bytes are the
/// SHA-256 digest of that counter, so the id is opaque rather than a readable count.
/// The hyphenated version-4 layout is the fleet's one renderer of it
/// ([`::service_common::uuid_from_random_bytes`]), so this crate spells out neither the
/// group boundaries nor the version bits.
pub fn change_id(seq: u64) -> String {
    use sha2::Digest;

    let digest = sha2::Sha256::digest(format!("change/{seq}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[10..].copy_from_slice(&seq.to_be_bytes()[2..]);
    ::service_common::uuid_from_random_bytes(bytes)
}

/// One configuration change of a domain, as stored: what a caller tracks by its
/// `ChangeId`.
#[derive(Debug, Clone)]
pub struct DomainChange {
    /// The change's own identifier, the `GUID` a caller is handed and reads it back by.
    pub change_id: String,
    /// The instant the call that began the change was served.
    pub started_at: SystemTime,
    /// When the clock finishes this change's own work.
    pub due_at: SystemTime,
    /// The modelled properties this change moves — the pending ones until the change
    /// is due and the completed ones afterwards.
    pub properties: Vec<String>,
    /// Whether this change belongs to a cloned blue/green upgrade, which is what makes
    /// the domain's clone state part of what the change reports.
    pub clone_change: bool,
    /// Whether the change is the one that STARTED a cloned blue/green upgrade, which
    /// stays open for as long as that upgrade waits on the caller.
    pub starts_clone_upgrade: bool,
}

impl DomainChange {
    /// Whether the clock has not yet finished this change's own work.
    pub fn applying(&self, now: SystemTime) -> bool {
        now < self.due_at
    }
}

/// When `change`'s status last moved: the instant it was asked for while the clock is
/// still applying it, and the instant the clock finished applying it afterwards — never
/// the instant of the READ, so two reads of a settled change report the same time.
pub fn change_last_updated(change: &DomainChange, now: SystemTime) -> SystemTime {
    if change.applying(now) {
        change.started_at
    } else {
        change.due_at
    }
}

/// The cutover a caller asked for: which cluster the domain is to run on, and when the
/// move completes.
#[derive(Debug, Clone)]
pub struct CloneCutover {
    /// One of `CUTOVER_TARGET_VALUES`.
    pub target: String,
    /// When the clock completes the move.
    pub due_at: SystemTime,
}

/// A cloned blue/green upgrade's own state, as stored.
#[derive(Debug, Clone)]
pub struct CloneUpgrade {
    /// When the cloned cluster finishes being stood up on the target release.
    pub ready_at: SystemTime,
    /// The share of the read traffic the cloned cluster took before the shift that is
    /// in flight — the `Current` half of a reported split until that shift lands.
    pub current_clone_percent: i32,
    /// The share the last shift asked for — the `Target` half.
    pub target_clone_percent: i32,
    /// When the clock completes that shift.
    pub split_due_at: SystemTime,
    /// The cutover a caller asked for, while none has been asked for yet `None`.
    pub cutover: Option<CloneCutover>,
    /// How long the cluster that is cut over from is kept, in hours: what the upgrade
    /// was started with and what a later `UpdateClone` set.
    pub retention_in_hours: Option<i32>,
    /// The `RetentionAction` that ENDED the retention period early, once one was asked
    /// for.
    pub retention_action: Option<String>,
    /// When that action was asked for, which is when the retention period ended.
    pub retention_ended_at: Option<SystemTime>,
}

impl CloneUpgrade {
    /// A clone upgrade just started: the cloned cluster is being stood up, it carries
    /// none of the read traffic yet, and the retention period is the one the upgrade
    /// was started with.
    pub fn start(retention_in_hours: Option<i32>, now: SystemTime) -> Self {
        let ready_at = active_deadline(now);
        Self {
            ready_at,
            current_clone_percent: 0,
            target_clone_percent: 0,
            split_due_at: ready_at,
            cutover: None,
            retention_in_hours,
            retention_action: None,
            retention_ended_at: None,
        }
    }

    /// Whether the cloned cluster is still being stood up.
    pub fn provisioning(&self, now: SystemTime) -> bool {
        now < self.ready_at
    }

    /// When the domain starts RUNNING the target release: the instant a cutover ONTO
    /// the cloned cluster completes. `None` while no cutover was asked for, for a
    /// cutover back to the primary cluster, which leaves the domain on the release it
    /// was already running, and once a `RollbackToPrimaryCluster` retention action has
    /// moved the domain back onto that cluster.
    pub fn version_effective_at(&self) -> Option<SystemTime> {
        if self.retention_action.as_deref() == Some(RETENTION_ACTION_ROLLBACK) {
            return None;
        }
        self.cutover
            .as_ref()
            .filter(|cutover| cutover.target == CUTOVER_TARGET_CLONED_CLUSTER)
            .map(|cutover| cutover.due_at)
    }

    /// When the retention period of the cluster that was cut over from ends: the
    /// instant the `RetentionAction` ended it, else the cutover's completion plus the
    /// retention period. `None` while no cutover onto the clone has completed, when no
    /// retention period was ever set — nothing is kept then — and for a period so long
    /// that no instant is that far ahead, which the clock would never reach anyway.
    pub fn retention_ends_at(&self) -> Option<SystemTime> {
        if let Some(ended) = self.retention_ended_at {
            return Some(ended);
        }
        let completed = self.version_effective_at()?;
        let hours = u64::try_from(self.retention_in_hours?).unwrap_or_default();
        completed.checked_add(std::time::Duration::from_secs(
            hours.saturating_mul(SECONDS_PER_HOUR),
        ))
    }

    /// Whether this upgrade is still one a caller can drive with `UpdateClone`.
    ///
    /// It ends when the caller cuts back to the primary cluster, and after a cutover
    /// onto the cloned cluster when the retention period of the cluster cut over from
    /// has run out (at once, when the upgrade kept none).
    pub fn in_progress(&self, now: SystemTime) -> bool {
        match self.cutover.as_ref() {
            None => true,
            Some(cutover) if now < cutover.due_at => true,
            Some(cutover) if cutover.target == CUTOVER_TARGET_PRIMARY_CLUSTER => false,
            Some(_) => match self.retention_ends_at() {
                Some(ends) => now < ends,
                // A period the clock cannot reach the end of has not ended; no period at
                // all means the cutover released the cluster it left with it.
                None => self.retention_in_hours.is_some(),
            },
        }
    }

    /// The share of the read traffic the cloned cluster carries NOW: the share the last
    /// shift asked for once the clock has made it land, and the one before it until
    /// then.
    pub fn live_clone_percent(&self, now: SystemTime) -> i32 {
        if now < self.split_due_at {
            self.current_clone_percent
        } else {
            self.target_clone_percent
        }
    }

    /// The split of the search traffic `clone_percent` of it reaching the cloned
    /// cluster is: the two shares of one whole.
    pub fn traffic_percents(clone_percent: i32) -> TrafficPercents {
        TrafficPercents {
            primary_read_traffic_percent: WHOLE_TRAFFIC_PERCENT.saturating_sub(clone_percent),
            clone_read_traffic_percent: clone_percent,
        }
    }

    /// The `TrafficSplitOptions` this upgrade reports: the live split, the one the last
    /// shift asked for, and whether that shift is still landing.
    pub fn traffic_split_options(&self, now: SystemTime) -> TrafficSplitOptions {
        TrafficSplitOptions {
            current: Self::traffic_percents(self.live_clone_percent(now)),
            target: Self::traffic_percents(self.target_clone_percent),
            status: if now < self.split_due_at {
                STEP_STATUS_IN_PROGRESS.to_string()
            } else {
                STEP_STATUS_COMPLETED.to_string()
            },
        }
    }

    /// The `CutoverOptions` this upgrade reports — the target a caller asked for and
    /// whether the move has completed — or `None` while no cutover was asked for.
    pub fn cutover_options(&self, now: SystemTime) -> Option<CutoverOptions> {
        let cutover = self.cutover.as_ref()?;
        Some(CutoverOptions {
            target: cutover.target.clone(),
            status: if now < cutover.due_at {
                STEP_STATUS_IN_PROGRESS.to_string()
            } else {
                STEP_STATUS_COMPLETED.to_string()
            },
        })
    }

    /// The `RetentionOptions` this upgrade reports, or `None` when it keeps the cluster
    /// cut over from for no period at all.
    ///
    /// Once a caller has ended the period with a `RetentionAction`, that action is what
    /// the upgrade's retention setting IS. Before cutover completes the reported period
    /// is the one that was asked for; after it, it is "the number of hours remaining in
    /// the retention period, rounded up"
    /// (`RetentionOptions$RetentionInHours`).
    pub fn retention_options(&self, now: SystemTime) -> Option<RetentionOptions> {
        if let Some(action) = self.retention_action.as_ref() {
            return Some(RetentionOptions::RetentionAction(action.clone()));
        }
        let hours = self.retention_in_hours?;
        let Some(ends) = self.retention_ends_at() else {
            return Some(RetentionOptions::RetentionInHours(hours));
        };
        Some(RetentionOptions::RetentionInHours(hours_remaining(
            ends, now,
        )))
    }

    /// The `ClonedBlueGreenUpgradeDetails` a change of this upgrade reports: the live
    /// split of the search traffic, the retention period, and the action the service
    /// takes by itself at the end of that period.
    ///
    /// A member that does not apply is omitted: there is no scheduled action before the
    /// retention period is running, since this emulator schedules nothing for the
    /// caller to pre-empt — the caller drives the upgrade.
    pub fn upgrade_details(&self, now: SystemTime) -> ClonedBlueGreenUpgradeDetails {
        let scheduled_action = self
            .retention_ends_at()
            .filter(|_| self.retention_action.is_none())
            .map(|ends| ClonedBlueGreenScheduledAction {
                action: Some(SCHEDULED_ACTION_DELETE_PRIMARY.to_string()),
                scheduled_time: Some(datetime_of(ends)),
            });
        ClonedBlueGreenUpgradeDetails {
            traffic_split: Some(Self::traffic_percents(self.live_clone_percent(now))),
            scheduled_action,
            retention_in_hours: self.retention_options(now).and_then(|options| {
                match options {
                    RetentionOptions::RetentionInHours(hours) => Some(hours),
                    RetentionOptions::RetentionAction(_) => None,
                }
            }),
        }
    }
}

/// `CutoverStatus`'s and `TrafficSplitStatus`'s `InProgress` — the two enums spell the
/// state of a step in flight with the same value.
pub const STEP_STATUS_IN_PROGRESS: &str = "InProgress";

/// `CutoverStatus`'s and `TrafficSplitStatus`'s `Completed`.
pub const STEP_STATUS_COMPLETED: &str = "Completed";

/// The hours left until `ends`, ROUNDED UP, and zero once that instant has passed.
fn hours_remaining(ends: SystemTime, now: SystemTime) -> i32 {
    let Ok(left) = ends.duration_since(now) else {
        return 0;
    };
    // A part-second left is a part-hour left, so it rounds up to one hour as well.
    let seconds = left.as_secs() + u64::from(left.subsec_nanos() > 0);
    i32::try_from(seconds.div_ceil(SECONDS_PER_HOUR)).unwrap_or(i32::MAX)
}

/// One version upgrade of a domain, as stored.
#[derive(Debug, Clone)]
pub struct DomainUpgrade {
    /// The release the domain was RUNNING when this upgrade was asked for — the one it
    /// keeps until this upgrade lands, and the one it stays on if this upgrade never
    /// does (a cutover back to the primary cluster, a rollback).
    ///
    /// Carried on the upgrade rather than read off the configuration record, which an
    /// upgrade does not rewrite: a domain upgraded twice was running its FIRST
    /// upgrade's target when the second one started, and that is the release a read has
    /// to report while the second upgrade is in flight.
    pub source_version: Option<String>,
    /// The release this upgrade moves the domain onto.
    pub target_version: String,
    /// The change the upgrade was started by, so the upgrade and its `ChangeId` cannot
    /// drift apart.
    pub change_id: String,
    /// When the upgrade was asked for.
    pub started_at: SystemTime,
    /// When an IN-PLACE upgrade lands. `None` for a cloned blue/green one, whose
    /// landing is decided by the caller's cutover.
    pub due_at: Option<SystemTime>,
    /// The cloned cluster's state, for an upgrade running the `ClonedBlueGreen`
    /// strategy.
    pub cloned_blue_green: Option<CloneUpgrade>,
}

impl DomainUpgrade {
    /// An in-place upgrade onto `target_version` from `source_version`, landing when the
    /// clock makes the change due.
    pub fn in_place(
        source_version: Option<String>,
        target_version: String,
        change_id: String,
        now: SystemTime,
    ) -> Self {
        Self {
            source_version,
            target_version,
            change_id,
            started_at: now,
            due_at: Some(active_deadline(now)),
            cloned_blue_green: None,
        }
    }

    /// A cloned blue/green upgrade onto `target_version` from `source_version`, keeping
    /// the cluster that is cut over from for `retention_in_hours`.
    pub fn cloned_blue_green(
        source_version: Option<String>,
        target_version: String,
        change_id: String,
        retention_in_hours: Option<i32>,
        now: SystemTime,
    ) -> Self {
        Self {
            source_version,
            target_version,
            change_id,
            started_at: now,
            due_at: None,
            cloned_blue_green: Some(CloneUpgrade::start(retention_in_hours, now)),
        }
    }

    /// The release the domain RUNS at `now`: this upgrade's target once the clock has
    /// made it effective, and the release the upgrade started from until then — which is
    /// also what a cutover back to the primary cluster leaves it on.
    pub fn running_version(&self, now: SystemTime) -> Option<String> {
        if self.version_landed(now) {
            return Some(self.target_version.clone());
        }
        self.source_version.clone()
    }

    /// When the domain starts RUNNING [`Self::target_version`]: the instant an in-place
    /// upgrade lands, or the one a cutover onto the cloned cluster completes. `None`
    /// while a cloned upgrade has had no cutover asked for, and for one cut back to the
    /// primary cluster.
    pub fn version_effective_at(&self) -> Option<SystemTime> {
        match self.cloned_blue_green.as_ref() {
            Some(clone) => clone.version_effective_at(),
            None => self.due_at,
        }
    }

    /// Whether the domain is RUNNING the target release at `now`.
    pub fn version_landed(&self, now: SystemTime) -> bool {
        self.version_effective_at()
            .is_some_and(|effective| now >= effective)
    }

    /// Whether this upgrade is still running — `UpgradeProcessing` on the domain's
    /// status.
    ///
    /// An in-place upgrade runs until it lands. A cloned blue/green one runs for as long
    /// as the caller can still drive it ([`CloneUpgrade::in_progress`]): the upgrade is
    /// not over when the clone is ready, it is over when the domain has been cut over
    /// and the cluster it left has been released.
    pub fn in_progress(&self, now: SystemTime) -> bool {
        match self.cloned_blue_green.as_ref() {
            Some(clone) => clone.in_progress(now),
            None => !self.version_landed(now),
        }
    }
}

/// What a DOMAIN answers about its own upgrade and its recorded changes — the reads
/// every operation of this surface reports them through, held beside the records they
/// are computed from rather than in the state root.
impl Domain {
    /// Record one configuration change and hand back the `ChangeId` that tracks it.
    ///
    /// The oldest entry is dropped once [`MAX_RECORDED_CHANGES`] are held, so a caller
    /// looping over changes cannot grow this record without end.
    pub fn record_change(&mut self, change: DomainChange) -> String {
        let change_id = change.change_id.clone();
        self.changes.push(change);
        if self.changes.len() > MAX_RECORDED_CHANGES {
            self.changes.remove(0);
        }
        change_id
    }

    /// The change `change_id` names, if this domain recorded one under it.
    pub fn change(&self, change_id: &str) -> Option<&DomainChange> {
        self.changes
            .iter()
            .find(|change| change.change_id == change_id)
    }

    /// The domain's most recent configuration change — what a request naming no
    /// `ChangeId` is answered about ("If omitted, the service returns information about
    /// the most recent configuration change").
    pub fn latest_change(&self) -> Option<&DomainChange> {
        self.changes.last()
    }

    /// The cloned blue/green upgrade a caller can still drive on this domain, or `None`
    /// when it has none ([`CloneUpgrade::in_progress`]).
    pub fn clone_upgrade(&self, now: SystemTime) -> Option<&CloneUpgrade> {
        self.upgrade
            .as_ref()?
            .cloned_blue_green
            .as_ref()
            .filter(|clone| clone.in_progress(now))
    }

    /// The same upgrade, to drive it through.
    pub fn clone_upgrade_mut(&mut self, now: SystemTime) -> Option<&mut CloneUpgrade> {
        self.upgrade
            .as_mut()?
            .cloned_blue_green
            .as_mut()
            .filter(|clone| clone.in_progress(now))
    }

    /// The release the domain RUNS at `now`.
    ///
    /// The release the domain's last upgrade left it on
    /// ([`DomainUpgrade::running_version`]) — its target once the clock has made that
    /// upgrade effective, and the release the upgrade started from until then, which is
    /// what a cutover back to the primary cluster leaves the domain on. A domain no
    /// upgrade has touched runs the release its configuration record holds.
    pub fn running_version(&self, now: SystemTime) -> Option<String> {
        self.upgrade
            .as_ref()
            .and_then(|upgrade| upgrade.running_version(now))
            .or_else(|| {
                self.config
                    .elasticsearch_version
                    .as_ref()
                    .map(|record| record.value.clone())
            })
    }

    /// Whether a version upgrade of this domain is still running —
    /// `ElasticsearchDomainStatus$UpgradeProcessing`.
    pub fn upgrade_processing(&self, now: SystemTime) -> bool {
        self.upgrade
            .as_ref()
            .is_some_and(|upgrade| upgrade.in_progress(now))
    }

    /// One recorded change as the `ChangeProgressDetails` the domain's status and
    /// config report it by.
    pub fn change_progress_details(
        &self,
        change: &DomainChange,
        now: SystemTime,
    ) -> ChangeProgressDetails {
        ChangeProgressDetails {
            change_id: Some(change.change_id.clone()),
            // No message: the service carries one only for a change it has something
            // to say about, and nothing here does.
            message: None,
            config_change_status: Some(self.config_change_status(change, now).to_string()),
            start_time: Some(datetime_of(change.started_at)),
            last_updated_time: Some(datetime_of(change_last_updated(change, now))),
            initiated_by: Some(INITIATED_BY_CUSTOMER.to_string()),
        }
    }

    /// The change's `ConfigChangeStatus` at `now`.
    ///
    /// A change is `ApplyingChanges` while the clock has not finished its own work. The
    /// change that STARTED a cloned blue/green upgrade is then `PendingUserInput` for as
    /// long as that upgrade waits on the caller to shift traffic, cut over or release the
    /// cluster it left; every other change, and that one once its upgrade is over, is
    /// `Completed`.
    pub fn config_change_status(&self, change: &DomainChange, now: SystemTime) -> &'static str {
        if change.applying(now) {
            return "ApplyingChanges";
        }
        if change.starts_clone_upgrade && self.clone_upgrade(now).is_some() {
            return "PendingUserInput";
        }
        "Completed"
    }

    /// The change's `OverallChangeStatus` at `now`: `PROCESSING` while the clock is
    /// still applying it, `PENDING` while it waits on the caller, else `COMPLETED`.
    pub fn overall_change_status(&self, change: &DomainChange, now: SystemTime) -> &'static str {
        match self.config_change_status(change, now) {
            "ApplyingChanges" => "PROCESSING",
            "PendingUserInput" => "PENDING",
            _ => "COMPLETED",
        }
    }

    /// One recorded change as the `ChangeProgressStatusDetails`
    /// `DescribeDomainChangeProgress` answers with.
    ///
    /// The properties the change moves are PENDING while the clock is still applying it
    /// and COMPLETED afterwards, which is what the two member's own documentation says
    /// they hold. `ChangeProgressStages` is not reported: the stages of a change are the
    /// service's own and this model names none of them, so there is no stage set to
    /// report and the stage count is zero.
    pub fn change_progress_status(
        &self,
        change: &DomainChange,
        now: SystemTime,
    ) -> ChangeProgressStatusDetails {
        let applying = change.applying(now);
        ChangeProgressStatusDetails {
            change_id: Some(change.change_id.clone()),
            start_time: Some(datetime_of(change.started_at)),
            status: Some(self.overall_change_status(change, now).to_string()),
            pending_properties: Some(if applying {
                change.properties.clone()
            } else {
                Vec::new()
            }),
            completed_properties: Some(if applying {
                Vec::new()
            } else {
                change.properties.clone()
            }),
            total_number_of_stages: 0,
            change_progress_stages: None,
            config_change_status: Some(self.config_change_status(change, now).to_string()),
            last_updated_time: Some(datetime_of(change_last_updated(change, now))),
            initiated_by: Some(INITIATED_BY_CUSTOMER.to_string()),
            // Only a change of a cloned blue/green upgrade carries the clone's state.
            cloned_blue_green_upgrade_details: change
                .clone_change
                .then(|| {
                    self.upgrade
                        .as_ref()
                        .and_then(|upgrade| upgrade.cloned_blue_green.as_ref())
                        .map(|clone| clone.upgrade_details(now))
                })
                .flatten(),
        }
    }
}
