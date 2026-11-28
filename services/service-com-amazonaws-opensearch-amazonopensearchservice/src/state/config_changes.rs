// SPDX-License-Identifier: CC0-1.0

//! Configuration changes of a domain, and the dry runs that validate one.
//!
//! A [`ConfigChange`] is the one record of an accepted `UpdateDomainConfig`:
//! `DescribeDomainChangeProgress` reports its progress, `CancelDomainConfigChange`
//! withdraws it and the domain's own `ChangeProgressDetails` is derived from it. A
//! [`DryRun`] is the same for a change asked about rather than applied, which
//! `DescribeDryRunProgress` reports.

use super::*;

/// One configuration change on a domain: what `DescribeDomainChangeProgress`
/// reports in full and what `DomainStatus`/`DomainConfig` summarise as a
/// `ChangeProgressDetails`.
///
/// A change is minted by the call that starts it (`CreateDomain`, or an
/// `UpdateDomainConfig` that changed at least one option) and completes when the
/// provisioning span the clock owes it has passed — the same instant the domain
/// itself settles, so the change's status and the domain's `Processing` flag can
/// never disagree.
#[derive(Debug, Clone)]
pub struct ConfigChange {
    /// `ChangeProgressStatusDetails$ChangeId`: the GUID this change is addressed by.
    pub change_id: String,
    /// The `DomainConfig` member names this change sets, as the `OPT_*` constants
    /// spell them. Reported as `PendingProperties` while the change is in flight and
    /// as `CompletedProperties` once it has landed.
    pub properties: Vec<String>,
    /// `StartTime`: the instant of the call that started the change.
    pub started_at: SystemTime,
    /// `LastUpdatedTime`: the instant this change's status last moved.
    pub last_updated_at: SystemTime,
    /// When the clock owes this change completion; `None` once it has completed.
    pub completes_at: Option<SystemTime>,
    /// One of `InitiatedBy`'s values; see [`INITIATED_BY_CUSTOMER`].
    pub initiated_by: String,
    /// The option values the domain held BEFORE this change, for a change a cancel
    /// can put back — recorded by [`Domain::record_revert`] and `None` for a change
    /// there is no going back from (the one a create starts, an engine version
    /// upgrade), which is therefore not cancellable.
    pub revert_to: Option<DomainOptions>,
    /// Whether this change was cancelled before it landed.
    pub cancelled: bool,
    /// The stages the deployment of this change goes through, in the order the
    /// service performs them — `ChangeProgressStages`, reported with the
    /// `TotalNumberOfStages` this list is the length of.
    ///
    /// Recorded by [`Domain::begin_change`] for a change the service has BEGUN
    /// applying to a provisioned cluster, and advanced by the clock ([`ChangeStage`]).
    /// EMPTY for a change that deploys nothing: the one a create starts, which waits
    /// for the cluster it would be applied to, and the one a delete records, which
    /// applies no configuration at all.
    pub stages: Vec<ChangeStage>,
    /// Whether this change belongs to the domain's CLONED BLUE/GREEN upgrade — the
    /// `UpgradeDomain` that started it and every `UpdateClone` that managed it since.
    ///
    /// What it decides is the `ClonedBlueGreenUpgradeDetails` a progress read reports: those
    /// details are the state of the upgrade the DOMAIN holds
    /// ([`Domain::cloned_blue_green`]), and they are reported for the changes of that
    /// upgrade and for no others.
    pub cloned_blue_green: bool,
}

impl ConfigChange {
    /// Materialise the completion the clock owes this change, if `now` is past its
    /// due instant.
    ///
    /// The completion is dated at the DUE instant rather than at `now`, so the
    /// `LastUpdatedTime` a caller reads does not move with the moment it happened to
    /// look.
    ///
    /// The STAGES of the change are owed their own movements by the same clock, each
    /// on its own share of the change's window, so they are materialised here too:
    /// what a caller reads about the change and what it reads about the stages of the
    /// change are then always as of the same instant.
    pub(super) fn settle(&mut self, now: SystemTime) {
        self.settle_stages(now);
        let Some(due) = self.completes_at else {
            return;
        };
        if now < due {
            return;
        }
        self.completes_at = None;
        self.last_updated_at = due;
    }

    /// Materialise whatever the clock owes this change's STAGES at `now`: each one is
    /// being worked on once its own share of the change's window has begun and done
    /// once that share has passed ([`ChangeStage::settle`]).
    fn settle_stages(&mut self, now: SystemTime) {
        for stage in &mut self.stages {
            stage.settle(now);
        }
    }

    /// Stop every stage of this change that had not been performed at `now`: the
    /// change was cancelled, so the work those stages stood for is not going to be
    /// performed.
    pub(super) fn fail_stages(&mut self, now: SystemTime) {
        for stage in &mut self.stages {
            stage.fail(now);
        }
    }

    /// Whether this change is still being applied.
    pub fn in_progress(&self) -> bool {
        self.completes_at.is_some()
    }

    /// Whether cancelling this change is something the service can do: it is still
    /// being applied AND the configuration it started from was recorded, so
    /// cancelling it puts that configuration back rather than leaving the change
    /// applied under another name.
    pub fn cancellable(&self) -> bool {
        self.in_progress() && self.revert_to.is_some()
    }

    /// `ChangeProgressStatusDetails$Status`: one of `OverallChangeStatus`'s values.
    pub fn status(&self) -> &'static str {
        if self.in_progress() {
            CHANGE_STATUS_PROCESSING
        } else if self.cancelled {
            CHANGE_STATUS_FAILED
        } else {
            CHANGE_STATUS_COMPLETED
        }
    }

    /// `ConfigChangeStatus` for this change: it is `ApplyingChanges` for as long as the
    /// deployment applying it is under way — until the change falls due — and
    /// `Completed` after, or `Cancelled` where a cancel stopped it before it landed.
    /// Which stage of that deployment it is on is reported beside this, per stage
    /// ([`ChangeStage`]).
    pub fn config_change_status(&self) -> &'static str {
        if self.in_progress() {
            CONFIG_CHANGE_APPLYING_CHANGES
        } else if self.cancelled {
            CONFIG_CHANGE_CANCELLED
        } else {
            CONFIG_CHANGE_COMPLETED
        }
    }

    /// This change as `ChangeProgressStatusDetails` — what
    /// `DescribeDomainChangeProgress` answers with.
    pub fn to_progress_status(&self) -> ChangeProgressStatusDetails {
        // A property of the change in flight is pending; one of a change that has
        // landed is completed. The two lists partition `properties`, so no property
        // is ever reported as both — and a cancelled change landed none of them, so
        // neither list holds any.
        let (pending, completed) = match (self.in_progress(), self.cancelled) {
            (true, _) => (self.properties.clone(), Vec::new()),
            (false, true) => (Vec::new(), Vec::new()),
            (false, false) => (Vec::new(), self.properties.clone()),
        };
        ChangeProgressStatusDetails {
            change_id: Some(self.change_id.clone()),
            start_time: Some(datetime(self.started_at)),
            status: Some(self.status().to_string()),
            pending_properties: Some(pending),
            completed_properties: Some(completed),
            // "The total number of stages required for the configuration change" is the
            // number of stages the deployment applying it goes through
            // ([`CHANGE_STAGES`]) — what it REQUIRES, not how many of them it has
            // published yet: a change that is waiting for the cluster it will be applied
            // to publishes no stage and still reports the count a caller renders progress
            // against. A change that applies no configuration at all — the one a delete
            // records — requires no stage, and the member stays at its own modelled
            // default. The container is reported either way, empty rather than absent, so
            // a caller walking the stages has a list to walk.
            total_number_of_stages: if self.properties.is_empty() {
                0
            } else {
                i32::try_from(STAGE_COUNT).unwrap_or(i32::MAX)
            },
            change_progress_stages: Some(
                self.stages.iter().map(ChangeStage::to_stage).collect(),
            ),
            last_updated_time: Some(datetime(self.last_updated_at)),
            config_change_status: Some(self.config_change_status().to_string()),
            initiated_by: Some(self.initiated_by.clone()),
            // Validation failures come from the pre-update validation check a verbose
            // dry run runs, which reports them on the dry run's OWN record
            // ([`DryRun::to_progress_status`], read by `DescribeDryRunProgress`)
            // rather than on the change a later call applied.
            validation_failures: None,
            // Advisory warnings are accepted through the same verbose dry run.
            accepted_warnings: None,
            // The state of the cloned blue/green upgrade this change belongs to is the
            // DOMAIN's ([`Domain::cloned_blue_green`]), which this record cannot reach:
            // [`crate::service::domain_configuration::reported_change_progress`] is handed
            // both and fills it in.
            cloned_blue_green_upgrade_details: None,
        }
    }

    /// This change as the `ChangeProgressDetails` every full descriptor of the domain
    /// (`DomainStatus`, `DomainConfig`) carries, so the change a descriptor names is
    /// the same change `DescribeDomainChangeProgress` reports.
    pub fn to_details(&self) -> ChangeProgressDetails {
        ChangeProgressDetails {
            change_id: Some(self.change_id.clone()),
            // `Message` is "a message corresponding to the status of the
            // configuration change"; the service's own wording for one is not
            // published, and the status itself is reported in
            // `ConfigChangeStatus`, so none is invented.
            message: None,
            config_change_status: Some(self.config_change_status().to_string()),
            initiated_by: Some(self.initiated_by.clone()),
            start_time: Some(datetime(self.started_at)),
            last_updated_time: Some(datetime(self.last_updated_at)),
        }
    }
}

/// How many verbose dry runs one domain keeps, the same bound and for the same
/// reason as [`CHANGE_HISTORY`]: a caller may ask for one before every update it is
/// considering, so the record set a domain holds has to be bounded.
pub const DRY_RUN_HISTORY: usize = 50;

/// `DryRunMode`'s `Verbose` value (one of [`DRY_RUN_MODE_VALUES`]): the mode that
/// "runs an additional check to validate the changes you're making", which is the
/// analysis `DescribeDryRunProgress` reports the progress of. `Basic` "only returns
/// the type of deployment (blue/green or dynamic) that the update will cause", which
/// the update call answers from its own analysis, so it starts no run and leaves
/// nothing to report on.
pub const DRY_RUN_MODE_VERBOSE: &str = "Verbose";

/// `DryRunProgressStatus$DryRunStatus` for a run whose validation check found the
/// proposed configuration applicable — the terminal value a client polls a dry run to.
///
/// `DryRunStatus` is a free `String` in the model, so its value set is recorded
/// behaviour: the service reports it in LOWERCASE, terminal at this value and at
/// [`DRY_RUN_STATUS_FAILED`].
pub const DRY_RUN_STATUS_SUCCEEDED: &str = "succeeded";

/// `DryRunProgressStatus$DryRunStatus` for a run whose validation check found a
/// failure the change cannot proceed past — the other terminal value.
pub const DRY_RUN_STATUS_FAILED: &str = "failed";

/// One verbose dry run on a domain: the analysis an `UpdateDomainConfig` carrying
/// `DryRun` with `DryRunMode` [`DRY_RUN_MODE_VERBOSE`] started, which
/// `DescribeDryRunProgress` reports the progress, the configuration and the results
/// of.
///
/// A dry run applies NOTHING: it is "a pre-update dry run analysis", so the record
/// holds the configuration the change WOULD have left the domain in — computed by
/// the same merge the real update performs, on a copy of the domain — beside the
/// deployment kind that change would need. The analysis RAN in the call that asked
/// for it — that same answer already carries its `DryRunResults` — so the record it
/// mints is terminal from the start: [`DRY_RUN_STATUS_SUCCEEDED`] for an analysis
/// that found no validation failure and [`DRY_RUN_STATUS_FAILED`] for one that did,
/// and nothing about the run moves afterwards.
#[derive(Debug, Clone)]
pub struct DryRun {
    /// `DryRunProgressStatus$DryRunId`: the GUID this run is addressed by.
    pub dry_run_id: String,
    /// `CreationDate`: the instant of the call that started the run.
    pub started_at: SystemTime,
    /// `UpdateDate`: the instant this run's status last moved — the instant it was
    /// created, because the analysis it reports concluded within that call.
    pub last_updated_at: SystemTime,
    /// `ValidationFailures`: "the validation failures that occurred as a result of the
    /// dry run", as the pre-update check this run performed found them. The status
    /// below is derived from this ONE list, so what the run reports and what it is
    /// cannot disagree.
    pub validation_failures: Vec<ValidationFailure>,
    /// The `DomainStatus` of the configuration the change would leave the domain in
    /// — "details about the changes you're planning to make on the domain", which
    /// `DescribeDryRunProgress` answers as `DryRunConfig`.
    ///
    /// A projection rather than a domain record, because it describes a
    /// configuration that was never applied and never will be by this run: there is
    /// no live domain for it to be re-derived from, so it is held exactly as the
    /// analysis computed it.
    pub config: DomainStatus,
    /// `DryRunResults$DeploymentType`: which of the four documented deployment kinds
    /// applying the change would be, as the update path's own analysis decided.
    pub deployment_type: String,
    /// `AcceptedWarnings`: "the list of advisory warning codes that were accepted
    /// for the configuration change", as the request that started the run carried
    /// them.
    pub accepted_warnings: Vec<String>,
}

impl DryRun {
    /// `DryRunProgressStatus$DryRunStatus` for this run.
    ///
    /// Terminal either way: the pre-update check ran in the call that asked for the
    /// run, so its conclusion is the failures it found — none, and the proposed
    /// configuration is applicable; any, and the change it analysed cannot proceed.
    pub fn status(&self) -> &'static str {
        if self.validation_failures.is_empty() {
            DRY_RUN_STATUS_SUCCEEDED
        } else {
            DRY_RUN_STATUS_FAILED
        }
    }

    /// This run as `DryRunProgressStatus` — what the call that starts it and
    /// `DescribeDryRunProgress` both answer with, so the two can never report one run
    /// differently.
    pub fn to_progress_status(&self) -> DryRunProgressStatus {
        DryRunProgressStatus {
            dry_run_id: self.dry_run_id.clone(),
            dry_run_status: self.status().to_string(),
            creation_date: datetime_text(self.started_at),
            update_date: datetime_text(self.last_updated_at),
            // The failures the pre-update check recorded, which is the same list the
            // status above is derived from.
            validation_failures: Some(self.validation_failures.clone()),
            accepted_warnings: Some(self.accepted_warnings.clone()),
        }
    }

    /// This run as `DryRunResults` — the same answer the call that started it gave,
    /// from the same stored analysis.
    pub fn to_results(&self) -> DryRunResults {
        DryRunResults {
            deployment_type: Some(self.deployment_type.clone()),
            // "A message corresponding to the deployment type": this service's own
            // wording for one is not published, and the deployment kind is already
            // reported in `DeploymentType`, so none is invented.
            message: None,
        }
    }
}
