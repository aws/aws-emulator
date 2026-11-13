// SPDX-License-Identifier: CC0-1.0

//! The stored job records, their bookmarks and the runs started of them, with the capacity,
//! glue-version and timeout defaults a create applies.

use super::*;

/// `JobCommand$Name` values the model documents: "For an Apache Spark ETL job, this
/// must be `glueetl`. For a Python shell job, it must be `pythonshell`. For an Apache
/// Spark streaming ETL job, this must be `gluestreaming`. For a Ray job, this must be
/// `glueray`." A command naming anything else is not a job this service can run.
pub const JOB_COMMAND_NAMES: [&str; 4] = ["glueetl", "pythonshell", "gluestreaming", "glueray"];

/// The `JobMode` stored for a create that named none: "When the JobMode field is
/// missing or null, SCRIPT is assigned as the default value" (`CreateJob$JobMode`).
pub const DEFAULT_JOB_MODE: &str = "SCRIPT";

/// The `GlueVersion` stored for a create that named none, which is the version the service
/// runs such a job on and reports from every read of it.
///
/// `CreateJob$GlueVersion` states the rule — a job created without a Glue version is given
/// one — and names the version that was current when the model was published ("Jobs that are
/// created without specifying a Glue version default to Glue 5.1"). The version filled in is
/// whatever the service's current default is, and that is what it answers with; a create
/// that names a version keeps the one it named, whichever it is.
pub const DEFAULT_GLUE_VERSION: &str = "6.0";

/// The `JobRunQueuingEnabled` a definition — or a run — that named none carries: "A value
/// of true means job run queuing is enabled for the job runs. If false or not populated, the
/// job runs will not be considered for queueing" (`Job$JobRunQueuingEnabled`). "Not
/// populated" and `false` are the same behaviour, so the flag the service applies is always
/// known and every read reports it rather than leaving a client to guess.
pub const DEFAULT_JOB_RUN_QUEUING_ENABLED: bool = false;

/// The log group a run of a Python shell job writes into, and the one every other job's run
/// writes into: `JobRun$LogGroupName` is "the name of the log group for secure logging …
/// This name can be `/aws-glue/jobs/`", which is the group Spark job runs log under, while a
/// Python shell job (`JobCommand$Name` [`PYTHON_SHELL_COMMAND`]) logs under its own group.
///
/// Those two are the groups the member's own documentation and the recorded reads of a run
/// state; no recorded read and no documented sentence names a separate group for a Ray job's
/// runs, so a run of one reports the group the member documents rather than a third name this
/// emulator would have to invent.
pub const SPARK_JOB_LOG_GROUP: &str = "/aws-glue/jobs";
/// See [`SPARK_JOB_LOG_GROUP`].
pub const PYTHON_SHELL_JOB_LOG_GROUP: &str = "/aws-glue/python-jobs";

/// The log group a run of this job writes into: the group its COMMAND logs under
/// ([`SPARK_JOB_LOG_GROUP`], [`PYTHON_SHELL_JOB_LOG_GROUP`]), and — for a run that carries a
/// security configuration — that group with the role and configuration names appended, which
/// is the form the member documents for an encrypted group: "If you add a role name and
/// `SecurityConfiguration` name (in other words,
/// `/aws-glue/jobs-yourRoleName-yourSecurityConfigurationName/`), then that security
/// configuration is used to encrypt the log group."
///
/// `role` is the job's `Role`, read through this crate's ONE reading of that member
/// ([`glue_role_reference`]), so the name appended is the one IAM holds the role under
/// whichever form the request named it in.
pub fn job_run_log_group(
    command: &JobCommand,
    role: &str,
    security_configuration: Option<&str>,
) -> String {
    let base = match command.name.as_deref() {
        Some(PYTHON_SHELL_COMMAND) => PYTHON_SHELL_JOB_LOG_GROUP,
        _ => SPARK_JOB_LOG_GROUP,
    };
    match security_configuration {
        Some(configuration) => format!(
            "{base}-{}-{configuration}",
            glue_role_reference(role).name
        ),
        None => base.to_string(),
    }
}

/// `ExecutionProperty$MaxConcurrentRuns` for a job whose create named no execution
/// property: "The maximum number of concurrent runs allowed for the job. The default
/// is 1." The model's `@default` of 0 is what an ABSENT integer deserializes to, not a
/// concurrency a job can run at.
pub const DEFAULT_MAX_CONCURRENT_RUNS: i32 = 1;

/// `Job$Timeout` when the create left it blank, for Glue 4.0 and earlier: "When the
/// value is left blank, the timeout is defaulted to 2,880 minutes for Glue version 4.0
/// and earlier, or 480 minutes for Glue version 5.0 and later."
pub const DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4: i32 = 2_880;

/// `Job$Timeout` when the create left it blank, for Glue 5.0 and later — the second
/// half of the sentence quoted on [`DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4`].
pub const DEFAULT_TIMEOUT_MINUTES_FROM_GLUE_5: i32 = 480;

/// The largest `Timeout` a job may carry: "Jobs must have timeout values less than 7
/// days or 10080 minutes. Otherwise, the jobs will throw an exception." (`Job$Timeout`)
pub const MAX_TIMEOUT_MINUTES: i32 = 10_080;

/// The `JobCommand$Name` of an Apache Spark ETL job, which is the only command
/// `ExecutionClass=FLEX` may be set on, and the `ExecutionClass` that restriction is
/// about: "Only jobs with Glue version 3.0 and above and command type `glueetl` will be
/// allowed to set `ExecutionClass` to `FLEX`." (`CreateJob$ExecutionClass`.)
pub const SPARK_ETL_COMMAND: &str = "glueetl";
/// See [`SPARK_ETL_COMMAND`].
pub const EXECUTION_CLASS_FLEX: &str = "FLEX";
/// The lowest `GlueVersion` that sentence admits `FLEX` on.
pub const MIN_FLEX_GLUE_VERSION: f64 = 3.0;

/// Whether a job's command and Glue version are the pair `ExecutionClass=FLEX` is allowed
/// on, by the sentence quoted on [`SPARK_ETL_COMMAND`].
///
/// A `GlueVersion` this cannot read as a number (the model's pattern `^(\w+\.)+\w+$` is
/// wider than a decimal) admits nothing: the restriction is stated over a version
/// comparison, so a version that cannot be compared does not satisfy it.
pub fn admits_flex_execution_class(command: &JobCommand, glue_version: &str) -> bool {
    let spark_etl = command.name.as_deref() == Some(SPARK_ETL_COMMAND);
    let version_admits = glue_version
        .parse::<f64>()
        .is_ok_and(|version| version >= MIN_FLEX_GLUE_VERSION);
    spark_etl && version_admits
}

/// The first Glue major version that takes [`DEFAULT_TIMEOUT_MINUTES_FROM_GLUE_5`].
pub(super) const FIRST_GLUE_MAJOR_VERSION_WITH_SHORT_DEFAULT_TIMEOUT: u32 = 5;

/// The capacity, in DPUs, a Spark job whose definition names none runs with: "When you
/// specify an Apache Spark ETL job (`JobCommand.Name`="glueetl") or Apache Spark
/// streaming ETL job (`JobCommand.Name`="gluestreaming"), you can allocate from 2 to 100
/// DPUs. The default is 10 DPUs." (`CreateJob$MaxCapacity`.)
pub const DEFAULT_SPARK_DPUS: f64 = 10.0;

/// The capacity, in DPUs, a Python shell job whose definition names none runs with:
/// "When you specify a Python shell job (`JobCommand.Name`="pythonshell"), you can
/// allocate either 0.0625 or 1 DPU. The default is 0.0625 DPU." (`CreateJob$MaxCapacity`.)
pub const DEFAULT_PYTHON_SHELL_DPUS: f64 = 0.0625;

/// The `WorkerType` a Spark job whose definition names no worker configuration runs
/// with, and the number of such workers.
///
/// From Glue 2.0 on, a Spark job's capacity is expressed as a worker type and a worker
/// count rather than as a DPU figure — "For Glue version 2.0 or later jobs, you cannot
/// specify a `Maximum capacity`. Instead, you should specify a `Worker type` and the
/// `Number of workers`" (`CreateJob$MaxCapacity`) — so the documented default of
/// [`DEFAULT_SPARK_DPUS`] is carried as that many `G.1X` workers, each of which "maps to
/// 1 DPU" (`CreateJob$WorkerType`).
pub const DEFAULT_SPARK_WORKER_TYPE: &str = "G.1X";
/// See [`DEFAULT_SPARK_WORKER_TYPE`]: ten one-DPU workers are the ten DPUs
/// `CreateJob$MaxCapacity` documents as a Spark job's default.
pub const DEFAULT_SPARK_WORKERS: i32 = 10;

/// The lowest `GlueVersion` whose jobs express capacity as a worker configuration, by the
/// sentence quoted on [`DEFAULT_SPARK_WORKER_TYPE`].
pub const MIN_WORKER_TYPE_GLUE_VERSION: f64 = 2.0;

/// The `JobCommand$Name` of an Apache Spark STREAMING ETL job, the second of the two
/// commands [`DEFAULT_SPARK_DPUS`] is documented for.
pub const SPARK_STREAMING_COMMAND: &str = "gluestreaming";

/// The `JobCommand$Name` of a Python shell job, the command [`DEFAULT_PYTHON_SHELL_DPUS`]
/// is documented for.
pub const PYTHON_SHELL_COMMAND: &str = "pythonshell";

/// The capacity a job definition runs with when it named none of `MaxCapacity`,
/// `WorkerType` and `NumberOfWorkers`, as `(worker_type, number_of_workers,
/// max_capacity)`.
///
/// The service fills a concrete capacity in rather than leaving a job no runner could
/// size: the figure is the one `CreateJob$MaxCapacity` documents for the job's COMMAND
/// ([`DEFAULT_SPARK_DPUS`] for a Spark batch or streaming job,
/// [`DEFAULT_PYTHON_SHELL_DPUS`] for a Python shell job), and from
/// [`MIN_WORKER_TYPE_GLUE_VERSION`] on a Spark job's figure is expressed as the worker
/// configuration that version takes ([`DEFAULT_SPARK_WORKER_TYPE`]).
///
/// A command this service documents no default capacity for — a Ray job, whose published
/// worker configuration this emulator would have to invent — is left with none, so
/// nothing is reported that the service does not state.
pub fn default_capacity(
    command: &JobCommand,
    glue_version: &str,
) -> (Option<String>, Option<i32>, Option<f64>) {
    if expresses_capacity_as_workers(command, glue_version) {
        return (
            Some(DEFAULT_SPARK_WORKER_TYPE.to_string()),
            Some(DEFAULT_SPARK_WORKERS),
            dpu_per_worker(DEFAULT_SPARK_WORKER_TYPE)
                .map(|per_worker| per_worker * f64::from(DEFAULT_SPARK_WORKERS)),
        );
    }
    match command.name.as_deref() {
        Some(SPARK_ETL_COMMAND) | Some(SPARK_STREAMING_COMMAND) => {
            (None, None, Some(DEFAULT_SPARK_DPUS))
        }
        Some(PYTHON_SHELL_COMMAND) => (None, None, Some(DEFAULT_PYTHON_SHELL_DPUS)),
        _ => (None, None, None),
    }
}

/// Whether the capacity of a job of this command and Glue version IS a worker
/// configuration rather than a DPU figure: a Spark batch or streaming job from
/// [`MIN_WORKER_TYPE_GLUE_VERSION`] on, by the sentence quoted on
/// [`DEFAULT_SPARK_WORKER_TYPE`] ("For Glue version 2.0 or later jobs, you cannot specify a
/// `Maximum capacity`. Instead, you should specify a `Worker type` and the `Number of
/// workers`").
fn expresses_capacity_as_workers(command: &JobCommand, glue_version: &str) -> bool {
    let spark = matches!(
        command.name.as_deref(),
        Some(SPARK_ETL_COMMAND) | Some(SPARK_STREAMING_COMMAND)
    );
    spark
        && glue_version
            .parse::<f64>()
            .is_ok_and(|version| version >= MIN_WORKER_TYPE_GLUE_VERSION)
}

/// The worker configuration `dpus` amounts to on a job whose capacity is expressed as
/// workers: that many [`DEFAULT_SPARK_WORKER_TYPE`] workers, since one of them "maps to 1
/// DPU" (`CreateJob$WorkerType`) — the same reading [`default_capacity`] gives the
/// documented default of [`DEFAULT_SPARK_DPUS`] DPUs.
///
/// `None` when the capacity of such a job is a DPU figure rather than a worker
/// configuration ([`expresses_capacity_as_workers`]), and when the figure is not a whole
/// number of those workers — a Python shell job's 0.0625 DPU is no worker count.
fn workers_for_dpus(
    command: &JobCommand,
    glue_version: &str,
    dpus: f64,
) -> Option<(&'static str, i32)> {
    if !expresses_capacity_as_workers(command, glue_version) {
        return None;
    }
    let workers = dpus / dpu_per_worker(DEFAULT_SPARK_WORKER_TYPE)?;
    if workers.fract() != 0.0 || !(1.0..=f64::from(i32::MAX)).contains(&workers) {
        return None;
    }
    Some((DEFAULT_SPARK_WORKER_TYPE, workers as i32))
}

/// The DPUs one worker of `worker_type` maps to, as `CreateJob$WorkerType` documents
/// each predefined worker ("For the `G.1X` worker type, each worker maps to 1 DPU", "…
/// `G.2X` … 2 DPU", "… `G.4X` … 4 DPU", "… `G.8X` … 8 DPU", "… `G.025X` … 0.25 DPU",
/// "… `Z.2X` … 2 M-DPU").
///
/// `None` for anything the model gives no per-worker DPU figure for — the `Standard`
/// worker type among them — so no capacity is derived where the service documents none.
pub fn dpu_per_worker(worker_type: &str) -> Option<f64> {
    match worker_type {
        "G.025X" => Some(0.25),
        "G.1X" => Some(1.0),
        "G.2X" | "Z.2X" => Some(2.0),
        "G.4X" => Some(4.0),
        "G.8X" => Some(8.0),
        _ => None,
    }
}

/// The `Timeout` a job created without one carries, which depends on its Glue version
/// (see [`DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4`]). The major version is the leading
/// integer of the `GlueVersion` string (`^((\w+\.)+\w+|auto)$`); a version whose major
/// part is not a number — the literal `auto` among them — is treated as the current
/// default, which is what an absent `GlueVersion` resolves to.
pub fn default_timeout_minutes(glue_version: &str) -> i32 {
    let major = glue_version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .unwrap_or(FIRST_GLUE_MAJOR_VERSION_WITH_SHORT_DEFAULT_TIMEOUT);
    if major >= FIRST_GLUE_MAJOR_VERSION_WITH_SHORT_DEFAULT_TIMEOUT {
        DEFAULT_TIMEOUT_MINUTES_FROM_GLUE_5
    } else {
        DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4
    }
}

/// A stored `Timeout` in MINUTES as a duration, held inside the range the member it was
/// stored from allows: at least one minute (a Glue `Timeout` member's `@range(min: 1)`) and
/// at most `ceiling_minutes`, so a stored value outside that range bounds nothing.
///
/// ONE conversion, shared by every budget this service holds in minutes (a job run's script
/// budget, [`job_script_budget`], and a machine learning task run's resource budget,
/// [`ml_task_run_budget`]), so two callers cannot disagree about what a minute is or about
/// what a value outside the range means. A member the model publishes no maximum for passes
/// `i32::MAX` as the ceiling.
#[must_use]
pub fn timeout_minutes_as_duration(timeout_minutes: i32, ceiling_minutes: i32) -> Duration {
    let minutes = timeout_minutes.clamp(1, ceiling_minutes.max(1));
    Duration::from_secs(u64::from(minutes.unsigned_abs()) * 60)
}

/// A stored job definition, plus the runs `StartJobRun` has started of it.
///
/// Every member of the modelled `Job` shape a create can fill is held here, so a later
/// `GetJob` reports what the create stored and nothing else — the members only a RUN
/// produces live on [`JobRunEntry`], in [`JobEntry::runs`], and `ProfileName` is not a
/// `CreateJob` member, so a created job carries none.
#[derive(Debug, Clone)]
pub struct JobEntry {
    /// The job's name, which is also its key in [`ComAmazonawsGlueAwsglueState::jobs`].
    pub name: String,
    /// One of the modelled `JobMode` values; [`DEFAULT_JOB_MODE`] when the create
    /// named none.
    pub job_mode: String,
    /// Whether this job's runs are considered for queueing;
    /// [`DEFAULT_JOB_RUN_QUEUING_ENABLED`] when the definition named nothing.
    pub job_run_queuing_enabled: bool,
    pub description: Option<String>,
    pub log_uri: Option<String>,
    pub role: String,
    /// Always stored, because the service documents a default concurrency
    /// ([`DEFAULT_MAX_CONCURRENT_RUNS`]) rather than "no execution property".
    pub execution_property: ExecutionProperty,
    pub command: JobCommand,
    pub default_arguments: Option<HashMap<String, String>>,
    pub non_overridable_arguments: Option<HashMap<String, String>>,
    pub connections: Option<ConnectionsList>,
    pub max_retries: i32,
    /// The job's capacity in DPUs: what the create named, else what its worker
    /// configuration maps to, else the documented default for its command and Glue
    /// version (see [`capacity_members`]). `None` only for a command this service
    /// publishes no default capacity for.
    pub max_capacity: Option<f64>,
    /// The job timeout in minutes: what the create named, else the documented default
    /// for its Glue version.
    pub timeout: i32,
    pub worker_type: Option<String>,
    pub number_of_workers: Option<i32>,
    pub security_configuration: Option<String>,
    pub notification_property: Option<NotificationProperty>,
    /// [`DEFAULT_GLUE_VERSION`] when the create named none.
    pub glue_version: String,
    pub code_gen_configuration_nodes: Option<HashMap<String, CodeGenConfigurationNode>>,
    pub execution_class: Option<String>,
    pub source_control_details: Option<SourceControlDetails>,
    pub maintenance_window: Option<String>,
    /// Server-set, written once at create so every later read reports the same instant.
    pub created_on: SystemTime,
    /// Server-set; moves with every write to this job.
    pub last_modified_on: SystemTime,
    /// The request's `Tags`, which are not part of the `Job` shape — they are read back
    /// through the tagging operations.
    pub tags: HashMap<String, String>,
    /// The runs of this job definition, oldest first, as `StartJobRun` appended them.
    ///
    /// They live on the job rather than in a store-wide map because every operation that
    /// addresses a run names its job as well (`GetJobRun`/`GetJobRuns`/`BatchStopJobRun`
    /// all take `JobName`), so a run is reclaimed with the definition it belongs to.
    pub runs: Vec<JobRunEntry>,
    /// This job's BOOKMARK — "a point that a job can resume processing"
    /// (`JobBookmarkEntry`) — or `None` while the job has none.
    ///
    /// A bookmark is state a RUN leaves behind, so a job definition nothing has run yet
    /// has none and every operation that addresses one (`GetJobBookmark`,
    /// `ResetJobBookmark`) answers that job with a miss. It lives on the job rather than
    /// beside it because both operations address it by `JobName` alone, so the bookmark is
    /// reclaimed with the definition it belongs to.
    pub bookmark: Option<JobBookmarkState>,
}

/// The bookmark of one job: the point a further run of it would resume processing from.
///
/// The members are `JobBookmarkEntry`'s own, minus the job name, which is the key this is
/// stored under ([`JobEntry::bookmark`]).
#[derive(Debug, Clone, Default)]
pub struct JobBookmarkState {
    /// "The version of the job" the bookmark is of: 1 for the bookmark a job's first run
    /// wrote, advanced by each reset, which starts a new version of the bookmark the next
    /// run continues.
    pub version: i32,
    /// "The run ID number": how many runs of the job have written into this version of the
    /// bookmark.
    pub run: i32,
    /// "The attempt ID number" of the run that last wrote the bookmark.
    pub attempt: i32,
    /// "The unique run identifier associated with the previous job run".
    pub previous_run_id: Option<String>,
    /// "The run ID number" of the run that last wrote the bookmark, as that run's `Id`.
    pub run_id: Option<String>,
    /// "The bookmark itself" — the position a resumed run reads from — and `None` for a
    /// bookmark no run has written a position into, which is the state a reset leaves.
    pub job_bookmark: Option<String>,
}

impl JobBookmarkState {
    /// The bookmark version a job's first run writes. Advanced by each reset, so a reset
    /// leaves the next run continuing a version of its own.
    pub const FIRST_VERSION: i32 = 1;

    /// Record that `run_id` ran: the bookmark advances to that run, keeping the run it
    /// followed as its `PreviousRunId`.
    ///
    /// Called by the only thing that writes a bookmark, the START of a run — whether a
    /// caller started it or the job's retry policy owed it — so a bookmark never moves
    /// because something was read, only because a further run of the job began.
    pub fn record_run(&mut self, run_id: &str, attempt: i32) {
        self.previous_run_id = self.run_id.take();
        self.run_id = Some(run_id.to_string());
        self.attempt = attempt;
        self.run = self.run.saturating_add(1);
        if self.version < Self::FIRST_VERSION {
            self.version = Self::FIRST_VERSION;
        }
    }

    /// Reset this bookmark: "Resets a bookmark entry" (`ResetJobBookmark`), so the
    /// position a further run would resume from is dropped and the run counter starts
    /// again, under the next version of the bookmark.
    ///
    /// NOT recorded: what is pinned is that the reset clears the position the next run
    /// would have resumed from and that the entry stays readable afterwards, which is what
    /// a client resets a bookmark to achieve.
    pub fn reset(&mut self) {
        self.version = self.version.saturating_add(1);
        self.run = 0;
        self.attempt = 0;
        self.previous_run_id = None;
        self.run_id = None;
        self.job_bookmark = None;
    }

    /// This bookmark in the `JobBookmarkEntry` shape every operation that answers one
    /// reports it in, attributed to the job it belongs to.
    pub fn to_job_bookmark_entry(&self, job_name: &str) -> JobBookmarkEntry {
        JobBookmarkEntry {
            job_name: Some(job_name.to_string()),
            version: self.version,
            run: self.run,
            attempt: self.attempt,
            previous_run_id: self.previous_run_id.clone(),
            run_id: self.run_id.clone(),
            job_bookmark: self.job_bookmark.clone(),
        }
    }
}

impl JobEntry {
    /// Build a stored job from a `CreateJob` request plus the members the SERVICE
    /// fills: the documented defaults for an absent `JobMode`, `GlueVersion`,
    /// `ExecutionProperty`, `Timeout` and worker/capacity configuration
    /// ([`capacity_members`]), and the create instant.
    pub fn from_create_input(input: &CreateJobInput, now: SystemTime) -> Self {
        let glue_version = input
            .glue_version
            .clone()
            .unwrap_or_else(|| DEFAULT_GLUE_VERSION.to_string());
        let max_concurrent_runs = max_concurrent_runs_of(input.execution_property.as_ref());
        let (worker_type, number_of_workers, max_capacity) = capacity_members(
            input.max_capacity,
            input.allocated_capacity,
            input.worker_type.as_deref(),
            input.number_of_workers,
            &input.command,
            &glue_version,
        );
        Self {
            name: input.name.clone(),
            job_mode: input
                .job_mode
                .clone()
                .unwrap_or_else(|| DEFAULT_JOB_MODE.to_string()),
            job_run_queuing_enabled: input
                .job_run_queuing_enabled
                .unwrap_or(DEFAULT_JOB_RUN_QUEUING_ENABLED),
            description: input.description.clone(),
            log_uri: input.log_uri.clone(),
            role: input.role.clone(),
            execution_property: ExecutionProperty {
                max_concurrent_runs,
            },
            command: input.command.clone(),
            default_arguments: input.default_arguments.clone(),
            non_overridable_arguments: input.non_overridable_arguments.clone(),
            connections: input.connections.clone(),
            max_retries: input.max_retries,
            max_capacity,
            timeout: input
                .timeout
                .unwrap_or_else(|| default_timeout_minutes(&glue_version)),
            worker_type,
            number_of_workers,
            security_configuration: input.security_configuration.clone(),
            notification_property: input.notification_property.clone(),
            glue_version,
            code_gen_configuration_nodes: input.code_gen_configuration_nodes.clone(),
            execution_class: input.execution_class.clone(),
            source_control_details: input.source_control_details.clone(),
            maintenance_window: input.maintenance_window.clone(),
            created_on: now,
            last_modified_on: now,
            tags: input.tags.clone().unwrap_or_default(),
            // A job definition has no runs until `StartJobRun` starts one.
            runs: Vec::new(),
            // And no bookmark until one of those runs writes it.
            bookmark: None,
        }
    }

    /// Overwrite this job definition with the one an `UpdateJob` request submits.
    ///
    /// "The previous job definition is completely overwritten by this information"
    /// (`UpdateJob`), and "unspecified configuration is removed or reset to default
    /// values" (`UpdateJobRequest$JobUpdate`) — so every member of the stored definition
    /// the submitted `JobUpdate` can carry is taken from it, absent or not, and the
    /// members the SERVICE fills are re-derived from the submitted definition exactly as
    /// [`JobEntry::from_create_input`] derives them for a create. The exceptions are the
    /// four fields `JobUpdate` has no member for — the job's name, its create instant,
    /// its tags (read back through the tagging operations) and the runs `StartJobRun`
    /// already started of it — and `GlueVersion`, which "retains the value from the
    /// existing job definition" when the update names none (`JobUpdate$GlueVersion`).
    ///
    /// A submitted definition naming no `Role` or no `Command` is refused by the
    /// operation before it reaches here, since `JobUpdate` documents both as required.
    pub fn apply_update(&mut self, update: &JobUpdate, now: SystemTime) {
        let glue_version = update
            .glue_version
            .clone()
            .unwrap_or_else(|| self.glue_version.clone());
        let max_concurrent_runs = max_concurrent_runs_of(update.execution_property.as_ref());
        let command = update.command.clone().unwrap_or_default();
        let (worker_type, number_of_workers, max_capacity) = capacity_members(
            update.max_capacity,
            update.allocated_capacity,
            update.worker_type.as_deref(),
            update.number_of_workers,
            &command,
            &glue_version,
        );
        // Carried over rather than overwritten; moved out so the whole record below is
        // rebuilt, which is what makes "completely overwritten" exhaustive.
        let name = std::mem::take(&mut self.name);
        let created_on = self.created_on;
        let tags = std::mem::take(&mut self.tags);
        let runs = std::mem::take(&mut self.runs);
        // Carried over for the same reason as the runs: `JobUpdate` has no member for the
        // bookmark, and an update of the definition does not undo the runs that wrote it.
        let bookmark = self.bookmark.take();
        *self = Self {
            name,
            job_mode: update
                .job_mode
                .clone()
                .unwrap_or_else(|| DEFAULT_JOB_MODE.to_string()),
            job_run_queuing_enabled: update
                .job_run_queuing_enabled
                .unwrap_or(DEFAULT_JOB_RUN_QUEUING_ENABLED),
            description: update.description.clone(),
            log_uri: update.log_uri.clone(),
            role: update.role.clone().unwrap_or_default(),
            execution_property: ExecutionProperty {
                max_concurrent_runs,
            },
            command,
            default_arguments: update.default_arguments.clone(),
            non_overridable_arguments: update.non_overridable_arguments.clone(),
            connections: update.connections.clone(),
            max_retries: update.max_retries,
            max_capacity,
            timeout: update
                .timeout
                .unwrap_or_else(|| default_timeout_minutes(&glue_version)),
            worker_type,
            number_of_workers,
            security_configuration: update.security_configuration.clone(),
            notification_property: update.notification_property.clone(),
            glue_version,
            code_gen_configuration_nodes: update.code_gen_configuration_nodes.clone(),
            execution_class: update.execution_class.clone(),
            source_control_details: update.source_control_details.clone(),
            maintenance_window: update.maintenance_window.clone(),
            created_on,
            last_modified_on: now,
            tags,
            runs,
            bookmark,
        };
    }

    /// The maximum number of this job's runs that may be in flight at once:
    /// `ExecutionProperty$MaxConcurrentRuns`, held at the documented minimum of one
    /// ([`DEFAULT_MAX_CONCURRENT_RUNS`]) so a stored zero cannot make the job unrunnable.
    pub fn max_concurrent_runs(&self) -> i32 {
        self.execution_property
            .max_concurrent_runs
            .max(DEFAULT_MAX_CONCURRENT_RUNS)
    }

    /// Give every run of this job the state the CLOCK already owed it at `now`.
    ///
    /// This is the one write a reporting operation performs: a run's next state was due
    /// at [`JobRunEntry::settles_at`] whether or not anybody called, so materialising it
    /// on read reports the state the clock had already reached. It never moves a run
    /// because something was read.
    pub fn settle_due_runs(&mut self, now: SystemTime) {
        for run in &mut self.runs {
            run.settle_if_due(now);
        }
    }

    /// CLAIM the one script execution a run of this job is still owed, and answer which run
    /// it was — oldest first, or `None` when every run of this job has been performed.
    ///
    /// A run begins owing one execution of its job's script
    /// ([`JobRunEntry::awaiting_script`]), so this is what the execution path asks for to
    /// find the work the service already owes ([`super::perform_owed_job_runs`]). The claim
    /// is taken HERE, under the store's guard, and the script then runs without it — so two
    /// calls that find the same run owed its script cannot both run it, and the run's effects
    /// happen once. A claim whose execution never happened is given back
    /// ([`JobEntry::release_script_claim`]), so nothing is left owed by nobody. The walk is
    /// bounded by the runs one job holds ([`MAX_RUNS_PER_JOB`]).
    pub fn claim_run_awaiting_script(&mut self) -> Option<String> {
        let run = self.runs.iter_mut().find(|run| run.awaiting_script)?;
        run.awaiting_script = false;
        Some(run.id.clone())
    }

    /// Give back the claim on the run `run_id`'s script execution, for a claim nothing ran.
    ///
    /// The run owes its script an execution again, so a later call performs it rather than
    /// this emulator reporting an outcome no execution produced. A run that has come to rest
    /// in the meantime owes nothing.
    pub fn release_script_claim(&mut self, run_id: &str) {
        if let Some(run) = self.run_mut(run_id) {
            if !run.is_terminal() && run.settles_to != JOB_RUN_STATE_STOPPED {
                run.awaiting_script = true;
            }
        }
    }

    /// Record the next ATTEMPT every run of this job that FAILED leaves owed.
    ///
    /// "The maximum number of times to retry this job after a JobRun fails"
    /// (`Job$MaxRetries`) is a policy of the JOB, so the SERVICE — not the caller — runs the
    /// next attempt of a run that ended in the failure state while the job still has retry
    /// budget: a further run of the same job under the same configuration, carrying the next
    /// `Attempt` and naming the attempt it retries in its `PreviousRunId`
    /// ([`JobRunEntry::retry`]). Like any other run that attempt owes its job's script ONE
    /// execution, which the execution path performs for it
    /// ([`super::perform_owed_job_runs`]) — so what an attempt comes to rest in is what its
    /// OWN execution came to and not what the attempt it retries came to.
    ///
    /// A retry is owed only by a run that has COME TO REST in the failure state, so the
    /// chain advances one attempt per span the clock owes: a caller polling it observes each
    /// attempt in flight before the next one exists, and a run a submitted stop brought to
    /// rest — which failed at nothing — buys no attempt at all. A run another attempt
    /// already retries is retried by nothing further, so a chain never forks and the chain
    /// one start begins is one original attempt plus at most `MaxRetries` further runs.
    pub fn record_owed_attempts(&mut self) {
        if self.max_retries <= 0 {
            return;
        }
        let retried: BTreeSet<&str> = self
            .runs
            .iter()
            .filter_map(|run| run.previous_run_id.as_deref())
            .collect();
        let recorded: Vec<JobRunEntry> = self
            .runs
            .iter()
            .filter(|run| {
                run.state == JOB_RUN_STATE_FAILED
                    && run.attempt < self.max_retries
                    && !retried.contains(run.id.as_str())
            })
            .map(|failed| failed.retry(new_job_run_id()))
            .collect();
        for attempt in recorded {
            self.record_run(attempt);
        }
    }

    /// The stored run with this `Id`, or `None` when this job has no such run.
    pub fn run(&self, run_id: &str) -> Option<&JobRunEntry> {
        self.runs.iter().find(|run| run.id == run_id)
    }

    /// The same run, to write to: what an operation that moves one run of this job
    /// (`BatchStopJobRun`) addresses it through.
    pub fn run_mut(&mut self, run_id: &str) -> Option<&mut JobRunEntry> {
        self.runs.iter_mut().find(|run| run.id == run_id)
    }

    /// How many of this job's runs have not finished.
    ///
    /// Call [`JobEntry::settle_due_runs`] first, or a run the clock has already finished
    /// still counts as in flight.
    pub fn runs_in_flight(&self) -> usize {
        self.runs.iter().filter(|run| !run.is_terminal()).count()
    }

    /// Append a run, keeping at most [`MAX_RUNS_PER_JOB`] of them.
    ///
    /// This is also where the job's BOOKMARK is written: a bookmark is the point a job
    /// resumes processing at, which a run is what establishes — so the first run of a job
    /// gives it one ([`JobBookmarkState::record_run`]) and every later run advances it,
    /// including an attempt the job's retry policy owed ([`JobEntry::record_owed_attempts`]),
    /// which is a further run of the job and not a re-read of an earlier one. Nothing else
    /// writes it except a reset, so a bookmark never moves because something was read.
    pub fn record_run(&mut self, run: JobRunEntry) {
        self.bookmark
            .get_or_insert_with(JobBookmarkState::default)
            .record_run(&run.id, run.attempt);
        self.runs.push(run);
        if self.runs.len() > MAX_RUNS_PER_JOB {
            let overflow = self.runs.len() - MAX_RUNS_PER_JOB;
            self.runs.drain(..overflow);
        }
    }

    /// Whether this stored job definition is the one `input` asks to create, member for
    /// member.
    ///
    /// "It must be unique in your account" (`CreateJobRequest$Name`), so the name is what
    /// a create reuses when it submits a definition under one already taken: such a create
    /// is a RETRY of the create that stored this record only when it asks for the very
    /// same job, and otherwise it has associated one identifier with two different
    /// records — which is what the operation's `IdempotentParameterMismatchException`
    /// ("the same unique identifier was associated with two different records") states.
    ///
    /// The comparison is over the whole stored definition, the tags included, because any
    /// member of it is part of the job the caller asked for. It is made on the modelled
    /// `Job` shape rendered by this service's own JSON codec rather than member by member:
    /// the definition carries the `CodeGenConfigurationNodes` ETL graph, whose modelled
    /// shapes derive no equality and hold `HashMap` members whose iteration order comes
    /// from a per-process hash seed, so neither `Debug` nor the rendered bytes of two equal
    /// graphs match while `serde_json::Value`'s own equality compares objects member by
    /// member whichever map backs them. The two SERVER-set instants are not part of the
    /// submitted definition, so the candidate is built carrying this record's own.
    ///
    /// A definition the codec cannot render is not shown to be this one, so such a create
    /// is answered as the mismatch it cannot be told apart from rather than as a retry
    /// that would drop what it sent behind a refusal.
    pub fn matches_create_input(&self, input: &CreateJobInput) -> bool {
        let mut candidate = Self::from_create_input(input, self.created_on);
        candidate.last_modified_on = self.last_modified_on;
        if self.tags != candidate.tags {
            return false;
        }
        match (
            rendered_shape(&Job::SCHEMA, &self.to_job()),
            rendered_shape(&Job::SCHEMA, &candidate.to_job()),
        ) {
            (Some(stored), Some(requested)) => stored == requested,
            _ => false,
        }
    }

    /// Rebuild the modelled `Job` output shape from what is stored.
    pub fn to_job(&self) -> Job {
        Job {
            name: Some(self.name.clone()),
            job_mode: Some(self.job_mode.clone()),
            job_run_queuing_enabled: Some(self.job_run_queuing_enabled),
            description: self.description.clone(),
            log_uri: self.log_uri.clone(),
            role: Some(self.role.clone()),
            created_on: Some(to_date_time(self.created_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            execution_property: Some(self.execution_property.clone()),
            command: Some(self.command.clone()),
            default_arguments: self.default_arguments.clone(),
            non_overridable_arguments: self.non_overridable_arguments.clone(),
            connections: self.connections.clone(),
            max_retries: self.max_retries,
            // `AllocatedCapacity` is the same quantity as `MaxCapacity` — "This field
            // is deprecated. Use MaxCapacity instead" — so it reports the same number
            // of whole DPUs rather than a second, disagreeing figure. A job with no
            // capacity figure reports the model's own default of 0.
            allocated_capacity: self.max_capacity.map_or(0, |capacity| capacity as i32),
            timeout: Some(self.timeout),
            max_capacity: self.max_capacity,
            worker_type: self.worker_type.clone(),
            number_of_workers: self.number_of_workers,
            security_configuration: self.security_configuration.clone(),
            notification_property: self.notification_property.clone(),
            glue_version: Some(self.glue_version.clone()),
            code_gen_configuration_nodes: self.code_gen_configuration_nodes.clone(),
            execution_class: self.execution_class.clone(),
            source_control_details: self.source_control_details.clone(),
            maintenance_window: self.maintenance_window.clone(),
            // `ProfileName` names a usage profile, which `CreateJob` cannot set.
            profile_name: None,
        }
    }
}

/// One modelled shape as the JSON value this service's own codec renders for it.
///
/// Rendering through the codec is what makes two shapes COMPARABLE at all: the modelled
/// structures derive no equality, and several of them hold `std::collections::HashMap`
/// members (a job's `DefaultArguments`, its `CodeGenConfigurationNodes` and the options
/// inside them) whose iteration order comes from a per-process hash seed, so neither
/// `Debug` nor the rendered bytes of two equal values match. `serde_json::Value` then
/// supplies the equality: its objects compare by member, not by order, whichever map
/// backs them, and its lists keep their order, which is right for the modelled sequences.
///
/// A value the codec cannot render answers `None`, so the comparison is impossible rather
/// than false and the caller decides what to do with that.
pub(super) fn rendered_shape(
    schema: &::aws_smithy_schema::Schema,
    value: &dyn ::aws_smithy_schema::serde::SerializableStruct,
) -> Option<::serde_json::Value> {
    use ::aws_smithy_schema::serde::ShapeSerializer;

    let mut serializer = ::aws_smithy_schema_server::json::JsonShapeSerializer::new();
    if let Err(error) = serializer.write_struct(schema, value) {
        ::tracing::warn!(
            %error,
            "the JSON codec could not render a stored shape, so two definitions cannot be compared"
        );
        return None;
    }
    match ::serde_json::from_str::<::serde_json::Value>(&serializer.into_string()) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            ::tracing::warn!(
                %error,
                "the JSON this service renders for a stored shape did not parse back"
            );
            None
        }
    }
}

/// `ExecutionProperty$MaxConcurrentRuns` a submitted job definition asks for: what it
/// names, else the documented default ([`DEFAULT_MAX_CONCURRENT_RUNS`]) — which is also
/// what a property naming zero concurrent runs resolves to, since that would otherwise
/// store a job no run of which could ever start.
///
/// `CreateJob` and `UpdateJob` carry the same member with the same documentation, so
/// both resolve it here.
pub(super) fn max_concurrent_runs_of(property: Option<&ExecutionProperty>) -> i32 {
    property
        .map(|property| property.max_concurrent_runs)
        .filter(|runs| *runs > 0)
        .unwrap_or(DEFAULT_MAX_CONCURRENT_RUNS)
}

/// The capacity members a submitted job definition is STORED with, as
/// `(worker_type, number_of_workers, max_capacity)`.
///
/// What the definition named, read by [`capacity_from`] — else, when it named no capacity
/// at all, the documented default for its command and Glue version
/// ([`default_capacity`]), because the service fills those in rather than storing a job
/// with no capacity for a runner to size. A definition that named only a DPU figure, on a
/// job whose capacity IS a worker configuration, is stored with the configuration that
/// figure amounts to ([`workers_for_dpus`]), so the three members always describe ONE
/// capacity. `CreateJob` and `UpdateJob` submit the same four capacity members with the
/// same documentation, so both resolve them here.
pub(super) fn capacity_members(
    max_capacity: Option<f64>,
    allocated_capacity: i32,
    worker_type: Option<&str>,
    number_of_workers: Option<i32>,
    command: &JobCommand,
    glue_version: &str,
) -> (Option<String>, Option<i32>, Option<f64>) {
    let named = capacity_from(
        max_capacity,
        allocated_capacity,
        worker_type,
        number_of_workers,
    );
    if worker_type.is_none() && number_of_workers.is_none() {
        // Nothing named a capacity at all: the service fills in the documented default.
        let Some(dpus) = named else {
            return default_capacity(command, glue_version);
        };
        // A definition that expressed its capacity as a DPU figure, on a command and Glue
        // version whose capacity IS a worker configuration, is stored with the
        // configuration that figure amounts to, so a read reports the capacity the job
        // holds in the form that job's capacity takes.
        if let Some((derived_worker_type, workers)) = workers_for_dpus(command, glue_version, dpus)
        {
            return (
                Some(derived_worker_type.to_string()),
                Some(workers),
                Some(dpus),
            );
        }
    }
    (
        worker_type.map(str::to_string),
        number_of_workers,
        named,
    )
}

/// The capacity, in DPUs, a request asks for: `MaxCapacity` when it names one, else the
/// deprecated `AllocatedCapacity` ("This parameter is deprecated. Use MaxCapacity
/// instead", so the two are one quantity), else what the worker configuration maps to —
/// `NumberOfWorkers` workers of `WorkerType`, at the DPUs per worker [`dpu_per_worker`]
/// quotes from the model. `None` when nothing names a capacity and no worker
/// configuration the model gives a DPU figure for is named.
///
/// `CreateJob`, `UpdateJob` and `StartJobRun` carry the same four members and the same
/// documentation for each, and a session create carries three of them under that same
/// documentation, so all of them resolve capacity here.
pub(super) fn capacity_from(
    max_capacity: Option<f64>,
    allocated_capacity: i32,
    worker_type: Option<&str>,
    number_of_workers: Option<i32>,
) -> Option<f64> {
    if let Some(max_capacity) = max_capacity {
        return Some(max_capacity);
    }
    if allocated_capacity > 0 {
        return Some(f64::from(allocated_capacity));
    }
    let workers = number_of_workers.filter(|count| *count > 0)?;
    let per_worker = dpu_per_worker(worker_type?)?;
    Some(per_worker * f64::from(workers))
}

/// The `JobRunState` a run is in the instant `StartJobRun` returns, and the one the
/// clock then owes it. Both are values of the modelled `JobRunState` enum (`STARTING`,
/// `RUNNING`, `STOPPING`, `STOPPED`, `SUCCEEDED`, `FAILED`, `TIMEOUT`, `ERROR`,
/// `WAITING`, `EXPIRED`).
pub const JOB_RUN_STATE_STARTING: &str = "STARTING";
/// See [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_FAILED: &str = "FAILED";
/// The `JobRunState` a run is in once a stop has been submitted for it, before it comes
/// to rest in [`JOB_RUN_STATE_STOPPED`]. See [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_STOPPING: &str = "STOPPING";
/// The `JobRunState` a submitted stop brings a run to rest in. See
/// [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_STOPPED: &str = "STOPPED";
/// The `JobRunState` of a run whose script finished without failing. See
/// [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_SUCCEEDED: &str = "SUCCEEDED";
/// The `JobRunState` of a run that "is terminated and enters `TIMEOUT` status"
/// (`JobRun$Timeout`). See [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_TIMEOUT: &str = "TIMEOUT";
/// The `JobRunState` of a run that ended on a service error rather than on its script.
/// See [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_ERROR: &str = "ERROR";
/// The `JobRunState` of a run held back by job-run queuing: "when a job run is in a
/// WAITING state as a result of job run queuing" (`JobRun$StateDetail`). See
/// [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_WAITING: &str = "WAITING";
/// The `JobRunState` of a queued run that waited past its queuing window. See
/// [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_EXPIRED: &str = "EXPIRED";

/// The `JobRunState` values a run never moves out of again: of the modelled enum quoted
/// on [`JOB_RUN_STATE_STARTING`], the ones that name a finished run. The other four
/// (`STARTING`, `RUNNING`, `STOPPING`, `WAITING`) each still owe a further state.
pub const TERMINAL_JOB_RUN_STATES: [&str; 6] = [
    JOB_RUN_STATE_STOPPED,
    JOB_RUN_STATE_SUCCEEDED,
    JOB_RUN_STATE_FAILED,
    JOB_RUN_STATE_TIMEOUT,
    JOB_RUN_STATE_ERROR,
    JOB_RUN_STATE_EXPIRED,
];

/// How long a run stays in the state a call put it in before the clock owes it the next
/// one: [`JOB_RUN_STATE_STARTING`] before the state the run comes to rest in, and
/// [`JOB_RUN_STATE_STOPPING`] before [`JOB_RUN_STATE_STOPPED`].
///
/// CHOSEN, not documented: Glue publishes no duration for starting or for stopping a job
/// run, so this is the emulator's own span — deliberately not zero, so a caller that
/// polls `GetJobRun` straight after `StartJobRun` still observes the run in flight, and
/// the concurrency limit that goes with it, and one that polls straight after a submitted
/// stop still observes the run stopping. The due instant is stored on the run and
/// materialised by whichever later call reads it; nothing sleeps.
pub const JOB_RUN_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// The `JobRunState` a run is in while its script is being executed, before the state that
/// execution comes to. See [`JOB_RUN_STATE_STARTING`].
pub const JOB_RUN_STATE_RUNNING: &str = "RUNNING";

/// `JobRun$Attempt` of a run that is not a retry of an earlier one: "The number of the
/// attempt to run this job", which for a first attempt is the model's own default of 0.
pub const FIRST_JOB_RUN_ATTEMPT: i32 = 0;

/// The prefix a Glue job run id carries, which is what makes an `IdString` recognisable
/// as a job run id rather than any other id the service mints.
pub const JOB_RUN_ID_PREFIX: &str = "jr_";

/// How many runs one job definition keeps.
///
/// CHOSEN: Glue publishes no retention for job-run records, so this is the emulator's own
/// window. It bounds [`JobEntry::runs`], and with it every per-job walk, so a job that
/// has been started many times still costs a bounded amount of work and memory. The
/// OLDEST record is dropped first, so the runs a caller is most likely to read are kept.
pub const MAX_RUNS_PER_JOB: usize = 100;

/// How many job run ids this process has minted. A nonce source for
/// [`new_job_run_id`], exactly as [`MINTED_DATABASE_IDS`] is for a database id.
pub(super) static MINTED_JOB_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `JobRunId`: the id the service assigns a job run when it starts.
///
/// [`JOB_RUN_ID_PREFIX`] followed by 64 lowercase hexadecimal characters, i.e. 256 bits,
/// which is the shape a Glue job run id has on the wire and is inside `IdString`'s
/// `length 1..255`. The bits come from [`mint_hex_id`], as a database id's do,
/// so two runs started in the same nanosecond cannot be given the same id. It is stored
/// on the run, so every later read of that run reports the same id.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_job_run_id() -> String {
    format!(
        "{JOB_RUN_ID_PREFIX}{}",
        mint_hex_id(&MINTED_JOB_RUN_IDS, JOB_RUN_ID_WORDS)
    )
}

/// How many 64-bit words a Glue job run id carries: four, i.e. 256 bits rendered as 64
/// lowercase hexadecimal characters.
pub(super) const JOB_RUN_ID_WORDS: usize = 4;

/// The job-run request an `Action` of a fired trigger amounts to: the run of `job_name`
/// that action initiates, carrying every override the two shapes share — `Arguments`,
/// `Timeout`, `SecurityConfiguration` and `NotificationProperty`, each documented on the
/// action in the words `StartJobRunRequest` documents it in — and nothing else, because an
/// action names no capacity, no execution class and no previous run to retry. Reading the
/// run off this is what makes a run a firing began the same record a direct start records
/// ([`JobRunEntry::start`]).
pub(super) fn job_run_request_for_action(action: &Action, job_name: &str) -> StartJobRunInput {
    StartJobRunInput {
        job_name: job_name.to_string(),
        arguments: action.arguments.clone(),
        timeout: action.timeout,
        security_configuration: action.security_configuration.clone(),
        notification_property: action.notification_property.clone(),
        ..StartJobRunInput::default()
    }
}

/// One run of a job definition, as a start of that job or a trigger's firing recorded it.
///
/// A run carries the job's own configuration with the members the `StartJobRun` request
/// overrode applied on top — "This value overrides the timeout value set in the parent
/// job" (`StartJobRun$Timeout`) is the rule for each of them — plus the members only a
/// run has: its id, its attempt, its state and the instants it moved between them.
#[derive(Debug, Clone)]
pub struct JobRunEntry {
    /// `JobRun$Id`, minted by [`new_job_run_id`].
    pub id: String,
    /// The name of the job definition this is a run of.
    pub job_name: String,
    /// `JobRun$TriggerName`: "The name of the trigger that started this job run", and
    /// `None` for a run a caller started directly, which no trigger started.
    pub trigger_name: Option<String>,
    /// `JobRun$Attempt`; one past the retried run's attempt when this run retries one.
    pub attempt: i32,
    /// `JobRun$PreviousRunId`: "the `JobRunId` specified in the `StartJobRun` action",
    /// i.e. the run this one retries, when the request named one.
    pub previous_run_id: Option<String>,
    pub job_mode: String,
    /// Whether this run was considered for queueing: what the start named, else the job's
    /// own flag ([`JobEntry::job_run_queuing_enabled`]).
    pub job_run_queuing_enabled: bool,
    /// Server-set: the instant `StartJobRun` recorded this run.
    pub started_on: SystemTime,
    /// Server-set: moves with every state change of this run.
    pub last_modified_on: SystemTime,
    /// Server-set when the run reaches a terminal state, and absent until then.
    pub completed_on: Option<SystemTime>,
    /// One of the modelled `JobRunState` values.
    pub state: String,
    /// The instant the clock owes this run its next state; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// The `JobRunState` the clock owes this run at [`JobRunEntry::settles_at`]:
    /// [`JOB_RUN_STATE_RUNNING`] while the run's script is still owed an execution, the
    /// state that execution came to once it has been performed
    /// ([`JobRunEntry::record_execution`]), and [`JOB_RUN_STATE_STOPPED`] once a stop has
    /// been submitted for it.
    pub settles_to: String,
    /// `JobRun$ErrorMessage` the run carries once it has come to rest, and `None` for a run
    /// that comes to rest having failed at nothing: the reason travels with the run from
    /// the instant the execution answered until the clock settles it, so the message a run
    /// reports is the one its own execution produced.
    pub settles_error: Option<String>,
    /// Whether this run still owes its job's script ONE execution.
    ///
    /// A run begins owing it, and the execution path clears this when the script has been
    /// performed ([`JobRunEntry::record_execution`]) or when a submitted stop has brought
    /// the run to rest before anything ran it — so no run is ever performed twice, and a
    /// run nothing has performed yet is found by [`JobEntry::claim_run_awaiting_script`]
    /// rather than reported as having come to any outcome.
    pub awaiting_script: bool,
    /// Whether the terminal state this run came to rest in has already been queued for
    /// publication as a `Glue Job State Change` event ([`super::job_run_events`]).
    ///
    /// A run begins having published nothing — it has come to rest at nothing — and the one
    /// place that turns a terminal state into an event
    /// ([`ComAmazonawsGlueAwsglueState::record_job_run_state_changes`]) sets this when it
    /// queues the run's change, so a run is published exactly once however it was brought
    /// to rest. It is bookkeeping about the publication and no part of what any read of the
    /// run reports.
    pub state_change_published: bool,
    /// The job's `DefaultArguments` with this run's `Arguments` applied over them, then
    /// the job's `NonOverridableArguments`: "For this job run, they replace the default
    /// arguments set in the job definition itself" (`StartJobRun$Arguments`), except for
    /// the "arguments for this job that are not overridden when providing job arguments
    /// in a job run" (`CreateJob$NonOverridableArguments`).
    pub arguments: HashMap<String, String>,
    pub error_message: Option<String>,
    pub max_capacity: Option<f64>,
    pub timeout: i32,
    pub worker_type: Option<String>,
    pub number_of_workers: Option<i32>,
    pub security_configuration: Option<String>,
    pub notification_property: Option<NotificationProperty>,
    pub glue_version: String,
    /// `JobRun$LogGroupName`: the group this run's logs are written under, as
    /// [`job_run_log_group`] resolves it from the job's command and this run's security
    /// configuration. Written once at start, so every read of the run reports one group.
    pub log_group_name: String,
    /// `JobRun$PredecessorRuns`: "a list of predecessors to this job run" — the runs "used in
    /// the predicate of a conditional trigger that triggered this job run" (`Predecessor`),
    /// which is empty for a run a caller started directly and for one a trigger with no
    /// satisfied predicate began.
    pub predecessors: Vec<Predecessor>,
    pub execution_class: Option<String>,
    pub maintenance_window: Option<String>,
    pub execution_role_session_policy: Option<String>,
}

impl JobRunEntry {
    /// Record a run of `job` as the `StartJobRun` request asks for it, in
    /// [`JOB_RUN_STATE_STARTING`] with its next state due [`JOB_RUN_SETTLES_AFTER`] out.
    ///
    /// `retries` is the `(id, attempt)` of the run this one retries, when the request's
    /// `JobRunId` ("The ID of a previous JobRun to retry") named one.
    ///
    /// `triggered_by` is the trigger whose firing initiated this run (`JobRun$TriggerName`,
    /// "The name of the trigger that started this job run"), and `None` for a run a caller
    /// started directly.
    pub fn start(
        job: &JobEntry,
        input: &StartJobRunInput,
        id: String,
        now: SystemTime,
        retries: Option<(String, i32)>,
        triggered_by: Option<&str>,
    ) -> Self {
        let (previous_run_id, attempt) = match retries {
            Some((previous_id, previous_attempt)) => {
                (Some(previous_id), previous_attempt.saturating_add(1))
            }
            None => (None, FIRST_JOB_RUN_ATTEMPT),
        };
        // Each capacity member of the run is the one the request named, else the job's,
        // and the DPU figure is then derived from that pair exactly as a job's is.
        let worker_type = input.worker_type.clone().or_else(|| job.worker_type.clone());
        let number_of_workers = input.number_of_workers.or(job.number_of_workers);
        let max_capacity = capacity_from(
            input.max_capacity,
            input.allocated_capacity,
            worker_type.as_deref(),
            number_of_workers,
        )
        .or(job.max_capacity);

        let mut arguments = job.default_arguments.clone().unwrap_or_default();
        if let Some(requested) = input.arguments.as_ref() {
            arguments.extend(
                requested
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone())),
            );
        }
        if let Some(locked) = job.non_overridable_arguments.as_ref() {
            arguments.extend(
                locked
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone())),
            );
        }

        Self {
            id,
            job_name: job.name.clone(),
            trigger_name: triggered_by.map(str::to_string),
            attempt,
            previous_run_id,
            job_mode: job.job_mode.clone(),
            // "If this field does not match the value set in the job run, then the value
            // from the job run field will be used."
            job_run_queuing_enabled: input
                .job_run_queuing_enabled
                .unwrap_or(job.job_run_queuing_enabled),
            started_on: now,
            last_modified_on: now,
            completed_on: None,
            state: JOB_RUN_STATE_STARTING.to_string(),
            settles_at: Some(now.checked_add(JOB_RUN_SETTLES_AFTER).unwrap_or(now)),
            // The run's script has not been executed yet, so the state the clock owes a
            // run that nobody stops is the one a run whose script is running is in; the
            // state it comes to REST in is whatever that execution comes to
            // ([`JobRunEntry::record_execution`]).
            settles_to: JOB_RUN_STATE_RUNNING.to_string(),
            settles_error: None,
            awaiting_script: true,
            // Nothing has come to rest, so nothing has been published about this run.
            state_change_published: false,
            arguments,
            // Nothing has failed yet: a run carries an error message only once it has.
            error_message: None,
            max_capacity,
            timeout: input.timeout.unwrap_or(job.timeout),
            worker_type,
            number_of_workers,
            security_configuration: input
                .security_configuration
                .clone()
                .or_else(|| job.security_configuration.clone()),
            notification_property: input
                .notification_property
                .clone()
                .or_else(|| job.notification_property.clone()),
            // `GlueVersion` is not a `StartJobRun` member: a run uses its job's.
            glue_version: job.glue_version.clone(),
            // Which log group the run writes under follows from the job's command and the
            // security configuration the run ends up with, so it is resolved here, once.
            log_group_name: job_run_log_group(
                &job.command,
                &job.role,
                input
                    .security_configuration
                    .as_deref()
                    .or(job.security_configuration.as_deref()),
            ),
            // A run a caller started is preceded by nothing; a firing whose predicate named
            // runs records them on the run it begins.
            predecessors: Vec::new(),
            execution_class: input
                .execution_class
                .clone()
                .or_else(|| job.execution_class.clone()),
            // Nor is `MaintenanceWindow`.
            maintenance_window: job.maintenance_window.clone(),
            execution_role_session_policy: input.execution_role_session_policy.clone(),
        }
    }

    /// Record what ONE execution of this run's script came to, which is the state the run
    /// comes to REST in: `SUCCEEDED` for a script that exited cleanly, the modelled failure
    /// state carrying the script's own error for one that raised, and the state a run that
    /// ran past its `Timeout` reports ([`super::JobRunOutcome`]).
    ///
    /// The state is owed from the instant the execution answered, but no earlier than the
    /// span every run spends in the state its start put it in
    /// ([`JOB_RUN_SETTLES_AFTER`]) — so a caller that polls straight after `StartJobRun`
    /// still observes the run in flight however fast its script was, and a run whose script
    /// took longer than that span comes to rest as soon as it answered.
    ///
    /// A run a stop has already been submitted for keeps the stop it was owed: a client
    /// asked for that run to stop, and the script's own outcome does not undo it. Nor does
    /// this move a run that has already come to rest.
    pub fn record_execution(&mut self, outcome: &super::JobRunOutcome, now: SystemTime) {
        // Whatever happens below, this run no longer owes its script an execution: it has
        // had one.
        self.awaiting_script = false;
        if self.is_terminal()
            || self.state == JOB_RUN_STATE_STOPPING
            || self.settles_to == JOB_RUN_STATE_STOPPED
        {
            return;
        }
        let span_ends = self
            .started_on
            .checked_add(JOB_RUN_SETTLES_AFTER)
            .unwrap_or(self.started_on);
        self.settles_at = Some(span_ends.max(now));
        self.settles_to = outcome.job_run_state().to_string();
        self.settles_error = outcome.error_message();
    }

    /// Whether this stored run has finished, by [`TERMINAL_JOB_RUN_STATES`].
    pub fn is_terminal(&self) -> bool {
        TERMINAL_JOB_RUN_STATES.contains(&self.state.as_str())
    }

    /// The RETRY of this run: the next attempt of the same job, under the configuration
    /// this attempt ran with, carrying one past this run's `Attempt` ("The number of the
    /// attempt to run this job") and naming this run as the `PreviousRunId` it retries
    /// ("The ID of the previous run of this job").
    ///
    /// It begins the instant this attempt FINISHED, because that is the instant the retry
    /// became owed — taking it from the stored instant rather than from when somebody read
    /// the chain is what makes two reads of the chain report the same runs — and like any
    /// other run it owes its own terminal state one span later
    /// ([`JOB_RUN_SETTLES_AFTER`]), so a caller walking the chain observes each attempt in
    /// flight before the next one exists. Nothing has failed in the new attempt yet, so it
    /// carries no `ErrorMessage` of its own and no completion instant.
    ///
    /// Scheduled by [`JobEntry::record_owed_attempts`], which decides WHETHER an attempt is
    /// owed; this builds the record of one. A retry of a run a TRIGGER began keeps that run's
    /// `TriggerName`, because the attempt is of the work that firing initiated; the workflow
    /// run that firing belongs to still reports the attempt IT recorded, since its actions
    /// name the run ids they began.
    pub(super) fn retry(&self, id: String) -> Self {
        let started_on = self.completed_on.unwrap_or(self.last_modified_on);
        Self {
            id,
            attempt: self.attempt.saturating_add(1),
            previous_run_id: Some(self.id.clone()),
            started_on,
            last_modified_on: started_on,
            completed_on: None,
            state: JOB_RUN_STATE_STARTING.to_string(),
            settles_at: Some(
                started_on
                    .checked_add(JOB_RUN_SETTLES_AFTER)
                    .unwrap_or(started_on),
            ),
            // This attempt's script has not been executed yet either, so it owes one and
            // the clock owes it the state a run whose script is running is in until that
            // execution answers.
            settles_to: JOB_RUN_STATE_RUNNING.to_string(),
            settles_error: None,
            awaiting_script: true,
            // A fresh attempt has come to rest at nothing, whatever was published about the
            // attempt it retries.
            state_change_published: false,
            error_message: None,
            ..self.clone()
        }
    }

    /// Move this run into the state the clock owed it at `now`, if it owed one.
    ///
    /// The run was due to leave the state a call put it in at [`JobRunEntry::settles_at`]
    /// whether or not anybody called, and the instants recorded are that DUE instant, not
    /// the instant somebody read it — so two reads of a settled run report the same run.
    pub fn settle_if_due(&mut self, now: SystemTime) {
        let Some(due) = self.settles_at else {
            return;
        };
        if due > now {
            return;
        }
        self.settles_at = None;
        self.state = self.settles_to.clone();
        // `JobRun$ErrorMessage` is "the error message associated with this job run": the
        // run carries the reason its own execution answered with, and a run that comes to
        // rest having failed at nothing — one that succeeded, and one a submitted stop
        // brought to rest — carries none.
        if self.is_terminal() {
            self.error_message = self.settles_error.take();
            self.completed_on = Some(due);
        }
        self.last_modified_on = due;
    }

    /// Submit a stop for this run, as `BatchStopJobRun` asks for one, and answer whether
    /// the submission was accepted.
    ///
    /// A run that has not finished moves into [`JOB_RUN_STATE_STOPPING`] with
    /// [`JOB_RUN_STATE_STOPPED`] owed one span later, so a caller reading straight after
    /// the stop still sees the run stopping, and the slot it occupies is only freed once
    /// it has stopped. A run already stopping keeps the stop already owed it — a second
    /// submission for the same run neither postpones it nor records a different one — and
    /// a run that has already finished accepts no stop at all: there is nothing left to
    /// stop.
    pub fn submit_stop(&mut self, now: SystemTime) -> bool {
        if self.is_terminal() {
            return false;
        }
        if self.state == JOB_RUN_STATE_STOPPING {
            return true;
        }
        self.state = JOB_RUN_STATE_STOPPING.to_string();
        self.settles_at = Some(now.checked_add(JOB_RUN_SETTLES_AFTER).unwrap_or(now));
        self.settles_to = JOB_RUN_STATE_STOPPED.to_string();
        // A run that is stopping is not run: a client asked for it to stop, so its script
        // is no longer owed an execution, and the run comes to rest having failed at
        // nothing.
        self.awaiting_script = false;
        self.settles_error = None;
        self.last_modified_on = now;
        true
    }

    /// `JobRun$ExecutionTime`: "The amount of time (in seconds) that the job run consumed
    /// resources" — the whole seconds between the run starting and finishing, and the
    /// model's own default of 0 while it has not finished.
    pub(super) fn execution_time_seconds(&self) -> i32 {
        let Some(completed_on) = self.completed_on else {
            return 0;
        };
        let elapsed = completed_on
            .duration_since(self.started_on)
            .unwrap_or_default();
        i32::try_from(elapsed.as_secs()).unwrap_or(i32::MAX)
    }

    /// Rebuild the modelled `JobRun` output shape from what is stored.
    pub fn to_job_run(&self) -> JobRun {
        JobRun {
            id: Some(self.id.clone()),
            attempt: self.attempt,
            previous_run_id: self.previous_run_id.clone(),
            // "The name of the trigger that started this job run": the trigger whose
            // firing initiated this run, and nothing for a run a caller started directly.
            trigger_name: self.trigger_name.clone(),
            job_name: Some(self.job_name.clone()),
            job_mode: Some(self.job_mode.clone()),
            job_run_queuing_enabled: Some(self.job_run_queuing_enabled),
            started_on: Some(to_date_time(self.started_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            completed_on: self.completed_on.map(to_date_time),
            job_run_state: Some(self.state.clone()),
            arguments: Some(self.arguments.clone()),
            error_message: self.error_message.clone(),
            // "A job run that was used in the predicate of a conditional trigger that
            // triggered this job run" (`Predecessor`): the runs that preceded this one, which
            // is the EMPTY list for a run nothing preceded rather than no list at all. A run
            // this one retries is not one of them — that is `PreviousRunId`.
            predecessor_runs: Some(self.predecessors.clone()),
            // `AllocatedCapacity` is the same quantity as `MaxCapacity` — "This field is
            // deprecated. Use MaxCapacity instead" — so it reports the same number of
            // whole DPUs, and the model's own default of 0 for a run with no capacity.
            allocated_capacity: self.max_capacity.map_or(0, |capacity| capacity as i32),
            execution_time: self.execution_time_seconds(),
            timeout: Some(self.timeout),
            max_capacity: self.max_capacity,
            worker_type: self.worker_type.clone(),
            number_of_workers: self.number_of_workers,
            security_configuration: self.security_configuration.clone(),
            // "The name of the log group for secure logging": the group this run's logs are
            // written under, resolved when the run started.
            log_group_name: Some(self.log_group_name.clone()),
            notification_property: self.notification_property.clone(),
            glue_version: Some(self.glue_version.clone()),
            // "The total time each executor ran during the lifecycle of a job run in
            // seconds, multiplied by a DPU factor": the script runs in this process rather
            // than on allocated DPUs, so no executor ran for a figure to be of.
            dpu_seconds: None,
            execution_class: self.execution_class.clone(),
            maintenance_window: self.maintenance_window.clone(),
            // `ProfileName` names a usage profile, which neither `CreateJob` nor
            // `StartJobRun` can set.
            profile_name: None,
            // "For example, when a job run is in a WAITING state as a result of job run
            // queuing, the field has the reason why": this run was never queued, and the
            // reason it failed is its `ErrorMessage`.
            state_detail: None,
            execution_role_session_policy: self.execution_role_session_policy.clone(),
        }
    }
}
