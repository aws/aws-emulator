// SPDX-License-Identifier: CC0-1.0

//! The bulk-import, dataset-export and enrichment job records of
//! [`super::ComAmazonawsIotsitewiseAwsiotsitewiseState`].

use super::*;

/// One bulk import job, as `CreateBulkImportJob` starts it and
/// `DescribeBulkImportJob` reports it.
///
/// Its STATUS is not stored: it is DERIVED from the instant the job was created
/// ([`Self::status`]), because the lifecycle is owed by the clock rather than by
/// any call. So a reader reports the status the clock has made due without
/// writing to the store, and two readers of one job can never disagree about
/// where it has got to.
///
/// That status is a RECORD of the lifecycle the create owed, not a report on
/// work something performed: no SAMPLE appears because a job ran, since the
/// values a `csv` or `parquet` file holds are not read from it. What an accepted
/// import DOES record is the destination session dataset's data segments, taken
/// from the data this store really holds for the stream each file names
/// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::ingest_data_segments`]), and
/// the video its `mp4` files carry, read from the objects themselves in the
/// emulator's own Amazon S3
/// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::ingest_capture_segments`]). An
/// operation that comes to ingest the rest of a job's files must likewise take
/// them from somewhere this emulator really holds, never synthesise values to
/// match the status reached here.
#[derive(Debug, Clone)]
pub struct BulkImportJobRecord {
    /// The `jobId` this job was minted with ([`new_id`]).
    pub job_id: String,
    /// "The unique name that helps identify the job request", as the create
    /// stated it. One name is held by at most one stored job
    /// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::bulk_import_job_holding_name`]).
    pub job_name: String,
    /// The ARN this job was minted with, in the region the create that started
    /// it was signed for ([`bulk_import_job_arn`]).
    ///
    /// No modelled member of a bulk import job reports an ARN; this one is what
    /// the already-exists refusal of a later create claiming this job's
    /// [`Self::job_name`] points at, so that refusal names the ARN this job
    /// CARRIES rather than one rebuilt from the refused request.
    pub arn: String,
    /// "The ARN of the IAM role that allows IoT SiteWise to read Amazon S3
    /// data". This service neither assumes nor resolves it.
    pub job_role_arn: String,
    /// "The files in the specified Amazon S3 bucket that contain your data",
    /// kept verbatim and in the order the request listed them, so the reader
    /// answers the request's own files. Bounded by
    /// [`MAX_FILES_PER_BULK_IMPORT_JOB`].
    pub files: Vec<File>,
    /// "The Amazon S3 destination where errors associated with the job creation
    /// request are saved". This service writes nothing to it.
    pub error_report_location: ErrorReportLocation,
    /// "Contains the configuration information of a job, such as the file format
    /// used to save data in Amazon S3" — reported only when the create named it.
    pub job_configuration: Option<JobConfiguration>,
    /// The modelled `adaptiveIngestion`, as the create stated it.
    pub adaptive_ingestion: Option<bool>,
    /// The modelled `deleteFilesAfterImport`, as the create stated it. This
    /// service deletes nothing: an import READS the files it takes video from and
    /// removes none of them.
    pub delete_files_after_import: Option<bool>,
    /// "The ID of the session dataset to ingest data into", for a job that
    /// ingests into one; `None` for a job ingesting into time series outside a
    /// workspace.
    pub dataset_id: Option<String>,
    /// The workspace the session dataset lives in — named together with
    /// [`Self::dataset_id`], and `None` for the same job that has no dataset.
    pub workspace_name: Option<String>,
    /// When the job was created, which is the `jobCreationDate` a read reports
    /// and the instant its whole lifecycle is owed from.
    pub created_at: SystemTime,
}

impl BulkImportJobRecord {
    /// The job's status at `now`, and the instant that status fell due.
    ///
    /// ONE place, so [`Self::status`] and [`Self::last_update_date`] cannot
    /// disagree about where the job has got to: the second is simply the instant
    /// the first became true.
    pub(super) fn settled(&self, now: SystemTime) -> (&'static str, SystemTime) {
        let running_at = self.created_at + BULK_IMPORT_JOB_STEP;
        let completed_at = running_at + BULK_IMPORT_JOB_STEP;
        if now >= completed_at {
            (BULK_IMPORT_JOB_COMPLETED, completed_at)
        } else if now >= running_at {
            (BULK_IMPORT_JOB_RUNNING, running_at)
        } else {
            (BULK_IMPORT_JOB_PENDING, self.created_at)
        }
    }

    /// The job's `jobStatus` at `now`: [`BULK_IMPORT_JOB_PENDING`] until the
    /// first [`BULK_IMPORT_JOB_STEP`] has elapsed, [`BULK_IMPORT_JOB_RUNNING`]
    /// until the second has, [`BULK_IMPORT_JOB_COMPLETED`] after that.
    /// `COMPLETED` is terminal, so the status never moves again.
    pub fn status(&self, now: SystemTime) -> &'static str {
        self.settled(now).0
    }

    /// "The date the job was last updated": the instant of the most recent
    /// status transition the clock has made due, which for a job nothing has
    /// moved yet is the instant it was created.
    pub fn last_update_date(&self, now: SystemTime) -> SystemTime {
        self.settled(now).1
    }

    /// Whether this job holds the `jobName` `name`.
    ///
    /// A name is matched exactly as both sides stated it: the modelled
    /// `BulkImportJobName` carries no case-insensitivity, so two names differing
    /// in case are two names.
    pub fn holds_name(&self, name: &str) -> bool {
        self.job_name == name
    }

    /// Whether this job answers a read scoped to `workspace`.
    ///
    /// A read may name a `workspaceName`, which NARROWS it to the jobs of that
    /// workspace: a job created for a session dataset in another workspace, and
    /// one created for no workspace at all, are not reachable through it. A read
    /// that names none is account-wide, so every job answers it.
    pub fn in_workspace(&self, workspace: Option<&str>) -> bool {
        match workspace {
            None => true,
            Some(workspace) => self.workspace_name.as_deref() == Some(workspace),
        }
    }
}

// ---------------------------------------------------------------------------
// The enrichment jobs of a workspace: what `CreateEnrichmentJob` starts and
// `ListEnrichmentJobs` enumerates. `CreateEnrichmentJob` "returns immediately
// with job details while processing continues in the background", so the job's
// STATUS — and the instant of its last status change — are DERIVED from the
// instant it was created, exactly as a dataset export job's and a query's are:
// a read reports what the clock has made due without writing anything, and two
// readers of one job cannot disagree about where it has got to.
//
// A cancellation IS something a call does to a job, so it is RECORDED on the
// record ([`EnrichmentJobRecord::cancelled_at`]) rather than derived, and from
// then on it is the terminal status every reader reports for that job: the
// cancel takes effect at the instant it is recorded, because
// `CancelEnrichmentJob` "responds immediately after recording the
// cancellation" with the status the job then stands in.
//
// What a record does NOT hold is an embedding of any video: nothing in this
// emulator analyses a time series, and no operation of this API answers an
// embedding. An operation that comes to answer the semantic search a completed
// job enables must derive its answer from the store, never synthesise one to
// match the status reached here.
//
// The enrichment STATUS is not an embedding, so it is derived rather than left
// absent: a record names the dataset, the time series and the trim window one
// job analyses, so the dataset that job names and the data segments of it that
// window contains report where that enrichment stands — see
// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::dataset_enrichment`] and
// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::data_segment_enrichment`],
// which the dataset reads answer from, so what a job reports about itself and
// what the resource it was performed on reports cannot disagree.
// ---------------------------------------------------------------------------

/// The `EnrichmentJobStatus` a job reports from the instant it is created:
/// "Initial status of the enrichment job, typically PENDING"
/// (`crate::ops::types::ENRICHMENT_JOB_STATUS_VALUES`).
pub const ENRICHMENT_JOB_PENDING: &str = "PENDING";

/// The value a job reports once the clock has started it processing: "The job
/// will transition to RUNNING when processing begins".
pub const ENRICHMENT_JOB_RUNNING: &str = "RUNNING";

/// The value a job reports once the clock has finished processing it. Terminal.
pub const ENRICHMENT_JOB_COMPLETED: &str = "COMPLETED";

/// The value a job a cancel was recorded on reports from that instant on:
/// "Jobs in PENDING or RUNNING status transition to CANCELLED". Terminal.
pub const ENRICHMENT_JOB_CANCELLED: &str = "CANCELLED";

/// The `JobType` an `eventDetection` configuration makes a job: the type is
/// "derived from the job configuration union member", so it is derived here
/// rather than stored beside the configuration.
pub const JOB_TYPE_EVENT_DETECTION: &str = "EVENT_DETECTION";

/// The `EnrichmentStatus` a data segment a completed enrichment job analysed
/// reports (`crate::ops::types::ENRICHMENT_STATUS_VALUES`).
pub const ENRICHMENT_ENRICHED: &str = "ENRICHED";

/// The value a data segment no completed enrichment job has analysed reports.
pub const ENRICHMENT_NOT_ENRICHED: &str = "NOT_ENRICHED";

/// The `DatasetEnrichmentStatus` a dataset every data segment of which is
/// enriched reports (`crate::ops::types::DATASET_ENRICHMENT_STATUS_VALUES`).
pub const DATASET_FULLY_ENRICHED: &str = "FULLY_ENRICHED";

/// The value a dataset only some of whose data segments are enriched reports.
pub const DATASET_PARTIALLY_ENRICHED: &str = "PARTIALLY_ENRICHED";

/// The value a dataset none of whose data segments is enriched reports.
pub const DATASET_NOT_ENRICHED: &str = "NOT_ENRICHED";

/// How long each step of an enrichment job's lifecycle takes: a created job
/// stays `PENDING` for this long, then reports `RUNNING` for this long, then is
/// `COMPLETED`.
///
/// CHOSEN, not documented: AWS publishes no duration for either step — a job
/// takes as long as the video it analyses. The span is deliberately NON-ZERO,
/// so a caller that lists jobs straight after creating one still observes the
/// transient status the real service reports, and short enough that a caller
/// can poll a job through to its terminal status.
pub const ENRICHMENT_JOB_STEP: Duration = Duration::from_millis(250);

/// How many enrichment jobs one workspace holds.
///
/// CHOSEN: a create is the only thing that adds a job and nothing evicts one,
/// so without a bound a stream of creates would grow one emulator process
/// without limit. `CreateEnrichmentJob` refuses past it with the modelled
/// `LimitExceededException` (HTTP 410), which its own documentation names for
/// "Too many concurrent jobs or requests".
pub const MAX_ENRICHMENT_JOBS_PER_WORKSPACE: usize = 100;

/// How many enrichment jobs the store holds in total, across every workspace.
/// The per-workspace cap cannot bound the store on its own, for the reason
/// [`MAX_DATASET_EXPORT_JOBS`] states.
pub const MAX_ENRICHMENT_JOBS: usize = 1_000;

/// One enrichment job, as `CreateEnrichmentJob` started it.
#[derive(Debug, Clone)]
pub struct EnrichmentJobRecord {
    /// The `jobId` this job was minted with ([`new_id`]).
    pub job_id: String,
    /// The workspace the job was created in. Every operation on a job
    /// addresses it within a workspace, so the pair is the job's identity —
    /// see [`enrichment_job_key`].
    pub workspace_name: String,
    /// The `jobConfiguration` the create carried, kept verbatim: it is what the
    /// job IS, and every other value a reader reports about the job — its type,
    /// the dataset it analyses and the time series it names — is DERIVED from
    /// it rather than copied beside it, so the two cannot drift.
    pub configuration: EnrichmentJobConfiguration,
    /// The `clientToken` the create carried, if any — a record of how that call
    /// was identified. What makes a RETRY of it replay this job is the store's
    /// registry of the tokens an operation has already served
    /// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_verdict`]),
    /// which recognises the CALL and not the job it left behind.
    pub client_token: Option<String>,
    /// When the job was created, which is the `createdAt` a read reports and
    /// the instant its whole lifecycle is owed from.
    pub created_at: SystemTime,
    /// When a cancel was recorded on this job, if one was: the `cancelledAt` a
    /// read reports, and from that instant the terminal status every reader
    /// reports for the job. Held rather than derived, because a cancellation is
    /// something a CALL did to the job and not something the clock owed it.
    pub cancelled_at: Option<SystemTime>,
}

impl EnrichmentJobRecord {
    /// The dataset this job analyses: `EventDetection`'s "IoT SiteWise dataset
    /// ID containing the video time-series data to analyze".
    pub fn dataset_id(&self) -> &str {
        enrichment_job_target(&self.configuration).0
    }

    /// The `propertyAlias` the configuration named, if it identified the time
    /// series that way: "Present when the job was created using a
    /// propertyAlias".
    pub fn property_alias(&self) -> Option<&str> {
        enrichment_job_target(&self.configuration).1
    }

    /// The `timeSeriesId` the configuration named, if it identified the time
    /// series that way: "Present when the job was created using a
    /// timeSeriesId".
    pub fn time_series_id(&self) -> Option<&str> {
        enrichment_job_target(&self.configuration).2
    }

    /// The job's `jobType`, derived from the configuration's union member.
    pub fn job_type(&self) -> &'static str {
        enrichment_job_target(&self.configuration).3
    }

    /// The job's `status` at `now`: `CANCELLED` once a cancel was recorded on
    /// it, otherwise `PENDING` until the first [`ENRICHMENT_JOB_STEP`] has
    /// elapsed, `RUNNING` until the second has, `COMPLETED` after that. A
    /// terminal status never moves again.
    ///
    /// No job of this emulator fails or times out, so `FAILED` and `TIMED_OUT`
    /// — and the `failureMessage` the model documents beside them — are
    /// statuses nothing here reports.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.cancelled_at.is_some() {
            return ENRICHMENT_JOB_CANCELLED;
        }
        match self.completed_at(now) {
            Some(_) => ENRICHMENT_JOB_COMPLETED,
            None if now >= self.created_at + ENRICHMENT_JOB_STEP => ENRICHMENT_JOB_RUNNING,
            None => ENRICHMENT_JOB_PENDING,
        }
    }

    /// The instant the job COMPLETED, and `None` until it has — or at all for a
    /// job a cancel was recorded on, which stopped where that cancel found it
    /// and so never reaches the status this instant records (`completedAt` is
    /// "only present if status is COMPLETED").
    pub fn completed_at(&self, now: SystemTime) -> Option<SystemTime> {
        if self.cancelled_at.is_some() {
            return None;
        }
        let completes_at = self.created_at + 2 * ENRICHMENT_JOB_STEP;
        (now >= completes_at).then_some(completes_at)
    }

    /// "Timestamp of the last job status change": the instant a recorded
    /// cancellation took the job to its terminal status, else the instant of the
    /// latest transition the clock has made due, and `None` for a job still in
    /// the status its create answered with, which nothing has moved yet.
    pub fn updated_at(&self, now: SystemTime) -> Option<SystemTime> {
        if let Some(cancelled_at) = self.cancelled_at {
            return Some(cancelled_at);
        }
        let runs_at = self.created_at + ENRICHMENT_JOB_STEP;
        self.completed_at(now)
            .or_else(|| (now >= runs_at).then_some(runs_at))
    }

    /// Whether the trim window this job analyses contains the half-open span
    /// `[start, end)` WHOLE.
    ///
    /// "Trim settings retrieve fully contained data segments within the
    /// specified time range", so a segment reaching outside the window — before
    /// its `startTime` or past its `endTime` — is not one this job analysed,
    /// however much of it the window covers.
    pub fn analyses_span(&self, start: &TimeInNanos, end: &TimeInNanos) -> bool {
        let (window_start, window_end) = enrichment_job_window(&self.configuration);
        timestamp_key(window_start) <= timestamp_key(start)
            && timestamp_key(end) <= timestamp_key(window_end)
    }

    /// Whether the status this job reports at `now` is one it can still move
    /// out of — what the duplicate-job rule reads, and what decides whether a
    /// cancel may still be recorded on the job.
    pub fn is_active(&self, now: SystemTime) -> bool {
        self.cancelled_at.is_none() && self.completed_at(now).is_none()
    }

    /// Record a cancellation on this job at `now`. The cancel takes effect from
    /// that instant: "the API responds immediately after recording the
    /// cancellation", with `CANCELLED` as the status it answers.
    pub fn record_cancel(&mut self, now: SystemTime) {
        self.cancelled_at = Some(now);
    }

    /// This job as the summary a listing reports for it, at `now`.
    ///
    /// Derived here rather than in the handler so a listing and any later read
    /// of one stored job cannot report different statuses or instants for it.
    pub fn summary(&self, now: SystemTime) -> EnrichmentJobSummary {
        EnrichmentJobSummary {
            job_id: self.job_id.clone(),
            status: self.status(now).to_string(),
            workspace_name: self.workspace_name.clone(),
            job_type: self.job_type().to_string(),
            dataset_id: self.dataset_id().to_string(),
            property_alias: self.property_alias().map(str::to_string),
            time_series_id: self.time_series_id().map(str::to_string),
            created_at: to_date_time(self.created_at),
            updated_at: self.updated_at(now).map(to_date_time),
        }
    }
}

/// The key [`ComAmazonawsIotsitewiseAwsiotsitewiseState::enrichment_jobs`]
/// holds a job under: the workspace it was created in and its own id, because
/// every operation on a job addresses it within a workspace, so a job of one
/// workspace is not reachable through another.
pub fn enrichment_job_key(workspace_name: &str, job_id: &str) -> (String, String) {
    (workspace_name.to_string(), job_id.to_string())
}

/// What identifies the WORK one enrichment job does, as the duplicate-job rule
/// compares two jobs: the dataset, the time series by whichever identifier the
/// configuration named, and the job type.
///
/// Also where every per-job value a reader derives comes from, so the record's
/// accessors and that rule read ONE definition of the configuration's shape.
pub fn enrichment_job_target(
    configuration: &EnrichmentJobConfiguration,
) -> (&str, Option<&str>, Option<&str>, &'static str) {
    match configuration {
        EnrichmentJobConfiguration::eventDetection(detection) => (
            detection.dataset_id.as_str(),
            detection.property_alias.as_deref(),
            detection.time_series_id.as_deref(),
            JOB_TYPE_EVENT_DETECTION,
        ),
    }
}

/// The trim window one enrichment job analyses: the `EnrichmentTrimSettings`
/// its configuration carries, read off that configuration rather than copied
/// beside it, exactly as [`enrichment_job_target`] reads the work it does.
pub fn enrichment_job_window(
    configuration: &EnrichmentJobConfiguration,
) -> (&TimeInNanos, &TimeInNanos) {
    match configuration {
        EnrichmentJobConfiguration::eventDetection(detection) => (
            &detection.trim_settings.start_time,
            &detection.trim_settings.end_time,
        ),
    }
}

/// Refuse an `EnrichmentJobConfiguration` whose members break a rule its own
/// shapes state, in the wording the refusal carries; `Ok` for one that keeps
/// them.
///
/// The rules come from the shapes themselves: `EventDetection`'s time series is
/// identified by one of two members ("Specify either timeSeriesId or
/// propertyAlias, but not both"), and an `EnrichmentTrimSettings` end time
/// "must be greater than startTime".
pub fn check_enrichment_job_configuration(
    configuration: &EnrichmentJobConfiguration,
) -> Result<(), String> {
    match configuration {
        EnrichmentJobConfiguration::eventDetection(detection) => {
            if detection.time_series_id.is_some() && detection.property_alias.is_some() {
                return Err("jobConfiguration.eventDetection: timeSeriesId and propertyAlias \
                            are mutually exclusive."
                    .to_string());
            }
            if timestamp_key(&detection.trim_settings.end_time)
                <= timestamp_key(&detection.trim_settings.start_time)
            {
                return Err("jobConfiguration.eventDetection.trimSettings.endTime must be \
                            greater than trimSettings.startTime."
                    .to_string());
            }
            Ok(())
        }
    }
}

/// The resource kind a `ResourceNotFoundException` names for an enrichment job,
/// under the one form [`not_found_message`] states every absence in.
///
/// Declared here, in this section, rather than beside the other resource kinds:
/// an enrichment job is addressed by `DescribeEnrichmentJob` and
/// `CancelEnrichmentJob` alone, so the label belongs with the record those
/// operations read.
pub const RESOURCE_ENRICHMENT_JOB: &str = "Enrichment Job";

/// The ARN segment an enrichment job is named by in the places this API reports
/// a job's ARN: the required `resourceArn` of the conflict a duplicate create,
/// or a cancel of a job that already reached a terminal status, is refused with.
///
/// No modelled member of an enrichment job carries an ARN, so the form is this
/// service's own — the `<type>/<id>` resource form every IoT SiteWise ARN
/// takes, as [`parse_resource_arn`] reads them back.
pub const ENRICHMENT_JOB_RESOURCE_TYPE: &str = "enrichment-job";

/// The ARN of an enrichment job in `region`; see
/// [`ENRICHMENT_JOB_RESOURCE_TYPE`].
pub fn enrichment_job_arn(region: &str, job_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ENRICHMENT_JOB_RESOURCE_TYPE}/{job_id}"),
    )
}

/// The ARN segment a BULK IMPORT job is named by, for the reason
/// [`ENRICHMENT_JOB_RESOURCE_TYPE`] states for the other job kind: no modelled
/// member of a bulk import job carries an ARN, so the form is this service's own
/// — the `<type>/<id>` resource form every IoT SiteWise ARN takes, as
/// [`parse_resource_arn`] reads them back.
pub const BULK_IMPORT_JOB_RESOURCE_TYPE: &str = "bulk-import-job";

/// The ARN of a bulk import job in `region`; see
/// [`BULK_IMPORT_JOB_RESOURCE_TYPE`].
pub fn bulk_import_job_arn(region: &str, job_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{BULK_IMPORT_JOB_RESOURCE_TYPE}/{job_id}"),
    )
}

/// How many spent idempotency tokens the store remembers.
///
/// CHOSEN: a mutating call that carries a `clientToken` adds an entry and nothing
/// else removes one, so without a bound a stream of tokens would grow one emulator
/// process without limit. Past the cap the LEAST RECENTLY SPENT token is forgotten
/// first, so the tokens a client could still be retrying are the ones kept.
pub const MAX_IDEMPOTENT_CALLS: usize = 1_000;

/// What one mutating call did, remembered under the `clientToken` it carried, so a
/// retry of that call is answered from it instead of running a second time.
#[derive(Debug, Clone)]
pub struct IdempotentCall {
    /// The request the token was spent on, as [`request_fingerprint`] renders it:
    /// what tells a RETRY of that call from a different call reusing its token.
    pub request: String,
    /// The resource the call acted on — the id a create minted, or the id the
    /// request addressed — which is what its operation replays its answer from.
    pub resource_id: String,
    /// When the token was spent, which is what the bound above evicts by.
    pub spent_at: SystemTime,
    /// The answer a BATCH data-segment call gave, for the operations whose answer
    /// their own writes make unreadable afterwards: the per-entry outcome of a
    /// batch is not a property of the dataset the batch wrote to, so a retry of
    /// one has nothing in the store to rebuild it from. `None` for every
    /// operation whose answer is read back from the resource it acted on.
    ///
    /// BOUNDED by the request that produced it: one answer carries at most one
    /// failure per entry, and an entry list is already capped — by
    /// [`MAX_DATA_SEGMENTS_PER_DATASET`] for the association forms and by the
    /// model's own `@length` for the deletion form — so the memory the
    /// idempotency record holds is bounded by that cap times
    /// [`MAX_IDEMPOTENT_CALLS`].
    pub batch_answer: Option<DataSegmentBatchAnswer>,
}

/// One entry a batch data-segment call could not apply, as the three batch
/// data-segment operations each report it: the segment the entry addressed plus
/// the code and the message saying why.
///
/// One struct for all three, because their modelled failure shapes carry the same
/// members — `sourceDatasetId` only where the operation's entries name a source —
/// and each handler maps it back into its own.
#[derive(Debug, Clone)]
pub struct DataSegmentFailure {
    /// The dataset the segment's data came from, for the operations whose entries
    /// name one.
    pub source_dataset_id: Option<String>,
    pub time_series_id: String,
    pub start_timestamp: TimeInNanos,
    pub end_timestamp: TimeInNanos,
    pub error_code: String,
    pub error_message: String,
}

/// What one batch data-segment call answered: the dataset it addressed, the
/// version that dataset reported after the call, and the entries it could not
/// apply — the whole of what a retry of that call replays.
#[derive(Debug, Clone)]
pub struct DataSegmentBatchAnswer {
    pub dataset_id: String,
    pub dataset_version: String,
    pub failures: Vec<DataSegmentFailure>,
}

/// What the `clientToken` a mutating request carries means for that request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenVerdict {
    /// Serve the request: it carries no token, or none this store has seen.
    Serve,
    /// The SAME request already ran under this token, so this one is a retry of
    /// it: answer from the resource named here and write nothing.
    Replay(String),
    /// A DIFFERENT request — this operation, this token, other members — already
    /// spent this token, which names the resource that earlier call acted on.
    /// Such a request repeats no call, so what it gets depends on what the
    /// operation can say: one whose error list declares the modelled conflict
    /// refuses it, naming that resource, and one that declares none carries it
    /// out as the request it is, because its answer would otherwise assert a
    /// change nothing recorded.
    Spent(String),
}

/// The key [`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_calls`] holds
/// a spent token under: the operation and the token, because a token is scoped to
/// the operation it was sent to — one value sent to two operations is two tokens.
pub fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}/{client_token}")
}

/// A canonical rendering of the members a mutating request is recognised by, so
/// that a retry of it renders identically and any other request does not.
///
/// `members` are rendered in the order given — each through [`std::fmt::Debug`],
/// so a list or a nested structure renders in full — and `tags` from its pairs
/// SORTED, because a map has no order of its own and this rendering must not
/// depend on one. A request carrying no tags renders like one carrying an empty
/// set, which is the same thing it asks for.
pub fn request_fingerprint(
    members: &[&dyn ::std::fmt::Debug],
    tags: Option<&HashMap<String, String>>,
) -> String {
    let mut rendered: Vec<String> = members.iter().map(|member| format!("{member:?}")).collect();
    let mut pairs: Vec<String> = tags
        .into_iter()
        .flatten()
        .map(|(key, value)| format!("{key:?}={value:?}"))
        .collect();
    pairs.sort();
    rendered.push(pairs.join(","));
    // A unit separator: no member of this API may carry one (every string shape
    // that reaches here forbids the C0 controls), so no rendering of two members
    // can be read as a rendering of one.
    rendered.join("\u{1f}")
}

/// How a member the request did not carry renders in a [`request_fingerprint`].
/// No rendering of a member it DID carry can collide with it: those are `Debug`
/// forms, which start with a bracket, a quote or a member name.
pub const FINGERPRINT_ABSENT: &str = "<absent>";

/// A canonical rendering of a `portalTypeConfiguration` a request carried, for
/// the [`request_fingerprint`] of that request.
///
/// The configuration is a MAP, which has no order of its own, so its entries go
/// in SORTED by key: a retry of one call has to render identically to it whatever
/// order either request's map happens to iterate in. Shared by the operations
/// that take one, so one configuration renders one way.
pub fn portal_type_configuration_fingerprint(
    configuration: Option<&HashMap<String, PortalTypeEntry>>,
) -> String {
    match configuration {
        None => FINGERPRINT_ABSENT.to_string(),
        Some(configuration) => {
            let mut entries: Vec<(&String, &PortalTypeEntry)> = configuration.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            format!("{entries:?}")
        }
    }
}

/// A canonical and BOUNDED rendering of an uploaded image FILE, for the
/// [`request_fingerprint`] of the request that carried it.
///
/// The image's own BYTES are not rendered: `ImageFile$data` admits up to 1.5 MB
/// and a fingerprint is kept for as long as the token it was spent under, so an
/// uploaded file goes in as the SHA-256 digest of its contents beside its file
/// type. Two different files still render differently and the same file renders
/// identically, which is all a retry has to be told apart by. Shared by every
/// operation that takes an image, so one upload renders one way.
pub fn image_file_fingerprint(file: Option<&ImageFile>) -> String {
    match file {
        None => FINGERPRINT_ABSENT.to_string(),
        Some(file) => {
            let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(&file.data);
            format!("{}:{}", file.r#type, ::hex::encode(digest))
        }
    }
}

/// A canonical rendering of a string MAP a request carried — the environment
/// variables a pipeline, a compute node or a task declares — for the
/// [`request_fingerprint`] of that request.
///
/// A map has no order of its own, so its pairs go in SORTED by key: a retry of
/// one call has to render identically to it whatever order either request's map
/// happens to iterate in. Shared by every operation that takes one, so one map
/// renders one way.
pub fn string_map_fingerprint(map: Option<&HashMap<String, String>>) -> String {
    match map {
        None => FINGERPRINT_ABSENT.to_string(),
        Some(map) => {
            let mut pairs: Vec<(&String, &String)> = map.iter().collect();
            pairs.sort_by_key(|(key, _)| *key);
            format!("{pairs:?}")
        }
    }
}

/// A canonical rendering of the `computations` list a request carried, for the
/// [`request_fingerprint`] of that request.
///
/// The list keeps the ORDER the request stated it in — a list is ordered, so two
/// orderings are two requests — while each node's own environment variables go
/// in through [`string_map_fingerprint`], because a map has no order of its own.
pub fn compute_nodes_fingerprint(nodes: &[ComputeNode]) -> String {
    nodes
        .iter()
        .map(|node| {
            format!(
                "{:?}/{:?}/{:?}/{}",
                node.compute_node_name,
                node.task_name,
                node.depends_on,
                string_map_fingerprint(node.environment_variables.as_ref()),
            )
        })
        .collect::<Vec<String>>()
        .join(",")
}

/// A canonical rendering of the `executionEnvironmentVariableOverrides` a
/// request carried, for the [`request_fingerprint`] of that request.
///
/// The global overrides go in through [`string_map_fingerprint`] and the
/// per-compute-node ones as their node names SORTED, each node's own map through
/// that same helper: both members are maps, which have no order of their own, so
/// a retry of one call has to render identically to it whatever order either
/// request's maps happen to iterate in.
///
/// A `computeNodes` map the request omitted renders like one it sent empty,
/// because both override no node at all and so ask for the same run.
pub fn execution_environment_variables_fingerprint(
    overrides: Option<&ExecutionEnvironmentVariables>,
) -> String {
    match overrides {
        None => FINGERPRINT_ABSENT.to_string(),
        Some(given) => {
            let mut nodes: Vec<(&String, String)> = given
                .compute_nodes
                .iter()
                .flatten()
                .map(|(node, variables)| (node, string_map_fingerprint(Some(variables))))
                .collect();
            nodes.sort_by_key(|(node, _)| *node);
            format!(
                "{}/{nodes:?}",
                string_map_fingerprint(given.global.as_ref())
            )
        }
    }
}

/// A canonical rendering of the `executionMountOverrides` a request carried, for
/// the [`request_fingerprint`] of that request.
///
/// The compute nodes it names go in SORTED, because it addresses them through a
/// map; each node's own overrides keep the ORDER the request stated them in,
/// because a list is ordered and two orderings are two requests.
pub fn mount_overrides_fingerprint(overrides: Option<&MountOverrides>) -> String {
    match overrides {
        None => FINGERPRINT_ABSENT.to_string(),
        Some(given) => {
            let mut nodes: Vec<(&String, &Vec<Mount>)> = given.compute_nodes.iter().collect();
            nodes.sort_by_key(|(node, _)| *node);
            format!("{nodes:?}")
        }
    }
}

/// A canonical rendering of a `taskConfiguration` a request carried, for the
/// [`request_fingerprint`] of that request.
///
/// Every member but the environment variables renders through
/// [`std::fmt::Debug`]; those go in through [`string_map_fingerprint`], because
/// a map has no order of its own. Shared by every operation that takes one, so
/// one configuration renders one way.
pub fn task_configuration_fingerprint(configuration: &TaskConfiguration) -> String {
    match configuration {
        TaskConfiguration::containerTaskConfiguration(container) => format!(
            "containerTaskConfiguration({:?},{:?},{:?},{:?},{:?},{:?},{:?},{:?},{})",
            container.ecr_uri,
            container.task_execution_role,
            container.processing_type,
            container.processing_unit,
            container.ephemeral_storage_configuration,
            container.command,
            container.timeout_seconds,
            container.mounts,
            string_map_fingerprint(container.environment_variables.as_ref()),
        ),
    }
}

/// How a resource addressed by `(workspaceName, name)` — a pipeline, a task —
/// is NAMED in the idempotency registry, which remembers one string per spent
/// token: the resource's own name is unique only within its workspace, so the
/// workspace goes in beside it.
///
/// Both names target shapes bounded by `^[a-zA-Z0-9_-]+$`, so the `/` between
/// them occurs in neither and one rendering reads back as exactly one pair.
pub fn workspace_scoped_token_resource(workspace_name: &str, resource_name: &str) -> String {
    format!("{workspace_name}/{resource_name}")
}

/// The `(workspaceName, name)` pair a [`workspace_scoped_token_resource`]
/// rendering names — the key the resource is stored under.
///
/// A string carrying no separator names no workspace, so it resolves to no
/// stored resource while still naming what the registry remembers.
pub fn workspace_scoped_token_key(resource: &str) -> (String, String) {
    match resource.split_once('/') {
        Some((workspace_name, resource_name)) => {
            (workspace_name.to_string(), resource_name.to_string())
        }
        None => (String::new(), resource.to_string()),
    }
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The bulk import job that already holds the `jobName` `name`, or `None`
    /// when no stored job does.
    ///
    /// `jobName` is documented "The unique name that helps identify the job
    /// request", so a name one job holds is not free for another: a create may
    /// not take it, the same one-name-one-resource rule this service applies to
    /// every name member it documents as unique.
    ///
    /// Answers the RECORD, so the refusal names the holder by the id and the ARN
    /// that job carries rather than by anything rebuilt from the refused
    /// request.
    pub fn bulk_import_job_holding_name(&self, name: &str) -> Option<&BulkImportJobRecord> {
        self.bulk_import_jobs
            .values()
            .find(|record| record.holds_name(name))
    }

    /// What `operation` is to do with the request whose `clientToken` is `token`
    /// and whose identifying members render as `request`.
    ///
    /// A pure READ: it records nothing, so asking does not spend a token.
    pub fn idempotent_verdict(
        &self,
        operation: &str,
        token: Option<&str>,
        request: &str,
    ) -> TokenVerdict {
        let Some(token) = token else {
            return TokenVerdict::Serve;
        };
        match self.idempotent_calls.get(&idempotency_key(operation, token)) {
            Some(call) if call.request == request => TokenVerdict::Replay(call.resource_id.clone()),
            Some(call) => TokenVerdict::Spent(call.resource_id.clone()),
            None => TokenVerdict::Serve,
        }
    }

    /// Remember that `operation` served `request` under `token`, acting on
    /// `resource_id`, so a retry of that call replays this answer.
    ///
    /// Call it only where the call SUCCEEDED: a refused request changed nothing,
    /// so the token it carried was not spent and the caller may send it again.
    /// A request carrying no token records nothing.
    pub fn record_idempotent_call(
        &mut self,
        operation: &str,
        token: Option<&str>,
        request: String,
        resource_id: &str,
        now: SystemTime,
    ) {
        self.remember_idempotent_call(operation, token, request, resource_id, None, now);
    }

    /// [`Self::record_idempotent_call`] for a batch data-segment call, which
    /// remembers the `answer` it gave as well: its per-entry outcome is no
    /// property of the dataset it wrote to, so a retry has nothing else to
    /// rebuild that answer from.
    pub fn record_idempotent_batch_call(
        &mut self,
        operation: &str,
        token: Option<&str>,
        request: String,
        answer: DataSegmentBatchAnswer,
        now: SystemTime,
    ) {
        let resource_id = answer.dataset_id.clone();
        self.remember_idempotent_call(operation, token, request, &resource_id, Some(answer), now);
    }

    /// The answer the batch data-segment call `operation` gave under `token`, if
    /// this store still remembers it.
    pub fn idempotent_batch_answer(
        &self,
        operation: &str,
        token: Option<&str>,
    ) -> Option<&DataSegmentBatchAnswer> {
        self.idempotent_calls
            .get(&idempotency_key(operation, token?))?
            .batch_answer
            .as_ref()
    }

    pub(super) fn remember_idempotent_call(
        &mut self,
        operation: &str,
        token: Option<&str>,
        request: String,
        resource_id: &str,
        batch_answer: Option<DataSegmentBatchAnswer>,
        now: SystemTime,
    ) {
        let Some(token) = token else {
            return;
        };
        let key = idempotency_key(operation, token);
        // Bounded by [`MAX_IDEMPOTENT_CALLS`], evicting the token spent longest
        // ago; re-recording a key already held replaces it and needs no room.
        while !self.idempotent_calls.contains_key(&key)
            && self.idempotent_calls.len() >= MAX_IDEMPOTENT_CALLS
        {
            let Some(oldest) = self
                .idempotent_calls
                .iter()
                .min_by_key(|(_, call)| call.spent_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.idempotent_calls.remove(&oldest);
        }
        self.idempotent_calls.insert(
            key,
            IdempotentCall {
                request,
                resource_id: resource_id.to_string(),
                spent_at: now,
                batch_answer,
            },
        );
    }
}
