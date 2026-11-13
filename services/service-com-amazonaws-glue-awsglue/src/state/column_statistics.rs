// SPDX-License-Identifier: CC0-1.0

//! The column-statistics records of a table: the task runs started for it, the task settings
//! a create recorded, and the states a run moves through.
//!
//! WHAT a run computes and where it reads it from live beside this
//! ([`super::column_statistics_source`], [`super::column_statistics_values`]): a run is
//! performed where it is started, carries its outcome, and publishes the statistics it
//! computed onto the table when the clock settles it.

use super::*;

/// One column statistics task run, as `StartColumnStatisticsTaskRun` recorded it.
///
/// A run carries the table it covers and the configuration the request named, plus the
/// members only a run has: its id, its status and the instants it moved between
/// statuses.
#[derive(Debug, Clone)]
pub struct ColumnStatisticsTaskRunEntry {
    /// `ColumnStatisticsTaskRun$ColumnStatisticsTaskRunId`, minted by
    /// [`new_column_statistics_task_run_id`].
    pub id: String,
    /// The catalog the covered table lives in, as [`resolve_catalog_id`] resolved it.
    pub catalog_id: String,
    /// The covered database's name, folded the way the catalog stores it.
    pub database_name: String,
    /// The covered table's name, folded the way the catalog stores it.
    pub table_name: String,
    /// The columns statistics are generated for: the ones the request listed, else every
    /// column the stored table has ("If none is supplied, all column names for the table
    /// will be used by default").
    pub column_name_list: Vec<String>,
    /// `ColumnStatisticsTaskRun$Role`: "The IAM role that the service assumes to generate
    /// statistics."
    pub role: String,
    /// `ColumnStatisticsTaskRun$SampleSize`: "The percentage of rows used to generate
    /// statistics", the request's value, whose own default of 0 means the entire table.
    pub sample_size: f64,
    pub security_configuration: Option<String>,
    /// One of the modelled `ColumnStatisticsState` values.
    pub status: String,
    /// The instant the clock owes this run its next status; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// `CreationTime`: "The time that this task was created."
    pub creation_time: SystemTime,
    /// `StartTime`: "The start time of the task."
    pub start_time: SystemTime,
    /// `LastUpdated`: "The last point in time when this task was modified."
    pub last_updated: SystemTime,
    /// `EndTime`: "The end time of the task", absent until the run has finished.
    pub end_time: Option<SystemTime>,
    /// `ErrorMessage`: "The error message for the job", absent until one has failed.
    pub error_message: Option<String>,
    /// What this run's computation over the table's data produced: the statistics of the
    /// columns it covers, or the reason it could not compute them.
    ///
    /// The work is done where the run is STARTED (`StartColumnStatisticsTaskRun` performs
    /// it with no store guard held, see
    /// [`compute_column_statistics`]), and this is what it answered — so the terminal
    /// status the clock owes this run is decided by the data, and the statistics a
    /// successful run publishes onto the table are the ones its own reading produced.
    pub outcome: ColumnStatisticsRunOutcome,
}

impl ColumnStatisticsTaskRunEntry {
    /// Record a run of the stored table `configuration` describes, in
    /// [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`] with its next status due
    /// [`COLUMN_STATISTICS_TASK_RUN_SETTLES_AFTER`] out.
    ///
    /// `table` is the stored table the run covers, which is where the covered database and
    /// table names come from, and `catalog_id` the catalog it was resolved in.
    pub fn start(
        configuration: ColumnStatisticsRunConfiguration,
        table: &TableEntry,
        catalog_id: &str,
        id: String,
        now: SystemTime,
        outcome: ColumnStatisticsRunOutcome,
    ) -> Self {
        Self {
            id,
            catalog_id: catalog_id.to_string(),
            database_name: table.database_name.clone(),
            table_name: table.name.clone(),
            // The columns the run covers, resolved ONCE
            // ([`ColumnStatisticsRunConfiguration`]) — the same list the run was performed
            // over.
            column_name_list: configuration.column_name_list,
            role: configuration.role,
            sample_size: configuration.sample_size,
            security_configuration: configuration.security_configuration,
            status: COLUMN_STATISTICS_TASK_RUN_STATE_STARTING.to_string(),
            settles_at: Some(
                now.checked_add(COLUMN_STATISTICS_TASK_RUN_SETTLES_AFTER)
                    .unwrap_or(now),
            ),
            creation_time: now,
            start_time: now,
            last_updated: now,
            end_time: None,
            // Nothing has failed yet: a run carries an error message only once it has.
            error_message: None,
            outcome,
        }
    }

    /// Whether this run has finished, by [`TERMINAL_COLUMN_STATISTICS_TASK_RUN_STATES`].
    pub fn is_terminal(&self) -> bool {
        TERMINAL_COLUMN_STATISTICS_TASK_RUN_STATES.contains(&self.status.as_str())
    }

    /// Whether this run is the one in flight for that catalog, database and table: it has
    /// not finished and it covers exactly that table.
    ///
    /// ONE predicate, so the run a start is refused for, the run a settings create is
    /// refused for and the run a stop ends cannot drift apart. `database` and `table` are
    /// already folded the way the catalog stores them ([`fold_name`]).
    pub fn is_running_for(&self, catalog_id: &str, database: &str, table: &str) -> bool {
        !self.is_terminal()
            && self.catalog_id == catalog_id
            && self.database_name == database
            && self.table_name == table
    }

    /// Move this run into the status the clock owed it at `now`, if it owed one, and
    /// answer the statistics that status publishes onto the table.
    ///
    /// The run was due to leave [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`] at
    /// [`Self::settles_at`] whether or not anybody called, and the instants recorded are
    /// that DUE instant, not the instant somebody read it — so two reads of a settled run
    /// report the same run.
    ///
    /// WHICH status it reaches is [`Self::outcome`]: a run whose reading of the table's
    /// data produced statistics succeeded and carries no error message, and one that could
    /// not compute them failed carrying the reason. `Some` answers the statistics of a
    /// successful run, to be stored on the table it covers
    /// ([`ComAmazonawsGlueAwsglueState::settle_due_column_statistics_task_runs`] is the
    /// one caller, because only the store can reach that table).
    #[must_use]
    pub fn settle_if_due(&mut self, now: SystemTime) -> Option<SettledColumnStatistics> {
        let due = self.settles_at?;
        if due > now {
            return None;
        }
        self.settles_at = None;
        self.end_time = Some(due);
        self.last_updated = due;
        match &self.outcome {
            ColumnStatisticsRunOutcome::Computed(statistics) => {
                self.status = COLUMN_STATISTICS_TASK_RUN_STATE_SUCCEEDED.to_string();
                // A run that generated statistics failed at nothing, so it carries no
                // error message at all.
                self.error_message = None;
                Some(SettledColumnStatistics {
                    analyzed_at: due,
                    statistics: statistics.clone(),
                })
            }
            ColumnStatisticsRunOutcome::Failed(reason) => {
                self.status = COLUMN_STATISTICS_TASK_RUN_STATE_FAILED.to_string();
                self.error_message = Some(reason.clone());
                None
            }
        }
    }

    /// Move this run into the state a stop leaves it in.
    ///
    /// Of the modelled `ColumnStatisticsState` values the one that names a run somebody
    /// stopped, which is terminal — so the run owes the clock no further state, its end
    /// instant is the stop, and it carries no failure reason: nothing failed.
    pub fn stop(&mut self, now: SystemTime) {
        self.settles_at = None;
        self.status = COLUMN_STATISTICS_TASK_RUN_STATE_STOPPED.to_string();
        self.end_time = Some(now);
        self.last_updated = now;
    }

    /// This run as the `ExecutionAttempt` a settings read reports: "a run attempt for a
    /// column statistics task run."
    ///
    /// Every member is this run's own — its id, the status it stands in, the instant it
    /// started and the reason it failed, if it did.
    pub fn to_execution_attempt(&self) -> ExecutionAttempt {
        ExecutionAttempt {
            status: Some(
                if self.status == COLUMN_STATISTICS_TASK_RUN_STATE_FAILED {
                    COLUMN_STATISTICS_EXECUTION_STATUS_FAILED.to_string()
                } else {
                    COLUMN_STATISTICS_EXECUTION_STATUS_STARTED.to_string()
                },
            ),
            column_statistics_task_run_id: Some(self.id.clone()),
            // "A timestamp when the last column statistics task run occurred": the
            // instant the attempt started.
            execution_timestamp: Some(to_date_time(self.start_time)),
            error_message: self.error_message.clone(),
        }
    }

    /// This run's place in a listing of runs, as that listing's continuation token renders
    /// it: the shared history cursor ([`history_page_cursor`]) over the instant the run
    /// started and its own id.
    ///
    /// ONE copy, so the two listings that page over these runs
    /// (`GetColumnStatisticsTaskRuns` over one table, `ListColumnStatisticsTaskRuns` over
    /// the account) cannot drift apart on the token format. Ordering runs by this key
    /// DESCENDING is the newest-first order both pages are served in, and comparing a token
    /// against it is what resumes a page — so any string a request sends names a
    /// well-defined place in that order.
    pub fn page_cursor(&self) -> String {
        history_page_cursor(self.start_time, &self.id)
    }

    /// Rebuild the modelled `ColumnStatisticsTaskRun` output shape from what is stored.
    pub fn to_column_statistics_task_run(&self) -> ColumnStatisticsTaskRun {
        ColumnStatisticsTaskRun {
            // "The Amazon Web Services account ID" the run belongs to.
            customer_id: Some(::engine::emulator_account_id().to_string()),
            column_statistics_task_run_id: Some(self.id.clone()),
            database_name: Some(self.database_name.clone()),
            table_name: Some(self.table_name.clone()),
            column_name_list: Some(self.column_name_list.clone()),
            catalog_id: Some(self.catalog_id.clone()),
            role: Some(self.role.clone()),
            sample_size: self.sample_size,
            security_configuration: self.security_configuration.clone(),
            // "The number of workers used to generate column statistics" and "the
            // calculated DPU usage in seconds for all autoscaled workers": the statistics
            // are computed in this process rather than by a fleet this emulator
            // provisions, so neither figure has anything to report and each stays at the
            // model's own default, and no `WorkerType` was used either.
            number_of_workers: 0,
            worker_type: None,
            dpu_seconds: 0.0,
            computation_type: Some(COLUMN_STATISTICS_COMPUTATION_TYPE_FULL.to_string()),
            status: Some(self.status.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            last_updated: Some(to_date_time(self.last_updated)),
            start_time: Some(to_date_time(self.start_time)),
            end_time: self.end_time.map(to_date_time),
            error_message: self.error_message.clone(),
        }
    }
}

/// What ONE column statistics task run was asked to do, however it was asked for: the
/// columns it covers and the three members that configure the generation.
///
/// A run is started in two ways — `StartColumnStatisticsTaskRun` asks for one directly, and
/// `CreateColumnStatisticsTaskSettings` registers the configuration its runs use and starts
/// the first of them — and this is what both resolve to, so a run started either way carries
/// the same members and is reported the same way.
#[derive(Debug, Clone)]
pub struct ColumnStatisticsRunConfiguration {
    /// The columns the run covers, already resolved against the stored table.
    pub column_name_list: Vec<String>,
    /// The role the generation is performed as.
    pub role: String,
    /// "The percentage of rows used to generate statistics", whose own default of 0 means
    /// the entire table.
    pub sample_size: f64,
    /// The security configuration the generation's logs are encrypted under, where one was
    /// named.
    pub security_configuration: Option<String>,
}

impl ColumnStatisticsRunConfiguration {
    /// The configuration a `StartColumnStatisticsTaskRun` request asks for of the stored
    /// table it addressed.
    pub fn of_start_request(
        input: &StartColumnStatisticsTaskRunInput,
        table: &TableEntry,
    ) -> Self {
        Self {
            column_name_list: covered_column_names(input, table),
            role: input.role.clone(),
            sample_size: input.sample_size,
            security_configuration: input.security_configuration.clone(),
        }
    }

    /// The configuration the stored settings of a table state for its runs: the columns
    /// those settings list, else every column the stored table declares — the same default a
    /// start that lists none takes ([`table_column_names`]) — and the three members the
    /// settings record.
    pub fn of_settings(settings: &ColumnStatisticsTaskSettingsEntry, table: &TableEntry) -> Self {
        Self {
            column_name_list: settings
                .column_name_list
                .clone()
                .filter(|names| !names.is_empty())
                .unwrap_or_else(|| table_column_names(table)),
            role: settings.role.clone(),
            sample_size: settings.sample_size,
            security_configuration: settings.security_configuration.clone(),
        }
    }
}

/// The columns a `StartColumnStatisticsTaskRun` request covers of the stored table it
/// addressed: the ones it listed, else every column the STORED table declares ("If none is
/// supplied, all column names for the table will be used by default" — an EMPTY list
/// supplies no column name either, so it takes the same default rather than covering
/// nothing).
///
/// ONE resolution, so the columns the run is PERFORMED over
/// ([`ColumnStatisticsRunPlan::of`]) and the columns the recorded run REPORTS
/// ([`ColumnStatisticsTaskRunEntry::column_name_list`]) cannot drift apart.
pub fn covered_column_names(
    input: &StartColumnStatisticsTaskRunInput,
    table: &TableEntry,
) -> Vec<String> {
    input
        .column_name_list
        .clone()
        .filter(|names| !names.is_empty())
        .unwrap_or_else(|| table_column_names(table))
}

/// Every column name the stored table has, in the order the table declares them: the
/// default column list of a column statistics task run ("If none is supplied, all column
/// names for the table will be used by default").
///
/// The names come from the table's `StorageDescriptor$Columns`, which is where "a list of
/// the `Columns` in the table" lives; a table stored without a storage descriptor
/// declares no columns and so yields none.
///
/// `PartitionKeys` are deliberately NOT part of the list. They are "a list of columns by
/// which the table is partitioned", and the statistics of such a column are addressed per
/// partition — `UpdateColumnStatisticsForPartition` and
/// `GetColumnStatisticsForPartition` take `PartitionValues` — so they are not what a
/// table-level statistics run covers.
pub fn table_column_names(table: &TableEntry) -> Vec<String> {
    table
        .storage_descriptor
        .as_ref()
        .and_then(|descriptor| descriptor.columns.as_ref())
        .map(|columns| {
            columns
                .iter()
                .map(|column| column.name.clone())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default()
}

/// The statistics the catalog holds for ONE column of a stored table, or `None` when it
/// holds none for that column.
///
/// ONE lookup, so every operation that addresses a column's statistics — the read, the
/// write and the delete — resolves a column name the same way: folded, because the catalog
/// addresses a column name without regard to case
/// ([`TableEntry::column_statistics`]).
pub fn stored_column_statistics<'a>(
    table: &'a TableEntry,
    column_name: &str,
) -> Option<&'a ColumnStatistics> {
    table.column_statistics.get(&fold_name(column_name))
}

/// Store `statistics` as the statistics of the column they name, replacing whatever the
/// table held for that column: there is at most one entry per column
/// ([`TableEntry::column_statistics`]), so a later write of the same column supersedes the
/// earlier one.
pub fn store_column_statistics(table: &mut TableEntry, statistics: ColumnStatistics) {
    table
        .column_statistics
        .insert(fold_name(&statistics.column_name), statistics);
}

/// Whether storing statistics for this column would take the table past
/// [`MAX_COLUMN_STATISTICS_PER_TABLE`].
///
/// Only a column the table holds NO statistics for yet can reach the ceiling: one it
/// already holds is replaced rather than added, so a client that refreshes the statistics
/// it stored is never refused.
pub fn column_statistics_at_capacity(table: &TableEntry, column_name: &str) -> bool {
    stored_column_statistics(table, column_name).is_none()
        && table.column_statistics.len() >= MAX_COLUMN_STATISTICS_PER_TABLE
}

/// Take the statistics the catalog holds for one column of a stored table out of it,
/// answering `None` when it held none — in which case nothing is removed.
pub fn remove_column_statistics(
    table: &mut TableEntry,
    column_name: &str,
) -> Option<ColumnStatistics> {
    table.column_statistics.remove(&fold_name(column_name))
}

/// The `ErrorDetail$ErrorCode` the catalog reports a column it holds no statistics for
/// under: the service's own name for an entity that does not exist, which is the error
/// class every statistics operation models for a thing it cannot resolve
/// (`EntityNotFoundException`, "a specified entity does not exist").
pub const COLUMN_STATISTICS_NOT_FOUND_ERROR_CODE: &str = "EntityNotFoundException";

/// The per-column failure a statistics read reports for a requested column the catalog
/// holds no statistics for: the column's own name, and the detail that says why it was not
/// satisfied ([`messages::COLUMN_STATISTICS_NOT_FOUND`]).
///
/// The read is a PARTIAL-SUCCESS operation — it answers the columns it could satisfy and
/// names the rest in `Errors` — so this is how an unsatisfied column reaches the caller,
/// built where every per-item failure of this catalog is built
/// ([`catalog_error_detail`]) so the `__type` tag the Data Catalog writes onto a nested
/// `ErrorDetail` is on this one too. `column_name` is the name the REQUEST asked for,
/// spelled as it sent it.
pub fn column_statistics_not_found_error(column_name: &str) -> ColumnError {
    ColumnError {
        column_name: Some(column_name.to_string()),
        error: Some(catalog_error_detail(
            COLUMN_STATISTICS_NOT_FOUND_ERROR_CODE,
            messages::COLUMN_STATISTICS_NOT_FOUND.to_string(),
        )),
    }
}

/// `ColumnStatisticsTaskSettings$ScheduleType` of settings created with a `Schedule`: of
/// the modelled `ScheduleType` (`CRON | AUTO`), the cron one, because the schedule those
/// settings carry IS a cron expression ("A schedule for running the column statistics,
/// specified in CRON syntax" — `CreateColumnStatisticsTaskSettingsRequest$Schedule`).
/// Settings created without a schedule carry no schedule type at all: nothing stated one.
pub const COLUMN_STATISTICS_SCHEDULE_TYPE_CRON: &str = "CRON";

/// `ColumnStatisticsTaskSettings$SettingSource` of settings a create wrote for one table:
/// of the modelled `SettingSource` (`CATALOG | TABLE`), the table one, because the create
/// addresses a single table (`DatabaseName` plus `TableName`, both `@required`) rather
/// than the catalog those settings would otherwise be inherited from.
pub const COLUMN_STATISTICS_SETTING_SOURCE_TABLE: &str = "TABLE";

/// `ExecutionAttempt$Status` of an attempt that ended in a column statistics task run's
/// failure state: of the modelled `ExecutionStatus` (`FAILED | STARTED`), the failed one.
pub const COLUMN_STATISTICS_EXECUTION_STATUS_FAILED: &str = "FAILED";

/// `ExecutionAttempt$Status` of any other attempt. The modelled `ExecutionStatus` carries
/// only the two values quoted on [`COLUMN_STATISTICS_EXECUTION_STATUS_FAILED`], so an
/// attempt that has not failed is reported as the run it started.
pub const COLUMN_STATISTICS_EXECUTION_STATUS_STARTED: &str = "STARTED";

/// The column statistics task settings of ONE table, as
/// `CreateColumnStatisticsTaskSettings` recorded them.
///
/// Only what the client STATED is held here. The members that address the settings —
/// their catalog, database and table — are the stored table's own and are read off it
/// when a read projects the modelled shape ([`Self::to_column_statistics_task_settings`]),
/// so one record cannot disagree with the table it hangs on.
#[derive(Debug, Clone)]
pub struct ColumnStatisticsTaskSettingsEntry {
    /// `ColumnStatisticsTaskSettings$Role`: "The role used for running the column
    /// statistics."
    pub role: String,
    /// `ColumnStatisticsTaskSettings$SampleSize`: "The percentage of data to sample", the
    /// request's value, whose own default is 0.
    pub sample_size: f64,
    /// The cron expression of `Schedule`, or `None` when the create stated no schedule.
    pub schedule_expression: Option<String>,
    /// `ColumnNameList`: "A list of column names for which to run statistics", or `None`
    /// when the create listed none — the settings record what was configured, and the
    /// default of a run that names no column is resolved by the run
    /// ([`table_column_names`]).
    pub column_name_list: Option<Vec<String>>,
    /// `SecurityConfiguration`: "Name of the security configuration that is used to
    /// encrypt CloudWatch logs."
    pub security_configuration: Option<String>,
    /// `CreateColumnStatisticsTaskSettingsRequest$Tags`: "A map of tags." Stored rather
    /// than served, for the reason [`CustomEntityTypeEntry::tags`] gives: no `ResourceArn`
    /// this service tags ([`ComAmazonawsGlueAwsglueState::resource_tags`]) addresses a
    /// settings record, so nothing reads them back yet — and dropping them would lose what
    /// the create was told.
    pub tags: HashMap<String, String>,
}

impl ColumnStatisticsTaskSettingsEntry {
    /// Record the settings a `CreateColumnStatisticsTaskSettings` request states.
    pub fn from_create_input(input: &CreateColumnStatisticsTaskSettingsInput) -> Self {
        Self {
            role: input.role.clone(),
            sample_size: input.sample_size,
            schedule_expression: input.schedule.clone(),
            // An empty list states no column either, so it is recorded as none rather
            // than as settings that cover nothing.
            column_name_list: input
                .column_name_list
                .clone()
                .filter(|names| !names.is_empty()),
            security_configuration: input.security_configuration.clone(),
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Rebuild the modelled `ColumnStatisticsTaskSettings` shape from what is stored.
    ///
    /// `table` is the stored table these settings hang on, which is where their catalog,
    /// database and table names come from, and `last_attempt` the last column statistics
    /// task run started for that table, if one has been: "the last `ExecutionAttempt` for
    /// the column statistics task run" is that run, so it is read off the stored runs
    /// rather than held here.
    pub fn to_column_statistics_task_settings(
        &self,
        table: &TableEntry,
        last_attempt: Option<&ColumnStatisticsTaskRunEntry>,
    ) -> ColumnStatisticsTaskSettings {
        ColumnStatisticsTaskSettings {
            database_name: Some(table.database_name.clone()),
            table_name: Some(table.name.clone()),
            catalog_id: Some(table.catalog_id.clone()),
            // A stated schedule is a schedule the service runs, so it is reported in the
            // scheduled state, the same state a stated crawler schedule is written in
            // ([`stated_schedule_state`]). No operation suspends a column-statistics
            // schedule, so this record never leaves that state.
            schedule: self
                .schedule_expression
                .as_ref()
                .map(|expression| Schedule {
                    schedule_expression: Some(expression.clone()),
                    state: Some(stated_schedule_state(Some(expression))),
                }),
            schedule_type: self
                .schedule_expression
                .as_ref()
                .map(|_| COLUMN_STATISTICS_SCHEDULE_TYPE_CRON.to_string()),
            setting_source: Some(COLUMN_STATISTICS_SETTING_SOURCE_TABLE.to_string()),
            column_name_list: self.column_name_list.clone(),
            role: Some(self.role.clone()),
            sample_size: self.sample_size,
            security_configuration: self.security_configuration.clone(),
            last_execution_attempt: last_attempt
                .map(ColumnStatisticsTaskRunEntry::to_execution_attempt),
        }
    }
}

/// The `ColumnStatisticsState` a column statistics task run is recorded in by
/// `StartColumnStatisticsTaskRun`: the first of the modelled enum
/// (`STARTING | RUNNING | SUCCEEDED | FAILED | STOPPED`), because the run has been
/// accepted and nothing has begun generating statistics yet.
pub const COLUMN_STATISTICS_TASK_RUN_STATE_STARTING: &str = "STARTING";
/// See [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`].
pub const COLUMN_STATISTICS_TASK_RUN_STATE_FAILED: &str = "FAILED";

/// See [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`]: the state a run reaches once it has
/// generated the statistics of the columns it covers.
pub const COLUMN_STATISTICS_TASK_RUN_STATE_SUCCEEDED: &str = "SUCCEEDED";

/// See [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`]: the state
/// `StopColumnStatisticsTaskRun` leaves the run it stopped in.
pub const COLUMN_STATISTICS_TASK_RUN_STATE_STOPPED: &str = "STOPPED";

/// The `ColumnStatisticsState` values a column statistics task run never moves out of
/// again: of the modelled enum quoted on
/// [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`], the ones that name a finished run. The
/// other two (`STARTING`, `RUNNING`) each still owe a further state.
pub const TERMINAL_COLUMN_STATISTICS_TASK_RUN_STATES: [&str; 3] = [
    COLUMN_STATISTICS_TASK_RUN_STATE_SUCCEEDED,
    COLUMN_STATISTICS_TASK_RUN_STATE_FAILED,
    COLUMN_STATISTICS_TASK_RUN_STATE_STOPPED,
];

/// The statistics a settled column statistics task run publishes onto the table it
/// covers, and the instant it generated them.
///
/// The instant is the run's own DUE instant, which is what the run reports as its end:
/// `ColumnStatistics$AnalyzedTime` is "the timestamp of when column statistics were
/// generated", so it is the instant the run finished rather than the instant some later
/// read materialised it — two reads of one settled run therefore report one instant.
#[derive(Debug, Clone)]
pub struct SettledColumnStatistics {
    /// When the statistics were generated.
    pub analyzed_at: SystemTime,
    /// The statistics of each covered column that has them.
    pub statistics: Vec<ComputedColumnStatistics>,
}

/// How long a column statistics task run stays in
/// [`COLUMN_STATISTICS_TASK_RUN_STATE_STARTING`] before the clock owes it its terminal
/// state.
///
/// CHOSEN, not documented: Glue publishes no duration for a column statistics task run,
/// so this is the emulator's own span — deliberately not zero, so a caller that reads
/// `GetColumnStatisticsTaskRun` straight after starting one still observes the run in
/// flight, and the refusal that goes with a run in flight. The due instant is stored on
/// the run and materialised by whichever later call reads it; nothing sleeps.
pub const COLUMN_STATISTICS_TASK_RUN_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// `ColumnStatisticsTaskRun$ComputationType` of a run `StartColumnStatisticsTaskRun`
/// started: of the modelled `ComputationType` (`FULL | INCREMENTAL`), the full
/// computation. The request carries no member that selects an incremental computation —
/// it names the table and the columns to "generate statistics" for — so a run started
/// this way computes the whole statistic set rather than an increment on an earlier one.
pub const COLUMN_STATISTICS_COMPUTATION_TYPE_FULL: &str = "FULL";

/// How many column statistics task runs this account keeps.
///
/// CHOSEN: Glue publishes no retention for these records, so this is the emulator's own
/// window, for the same reason as [`MAX_RUNS_PER_JOB`]: it bounds
/// [`ComAmazonawsGlueAwsglueState::column_statistics_task_runs`] and every walk over it.
/// The OLDEST record is dropped first, so the runs a caller is most likely to read are
/// kept.
pub const MAX_COLUMN_STATISTICS_TASK_RUNS: usize = 100;

/// How many column statistics task run ids this process has minted. A nonce source for
/// [`new_column_statistics_task_run_id`], exactly as [`MINTED_JOB_RUN_IDS`] is for a job
/// run id.
pub(super) static MINTED_COLUMN_STATISTICS_TASK_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `ColumnStatisticsTaskRunId`: the id the service assigns a column statistics
/// task run when it starts.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is inside `HashString`'s
/// `length 1..255`. The bits come from [`mint_hex_id`], as a job run id's do, so two runs
/// started in the same nanosecond cannot be given the same id. It is stored on the run,
/// so every later read of that run reports the same id.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_column_statistics_task_run_id() -> String {
    mint_hex_id(
        &MINTED_COLUMN_STATISTICS_TASK_RUN_IDS,
        COLUMN_STATISTICS_TASK_RUN_ID_WORDS,
    )
}

/// How many 64-bit words a column statistics task run id carries: two, i.e. 128 bits
/// rendered as 32 lowercase hexadecimal characters.
pub(super) const COLUMN_STATISTICS_TASK_RUN_ID_WORDS: usize = 2;

