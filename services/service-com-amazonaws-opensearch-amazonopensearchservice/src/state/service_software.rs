// SPDX-License-Identifier: CC0-1.0

//! A domain's service software, and the engine-version upgrades it is put through.
//!
//! [`ServiceSoftware`] is the ONE record `DescribeDomain` reports as
//! `ServiceSoftwareOptions` and `StartServiceSoftwareUpdate`,
//! `CancelServiceSoftwareUpdate` and `RollbackServiceSoftwareUpdate` move, so what a read
//! reports and what an update does cannot disagree. [`UpgradeRun`] is the same for an
//! engine-version upgrade: `UpgradeDomain` starts one and `GetUpgradeStatus` /
//! `GetUpgradeHistory` report it, step by step, from that record.

use super::*;

/// A service software deployment in flight on a domain: the release it is moving the
/// domain onto, and when the clock owes it completion.
///
/// A deployment is started by a service-software call and completes when the span the
/// clock owes it has passed ([`DOMAIN_WORK`], the same span a configuration change
/// takes) — materialised by whichever later call reads the record, so nothing sleeps.
#[derive(Debug, Clone)]
pub struct SoftwareDeployment {
    /// The release the domain is being moved onto, reported as
    /// `ServiceSoftwareOptions$NewVersion` while the deployment is in flight.
    pub target_version: String,
    /// When the clock owes this deployment completion.
    pub completes_at: SystemTime,
    /// Whether this deployment moves the domain BACK onto an earlier release. A
    /// landed rollback leaves the release it came off available to update to again,
    /// never available to roll back to — so which direction a deployment ran decides
    /// what the domain can do next.
    pub rollback: bool,
}

/// `RollbackServiceSoftwareOptions$Description` for a domain that has no earlier
/// release to return to: the service's own sentence for that decline, which names
/// the reason (no previous version) and where to take it further.
pub const NO_ROLLBACK_TARGET_DESCRIPTION: &str =
    "No previous software version available for rollback. Please contact AWS Support \
     for assistance.";

/// How a descriptor of a domain names the domain it describes: itself. A refusal
/// names it by the name the request carried instead, which is the only difference
/// between the two statements of the sentences below.
pub(super) const THIS_DOMAIN: &str = "this domain";

/// How the service states that a domain has no service software update to apply.
///
/// `ServiceSoftwareOptions$Description` states it about the domain being described
/// and a refused `StartServiceSoftwareUpdate` states it about the domain the request
/// named, so the sentence is written ONCE and only `domain` differs.
pub(super) fn no_update_available_sentence(domain: &str) -> String {
    format!("There is no software update available for {domain}.")
}

/// The same for a domain that is already applying a deployment.
pub(super) fn deployment_in_progress_sentence(domain: &str) -> String {
    format!("A service software deployment is already in progress on {domain}.")
}

/// The same for a domain that has an update scheduled and not yet begun.
pub(super) fn scheduled_update_sentence(domain: &str) -> String {
    format!("A service software update is already scheduled on {domain}.")
}

/// How the service states that a domain cannot take a service software update now:
/// a deployment is already in flight on it, one is already scheduled, no release is
/// available for it to be moved onto, or other work is in flight on the domain.
///
/// ONE sentence for the whole rule, with no variable part — the service states the
/// condition, not which part of it the record failed, and not the domain the request
/// named.
pub const DOMAIN_NOT_AVAILABLE_FOR_UPDATE: &str = "The domain is not available for update.";

/// The `ValidationException` payload for a service software update a domain is not in
/// a state to take.
///
/// `StartServiceSoftwareUpdate` models `ValidationException` for a request the
/// service cannot serve as asked, and a domain that is not available for an update is
/// exactly that — there is no release to deploy, so nothing is scheduled.
pub fn domain_not_available_for_update() -> ValidationException {
    ValidationException {
        message: Some(DOMAIN_NOT_AVAILABLE_FOR_UPDATE.to_string()),
    }
}

/// How the service states that a domain has no service software update a cancel can
/// call off: both halves of the answer — that the cancel cannot be done, and why —
/// in one fixed sentence carrying no variable part.
pub const NO_PENDING_UPDATE_TO_CANCEL: &str =
    "The domain update cannot be cancelled. There is no pending update.";

/// The `ValidationException` payload for a service software update a domain has none
/// of to call off: nothing is scheduled on it, or the deployment it scheduled has
/// already begun — and a deployment under way is past the point a cancel can reach.
///
/// `CancelServiceSoftwareUpdate` is documented "you can only perform this operation
/// before the `AutomatedUpdateDate` and when the domain's `UpdateStatus` is
/// `PENDING_UPDATE`", so outside that state there is nothing to cancel and the call is
/// refused with the operation's modelled `ValidationException`.
pub fn no_software_update_to_cancel() -> ValidationException {
    ValidationException {
        message: Some(NO_PENDING_UPDATE_TO_CANCEL.to_string()),
    }
}

// ---------------------------------------------------------------------------
// The refusals a domain with work in flight answers with
//
// A domain that is being provisioned, applying a configuration change, upgrading
// its engine or deploying service software ([`Domain::work_in_flight`]) serves
// neither a report of its nodes nor a call that would start further work on it.
// Each operation refuses with the error ITS model declares for the condition and
// with the service's own fixed sentence for it, which is why the sentences live
// here beside one another rather than in the handlers.
// ---------------------------------------------------------------------------

/// How the service states that the node information of a domain cannot be fetched
/// right now, and that the condition passes.
pub const NODE_INFORMATION_UNAVAILABLE: &str =
    "The node information is not available now, please try again later.";

/// The `DependencyFailureException` payload for a report of a domain's nodes the
/// service cannot answer: the nodes of a domain with work in flight are not up, or
/// are being replaced, so there is no node census to read.
///
/// `DependencyFailureException` is "an exception for when a failure in one of the
/// dependencies results in the service being unable to fetch details about the
/// resource" (`@httpError 424`), which is that condition exactly.
pub fn node_information_unavailable() -> DependencyFailureException {
    DependencyFailureException {
        message: Some(NODE_INFORMATION_UNAVAILABLE.to_string()),
    }
}

/// The `BaseException` payload for the additional validation check a verbose dry run
/// runs against the live domain, which cannot run while the domain is busy.
pub fn verbose_dry_run_during_activity() -> BaseException {
    BaseException {
        message: Some(
            "Verbose Dry Run cannot be performed at this time due to ongoing activities on \
             the domain"
                .to_string(),
        ),
    }
}

/// The `BaseException` payload for an engine version upgrade asked for on a domain
/// that is already busy with work of its own.
pub fn upgrade_blocked_by_background_tasks() -> BaseException {
    BaseException {
        message: Some("Upgrade failed. Domain has background tasks in progress".to_string()),
    }
}

/// The `ValidationException` payload for a call that would start another change on a
/// domain while one is still being applied.
pub fn change_already_in_progress() -> ValidationException {
    ValidationException {
        message: Some(
            "A change/update is in progress. Please wait for it to complete before \
             requesting another change."
                .to_string(),
        ),
    }
}

/// The `ValidationException` payload for a cancel asked for outside the two states a
/// domain has a change to cancel in, naming them.
pub fn cancel_outside_cancellable_states() -> ValidationException {
    ValidationException {
        message: Some(format!(
            "CancelDomainConfigChange only when domain state is in \
             {PROCESSING_STATUS_MODIFYING} or {PROCESSING_STATUS_UPDATING_SERVICE_SOFTWARE}."
        )),
    }
}

/// The two `DomainProcessingStatusType` values a domain has a configuration change to
/// cancel in: one applying a change of its options, and one deploying service
/// software. Every other state — being created, deleted, upgraded, or settled — has
/// no such change, which is what [`cancel_outside_cancellable_states`] states.
pub const CANCELLABLE_PROCESSING_STATUSES: [&str; 2] = [
    PROCESSING_STATUS_MODIFYING,
    PROCESSING_STATUS_UPDATING_SERVICE_SOFTWARE,
];

/// A service software update a caller asked to start LATER: the release it will move
/// the domain onto, and the instant the schedule the caller named puts it at.
///
/// An update asked for at an instant still ahead has been REQUESTED and has not begun,
/// which is `DeploymentStatus`'s `PENDING_UPDATE` and the one state
/// `ServiceSoftwareOptions$Cancellable` ("true if you're able to cancel your service
/// software version update") is true in. It becomes a [`SoftwareDeployment`] when the
/// clock reaches `starts_at` — materialised by whichever later call reads the record,
/// so nothing sleeps.
#[derive(Debug, Clone)]
pub struct PendingUpdate {
    /// The release the domain will be moved onto, reported as
    /// `ServiceSoftwareOptions$NewVersion` while the update waits.
    pub target_version: String,
    /// When the schedule the request named has the update begin.
    pub starts_at: SystemTime,
}

/// The service software of one domain: the release it runs, the deployment in flight
/// on it, and whether the release it last came off is still within its rollback
/// window.
///
/// This is the ONE record behind `ServiceSoftwareOptions` (reported by every full
/// descriptor of a domain) and `RollbackServiceSoftwareOptions` (reported by
/// `RollbackServiceSoftwareUpdate`), so the two projections can never disagree about
/// which release a domain runs.
#[derive(Debug, Clone)]
pub struct ServiceSoftware {
    /// `ServiceSoftwareOptions$CurrentVersion`: the release the domain is running.
    pub current_version: String,
    /// A release the domain can be moved onto, when one is available and no
    /// deployment is in flight.
    pub available_version: Option<String>,
    /// The deployment in flight, if any.
    pub deployment: Option<SoftwareDeployment>,
    /// The update a caller asked to start at a later instant, if any: requested, not
    /// begun, and so the one thing a cancel could still call off.
    pub pending_update: Option<PendingUpdate>,
    /// The release a rollback would return the domain to: the one it ran before the
    /// deployment that last landed. `None` on a domain no deployment has landed on,
    /// and on one whose last deployment was itself a rollback.
    pub rollback_to: Option<String>,
    /// When the rollback window of the deployment that last landed closes; see
    /// [`ROLLBACK_WINDOW`]. A rollback is available only strictly before it.
    pub rollback_until: Option<SystemTime>,
    /// `ServiceSoftwareOptions$OptionalDeployment`: "True if a service software is
    /// never automatically updated."
    pub optional_deployment: bool,
}

impl ServiceSoftware {
    /// The service software of a domain that has just been created: it runs the
    /// current release of the software built for the engine version it was created on
    /// ([`service_software_release`]), nothing is deploying, and there is no earlier
    /// release to roll back to.
    pub fn initial(engine_version: &str) -> Self {
        Self {
            current_version: service_software_release(engine_version),
            available_version: None,
            deployment: None,
            pending_update: None,
            rollback_to: None,
            rollback_until: None,
            // No automated deployment is scheduled on a domain this emulator holds,
            // so no release of it is ever updated without a caller asking.
            optional_deployment: true,
        }
    }

    /// Whether a deployment is being applied right now.
    pub fn deploying(&self) -> bool {
        self.deployment.is_some()
    }

    /// Materialise the deployment completion the clock owes this record, if `now` is
    /// past its due instant: the release it was moving onto becomes the one running,
    /// and what the domain can do next follows from which direction it ran.
    ///
    /// An update asked for at a later instant is begun here too, once the clock has
    /// reached it: that deployment was owed at the same instant whether or not anybody
    /// called.
    ///
    /// The landing is dated at the DUE instant rather than at `now`, so the rollback
    /// window a caller reads does not move with the moment it happened to look.
    pub(super) fn settle(&mut self, now: SystemTime) {
        // An update is begun before any landing is considered, so one whose whole span
        // has passed between two reads both begins and lands in a single settling.
        if self
            .pending_update
            .as_ref()
            .is_some_and(|pending| now >= pending.starts_at)
        {
            if let Some(pending) = self.pending_update.take() {
                self.deployment = Some(SoftwareDeployment {
                    completes_at: pending
                        .starts_at
                        .checked_add(DOMAIN_WORK)
                        .unwrap_or(pending.starts_at),
                    target_version: pending.target_version,
                    rollback: false,
                });
            }
        }
        let Some(deployment) = self.deployment.as_ref() else {
            return;
        };
        if now < deployment.completes_at {
            return;
        }
        let Some(deployment) = self.deployment.take() else {
            return;
        };
        let came_off = ::std::mem::replace(&mut self.current_version, deployment.target_version);
        if deployment.rollback {
            // Rolling back does not open a window to roll forward again: the release
            // the domain came off is simply an update it can take once more.
            self.available_version = Some(came_off);
            self.rollback_to = None;
            self.rollback_until = None;
        } else {
            // The update that just landed is the one a rollback would undo, for as
            // long as its window stays open.
            self.available_version = None;
            self.rollback_to = Some(came_off);
            self.rollback_until = Some(deployment.completes_at + ROLLBACK_WINDOW);
        }
    }

    /// Whether a service software update is available on this domain: a release it can
    /// be moved onto is available, no deployment is in flight to interrupt and none is
    /// waiting to begin.
    ///
    /// This is `ServiceSoftwareOptions$UpdateAvailable` — "true if you're able to
    /// update your service software version" — so the member a descriptor reports and
    /// the eligibility [`ServiceSoftware::begin_update`] tests are ONE answer.
    pub fn update_available(&self) -> bool {
        !self.deploying() && self.pending_update.is_none() && self.available_version.is_some()
    }

    /// Start the update a call at `now` asked for, to begin at `starts_at`, or answer
    /// `None` when no update is available — which is the whole eligibility test, so a
    /// caller cannot start one the record does not allow.
    ///
    /// A start at or behind `now` begins the deployment with this call; one still ahead
    /// is recorded as the [`PendingUpdate`] the schedule asked for, which the clock
    /// turns into a deployment when it reaches that instant. Either way the domain keeps
    /// running the release it is on until the deployment lands, and the release it is
    /// moving onto is that deployment's target.
    pub fn begin_update(
        &mut self,
        now: SystemTime,
        starts_at: SystemTime,
    ) -> Option<ServiceSoftwareOptions> {
        if !self.update_available() {
            return None;
        }
        let target = self.available_version.take()?;
        if starts_at <= now {
            self.deployment = Some(SoftwareDeployment {
                target_version: target,
                completes_at: starts_at.checked_add(DOMAIN_WORK).unwrap_or(starts_at),
                rollback: false,
            });
        } else {
            self.pending_update = Some(PendingUpdate {
                target_version: target,
                starts_at,
            });
        }
        Some(self.to_options())
    }

    /// Call off the update this domain has scheduled and not begun, or answer `None`
    /// when there is none to call off — which is the whole eligibility test, so a
    /// caller cannot cancel a deployment the record does not allow cancelling.
    ///
    /// Only an update that has been REQUESTED and has not begun is cancellable (see
    /// [`Self::to_options`]); cancelling it leaves the release it was going to move the
    /// domain onto available to update to again, which is the state the domain was in
    /// before the update was scheduled.
    pub fn cancel_update(&mut self) -> Option<ServiceSoftwareOptions> {
        let pending = self.pending_update.take()?;
        self.available_version = Some(pending.target_version);
        Some(self.to_options())
    }

    /// Whether a service software rollback is available on this domain at `now`: a
    /// deployment has landed leaving an earlier release to return to, its window is
    /// still open, and no deployment is in flight or waiting to begin.
    pub fn rollback_available(&self, now: SystemTime) -> bool {
        !self.deploying()
            && self.pending_update.is_none()
            && self.rollback_to.is_some()
            && self.rollback_until.is_some_and(|until| now < until)
    }

    /// Start the rollback a call at `now` asked for, or answer `None` when no rollback
    /// is available — which is the whole eligibility test, so a caller cannot start one
    /// the record does not allow.
    ///
    /// The rollback is a deployment like any other: the domain keeps running its
    /// current release until the clock makes the deployment due, and the release it is
    /// returning to is the deployment's target.
    pub fn begin_rollback(&mut self, now: SystemTime) -> Option<RollbackServiceSoftwareOptions> {
        if !self.rollback_available(now) {
            return None;
        }
        let target = self.rollback_to.take()?;
        // The window belonged to the deployment being undone; undoing it consumes it.
        self.rollback_until = None;
        self.deployment = Some(SoftwareDeployment {
            target_version: target.clone(),
            completes_at: now + DOMAIN_WORK,
            rollback: true,
        });
        Some(RollbackServiceSoftwareOptions {
            current_version: Some(self.current_version.clone()),
            new_version: Some(target),
            rollback_available: Some(true),
            description: Some(self.description()),
        })
    }

    /// This record's rollback options when no rollback is available: the release the
    /// domain runs, no release to return to, and `RollbackAvailable` false — the
    /// modelled member for "whether a service software rollback is available for the
    /// domain".
    ///
    /// `Description` is "a description of the rollback status", so it names which part
    /// of the rule the record fails: a deployment already running, a window that has
    /// closed, or no landed deployment to undo at all.
    ///
    /// The last of those is the sentence the service itself answers a domain with no
    /// earlier release to return to — the condition every domain is in until a
    /// deployment has landed on it — so it is reproduced as the service words it,
    /// including the direction it gives a caller that believes a rollback was due.
    pub fn rollback_unavailable(&self) -> RollbackServiceSoftwareOptions {
        let description = if self.deploying() {
            deployment_in_progress_sentence(THIS_DOMAIN)
        } else if self.pending_update.is_some() {
            scheduled_update_sentence(THIS_DOMAIN)
        } else if self.rollback_to.is_some() {
            "The rollback window of this domain's last service software update has closed."
                .to_string()
        } else {
            NO_ROLLBACK_TARGET_DESCRIPTION.to_string()
        };
        RollbackServiceSoftwareOptions {
            current_version: Some(self.current_version.clone()),
            // "The service software version that the domain will roll back to": it
            // will roll back to none.
            new_version: None,
            rollback_available: Some(false),
            description: Some(description),
        }
    }

    /// `ServiceSoftwareOptions$UpdateStatus`, one of `DeploymentStatus`'s values:
    /// `IN_PROGRESS` while a deployment is being applied, `PENDING_UPDATE` while one
    /// waits for the instant it was asked to start at, `ELIGIBLE` when a release
    /// the domain can take is available, and `COMPLETED` when the domain is settled on
    /// the release it runs.
    pub fn update_status(&self) -> &'static str {
        if self.deploying() {
            DEPLOYMENT_STATUS_IN_PROGRESS
        } else if self.pending_update.is_some() {
            DEPLOYMENT_STATUS_PENDING_UPDATE
        } else if self.available_version.is_some() {
            DEPLOYMENT_STATUS_ELIGIBLE
        } else {
            DEPLOYMENT_STATUS_COMPLETED
        }
    }

    /// "A description of the service software update status" — derived from the record
    /// rather than stored, so it cannot disagree with the status beside it.
    pub(super) fn description(&self) -> String {
        match (
            &self.deployment,
            &self.pending_update,
            &self.available_version,
        ) {
            (Some(deployment), _, _) if deployment.rollback => format!(
                "Service software rollback to {} is in progress.",
                deployment.target_version
            ),
            (Some(deployment), _, _) => format!(
                "Service software update to {} is in progress.",
                deployment.target_version
            ),
            (None, Some(pending), _) => format!(
                "Service software update to {} is scheduled.",
                pending.target_version
            ),
            (None, None, Some(available)) => {
                format!("Service software update {available} is available.")
            }
            (None, None, None) => no_update_available_sentence(THIS_DOMAIN),
        }
    }

    /// This record as `ServiceSoftwareOptions` — what every full descriptor of a
    /// domain reports and what the service-software operations answer with.
    pub fn to_options(&self) -> ServiceSoftwareOptions {
        ServiceSoftwareOptions {
            current_version: Some(self.current_version.clone()),
            new_version: self
                .deployment
                .as_ref()
                .map(|deployment| deployment.target_version.clone())
                .or_else(|| {
                    self.pending_update
                        .as_ref()
                        .map(|pending| pending.target_version.clone())
                })
                .or_else(|| self.available_version.clone()),
            update_available: Some(self.update_available()),
            // "True if you're able to cancel your service software version update":
            // only a deployment that has been requested and not yet begun can be
            // cancelled, which is an update asked for at an instant still ahead of the
            // call that asked for it.
            cancellable: Some(self.pending_update.is_some()),
            update_status: Some(self.update_status().to_string()),
            description: Some(self.description()),
            // "The timestamp ... until which you can manually request a service
            // software update. After this date, we automatically update your service
            // software." No deployment on a domain this emulator holds is automatic,
            // so there is no such date to report.
            automated_update_date: None,
            optional_deployment: Some(self.optional_deployment),
        }
    }
}

// ---------------------------------------------------------------------------
// Engine version upgrades
//
// `UpgradeDomain` starts a service-managed engine upgrade — or an upgrade
// eligibility check, which performs none — and `GetUpgradeHistory` reports the runs
// a domain has had. [`UpgradeRun`] below is the ONE record both reach: the run in
// flight and the history entry it becomes are the same record, so the two
// operations cannot disagree about what ran, when it started or how far it got.
// ---------------------------------------------------------------------------

/// How many upgrade runs a domain keeps: `UpgradeHistory` is the "history of the
/// last 10 upgrades and upgrade eligibility checks for an Amazon OpenSearch Service
/// domain", and `GetUpgradeHistory` "retrieves the complete history of the last 10
/// upgrades performed on" one — so an eleventh run drops the oldest.
pub const UPGRADE_HISTORY: usize = 10;

/// `UpgradeStep`'s `PRE_UPGRADE_CHECK`, the first of the "three steps that an upgrade
/// or upgrade eligibility check goes through".
pub const UPGRADE_STEP_PRE_UPGRADE_CHECK: &str = "PRE_UPGRADE_CHECK";

/// `UpgradeStep`'s `SNAPSHOT`; see [`UPGRADE_STEP_PRE_UPGRADE_CHECK`].
pub const UPGRADE_STEP_SNAPSHOT: &str = "SNAPSHOT";

/// `UpgradeStep`'s `UPGRADE`; see [`UPGRADE_STEP_PRE_UPGRADE_CHECK`].
pub const UPGRADE_STEP_UPGRADE: &str = "UPGRADE";

/// The steps an upgrade performs, in the order it performs them.
pub(super) const UPGRADE_STEPS: [&str; 3] = [
    UPGRADE_STEP_PRE_UPGRADE_CHECK,
    UPGRADE_STEP_SNAPSHOT,
    UPGRADE_STEP_UPGRADE,
];

/// The steps an upgrade eligibility check performs: the check alone, because such a
/// run "does not actually perform the upgrade" — so neither the snapshot an upgrade
/// takes first nor the upgrade itself runs.
pub(super) const UPGRADE_CHECK_STEPS: [&str; 1] = [UPGRADE_STEP_PRE_UPGRADE_CHECK];

/// `UpgradeStatus`'s `IN_PROGRESS`: the run, or the step, is still being performed.
pub const UPGRADE_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `UpgradeStatus`'s `SUCCEEDED`: the run, or the step, finished.
///
/// The other two values the model admits — `SUCCEEDED_WITH_ISSUES` and `FAILED` —
/// report an upgrade that hit trouble on the real cluster; nothing in this emulator
/// can make an upgrade it accepted fail, so no run is ever reported in them.
pub const UPGRADE_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The progress a finished step reports, and the most any step reports.
pub(super) const STEP_PROGRESS_COMPLETE: u32 = 100;

/// One upgrade, or one upgrade eligibility check, performed on a domain — as this
/// emulator persists it.
///
/// A run is started by `UpgradeDomain` and completes when the span the clock owes it
/// has passed ([`DOMAIN_WORK`], the same span a configuration change takes),
/// materialised by whichever later call reads the record, so nothing sleeps. Its
/// steps are derived from that span rather than stored, so the step a caller polls
/// and the overall status beside it cannot disagree.
#[derive(Debug, Clone)]
pub struct UpgradeRun {
    /// `UpgradeDomainResponse$UpgradeId`: "the unique identifier of the domain
    /// upgrade".
    pub upgrade_id: String,
    /// The engine version the domain ran when this run started.
    pub source_version: String,
    /// The engine version this run moves the domain onto — or, for an eligibility
    /// check, the one it was checked against.
    pub target_version: String,
    /// `UpgradeDomainRequest$PerformCheckOnly`: "when true, indicates that an upgrade
    /// eligibility check needs to be performed. Does not actually perform the
    /// upgrade."
    pub perform_check_only: bool,
    /// The advanced options the upgrade applies to the domain when it lands. An
    /// eligibility check applies none, because it changes nothing.
    pub advanced_options: BTreeMap<String, String>,
    /// `UpgradeHistory$StartTimestamp`: "UTC timestamp at which the upgrade API call
    /// was made".
    pub started_at: SystemTime,
    /// When the clock owes this run completion.
    pub completes_at: SystemTime,
    /// Whether the version this run moves the domain onto has already been
    /// materialised on the domain record, so the landing happens exactly once.
    pub applied: bool,
}

impl UpgradeRun {
    /// The steps this run performs, in order: all three for an upgrade, the check
    /// alone for an eligibility check.
    pub fn steps(&self) -> &'static [&'static str] {
        if self.perform_check_only {
            &UPGRADE_CHECK_STEPS
        } else {
            &UPGRADE_STEPS
        }
    }

    /// Whether this run is still being performed at `now`.
    pub fn in_progress(&self, now: SystemTime) -> bool {
        now < self.completes_at
    }

    /// `UpgradeHistory$UpgradeStatus`: "the current status of the upgrade".
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.in_progress(now) {
            UPGRADE_STATUS_IN_PROGRESS
        } else {
            UPGRADE_STATUS_SUCCEEDED
        }
    }

    /// How long this run has been going at `now`, capped at the span it takes.
    pub(super) fn elapsed(&self, now: SystemTime) -> u128 {
        let total = self.span();
        now.duration_since(self.started_at)
            .map(|elapsed| elapsed.as_millis().min(total))
            .unwrap_or_default()
    }

    /// The span the clock owes this run, in milliseconds. Never zero, so a caller
    /// reading straight after the call that started the run still sees it in
    /// progress.
    pub(super) fn span(&self) -> u128 {
        self.completes_at
            .duration_since(self.started_at)
            .unwrap_or(DOMAIN_WORK)
            .as_millis()
            .max(1)
    }

    /// Each step this run has STARTED by `now`, as its place in [`Self::steps`], its
    /// status and its progress percentage.
    ///
    /// The run's span is divided evenly between its steps, so a caller polling a run
    /// in flight sees the steps advance one after another. A step the run has not
    /// reached is not reported at all: `UpgradeHistory$StepsList` is "a list of each
    /// step performed as part of a specific upgrade or upgrade eligibility check".
    pub(super) fn step_progress(&self, now: SystemTime) -> Vec<(usize, &'static str, u32)> {
        let steps = self.steps();
        let elapsed = self.elapsed(now);
        let span = self.span();
        let count = u128::try_from(steps.len()).unwrap_or(1).max(1);
        (0..steps.len())
            .filter_map(|index| {
                let position = u128::try_from(index).unwrap_or(0);
                let starts_at = span * position / count;
                let ends_at = span * (position + 1) / count;
                if elapsed >= ends_at {
                    return Some((index, UPGRADE_STATUS_SUCCEEDED, STEP_PROGRESS_COMPLETE));
                }
                if elapsed < starts_at {
                    // A step this run has not started performing yet.
                    return None;
                }
                let through = (elapsed - starts_at) * u128::from(STEP_PROGRESS_COMPLETE)
                    / (ends_at - starts_at).max(1);
                Some((
                    index,
                    UPGRADE_STATUS_IN_PROGRESS,
                    u32::try_from(through).unwrap_or(STEP_PROGRESS_COMPLETE),
                ))
            })
            .collect()
    }

    /// The step this run is performing at `now`, and that step's status —
    /// `GetUpgradeStatusResponse`'s `UpgradeStep` and `StepStatus`, "the status of the
    /// current step that an upgrade is on".
    ///
    /// The current step is the LAST one the run has started, which is the last entry of
    /// [`Self::step_progress`] — the same derivation `GetUpgradeHistory` reports every
    /// step from, so the step a caller polls and the step the history lists for the same
    /// run cannot disagree. A finished run is reported on its last step, succeeded.
    pub fn current_step(&self, now: SystemTime) -> Option<(&'static str, &'static str)> {
        let steps = self.steps();
        let (index, status, _) = self.step_progress(now).into_iter().next_back()?;
        Some((*steps.get(index)?, status))
    }

    /// `UpgradeHistory$UpgradeName`: "a string that describes the upgrade".
    ///
    /// The model declares no form for it, so it is derived from the record rather
    /// than stored: which engine versions the run went between, and whether it
    /// performed the upgrade or only checked that it could.
    pub fn upgrade_name(&self) -> String {
        let what = if self.perform_check_only {
            "Upgrade eligibility check"
        } else {
            "Upgrade"
        };
        format!(
            "{what} from {} to {}",
            self.source_version, self.target_version
        )
    }

    /// This run as `UpgradeHistory` — one entry of what `GetUpgradeHistory` answers
    /// with.
    pub fn to_history(&self, now: SystemTime) -> UpgradeHistory {
        let steps = self.steps();
        UpgradeHistory {
            upgrade_name: Some(self.upgrade_name()),
            start_timestamp: Some(datetime(self.started_at)),
            upgrade_status: Some(self.status(now).to_string()),
            steps_list: Some(
                self.step_progress(now)
                    .into_iter()
                    .map(|(index, status, progress)| UpgradeStepItem {
                        upgrade_step: steps.get(index).map(|step| (*step).to_string()),
                        upgrade_step_status: Some(status.to_string()),
                        // "A list of strings containing detailed information about the
                        // errors encountered in a particular step": no step of a run
                        // this emulator performs encounters one.
                        issues: Some(Vec::new()),
                        progress_percent: Some(f64::from(progress)),
                    })
                    .collect(),
            ),
        }
    }
}
