// SPDX-License-Identifier: CC0-1.0

//! The RUN of an upload job: the processing `StartUploadJob` commissions, the profile
//! records that processing ingests into the domain, and the completion the clock owes the
//! job afterwards.
//!
//! `StartUploadJob` "starts the processing of an upload job to ingest profile data", and
//! the data it ingests is the file written to the job's upload target
//! ([`super::upload_job_file`]) read through the job's own `Fields` — "the mapping between
//! CSV Columns and Profile Object attributes" — and de-duplicated by its `UniqueKey`, "the
//! unique key columns for de-duping the profiles used to map data to the profile". So the
//! ingest is performed here, by the call that starts the job: the profiles it creates and
//! updates are in the domain's own store, which is what makes every later read of them —
//! a search, a get, a listing — answer from records that really exist.
//!
//! What the CLOCK owes a started job is its COMPLETION. A job reports `IN_PROGRESS` until
//! [`UPLOAD_JOB_PROCESSING_SPAN`] after the call that started it, and its terminal status,
//! `CompletedAt` and `ResultsSummary` from that instant on, so a caller that reads a job
//! back immediately still observes the processing state the start call put it in. The
//! outcome is computed once, by the start call, and the instant it falls due is stored, so
//! no handler ever sleeps and two reads of the same job at the same instant agree.
//!
//! A CALL can end a run before the clock does: `StopUploadJob` "stops the processing of an
//! upload job", which ends the run at the instant of the stop — the job reports `STOPPED`
//! from then on and the completion that run was owed never falls due.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{ObjectTypeField, Profile, ResultsSummary};

use super::{
    apply_profile_target, new_profile_id, standard_search_key_for_target, Domain, ProfileAction,
    StoredProfile, StoredUploadJob, ACTION_CREATED, ACTION_UPDATED, MAX_PROFILES_PER_DOMAIN,
};

/// The `UploadJobStatus` of a job whose processing is under way: "IN_PROGRESS: The upload
/// job is currently in progress, ingesting and processing the profile data."
pub const UPLOAD_JOB_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `UploadJobStatus` of a job that processed every record it was given: "SUCCEEDED:
/// The upload job has successfully completed the ingestion and processing of all profile
/// data."
pub const UPLOAD_JOB_SUCCEEDED: &str = "SUCCEEDED";

/// The `UploadJobStatus` of a job that processed some of its records and failed others —
/// the status between `SUCCEEDED` and `FAILED`.
pub const UPLOAD_JOB_PARTIALLY_SUCCEEDED: &str = "PARTIALLY_SUCCEEDED";

/// The `UploadJobStatus` of a job that ingested nothing: "FAILED: The upload job has
/// failed to complete."
pub const UPLOAD_JOB_FAILED: &str = "FAILED";

/// The `UploadJobStatus` of a job whose processing a caller ended: "STOPPED: The upload
/// job has been manually stopped or terminated before completion."
pub const UPLOAD_JOB_STOPPED: &str = "STOPPED";

/// The `StatusReason` of a job whose data could not be ingested: "VALIDATION_FAILURE: The
/// upload job has encountered an error or issue and was unable to complete the profile
/// data ingestion."
pub const UPLOAD_JOB_VALIDATION_FAILURE: &str = "VALIDATION_FAILURE";

/// How long after the call that started it a job's processing is treated as finished,
/// which is when the job reports its terminal status and its `CompletedAt`.
///
/// AWS documents no duration for an upload job's processing, so this is a CHOSEN span. It
/// is deliberately non-zero: a caller that reads the job back immediately after starting
/// it must still observe the `IN_PROGRESS` state the start call reported.
pub const UPLOAD_JOB_PROCESSING_SPAN: Duration = Duration::from_secs(3);

/// Emulator resource guard, NOT an AWS quota: the most records one run reads out of a
/// job's upload file.
///
/// The file is caller content held in this process, so the number of profiles ONE start
/// call can write is bounded. A record past the cap is counted as a failed record rather
/// than silently dropped, so the summary still accounts for every record of the file.
pub const MAX_UPLOAD_JOB_RECORDS: usize = 10_000;

/// What separates the unique-key COLUMNS of a job carrying more than one: `UniqueKey` is
/// "the unique key columns for de-duping the profiles", a single text member, so several
/// columns arrive in it as a list.
const UNIQUE_KEY_SEPARATOR: char = ',';

/// The counters of one run, as `ResultsSummary` reports them: "the number of updated,
/// created, and failed records".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UploadJobCounters {
    /// `CreatedRecords`: the records that created a profile.
    pub created: i64,
    /// `UpdatedRecords`: the records that updated a profile the unique key matched.
    pub updated: i64,
    /// `FailedRecords`: the records that could not be processed.
    pub failed: i64,
}

impl UploadJobCounters {
    /// How many records this run ingested — created plus updated.
    fn ingested(self) -> i64 {
        self.created.saturating_add(self.updated)
    }
}

/// What one run of an upload job came to: the status it ends in, why, and the records it
/// accounted for.
#[derive(Debug, Clone, Default)]
pub struct UploadJobOutcome {
    /// The terminal `UploadJobStatus` the job reports once its processing falls due.
    pub status: String,
    /// `StatusReason`, set only by a status that has one.
    pub status_reason: Option<String>,
    /// The counters the run accounted for, or `None` for a run that read no record at all
    /// — whose summary therefore carries no counter, exactly as a job that has processed
    /// nothing does.
    pub counters: Option<UploadJobCounters>,
}

impl UploadJobOutcome {
    /// The outcome of a run that read records: `SUCCEEDED` when every record was
    /// ingested, `PARTIALLY_SUCCEEDED` when some were and some were not, and `FAILED`
    /// when none were.
    fn of(counters: UploadJobCounters) -> Self {
        let (status, status_reason) = match (counters.ingested(), counters.failed) {
            (0, _) => (UPLOAD_JOB_FAILED, Some(UPLOAD_JOB_VALIDATION_FAILURE)),
            (_, 0) => (UPLOAD_JOB_SUCCEEDED, None),
            _ => (UPLOAD_JOB_PARTIALLY_SUCCEEDED, None),
        };
        Self {
            status: status.to_string(),
            status_reason: status_reason.map(str::to_string),
            counters: Some(counters),
        }
    }

    /// The outcome of a run with nothing to read: no file was written to the job's upload
    /// target, or what was written holds no record this run could read. Nothing was
    /// processed, so the job failed validation and its summary carries no counter.
    fn nothing_to_ingest() -> Self {
        Self {
            status: UPLOAD_JOB_FAILED.to_string(),
            status_reason: Some(UPLOAD_JOB_VALIDATION_FAILURE.to_string()),
            counters: None,
        }
    }
}

/// The processing one `StartUploadJob` call commissioned: when it was started, when it
/// falls due, and what it came to.
///
/// The outcome is computed by the start call — the ingest it performs is what writes the
/// profiles — and held here until the span falls due, so the job's reported status moves
/// with the clock and with nothing else.
#[derive(Debug, Clone)]
pub struct UploadJobRun {
    /// The instant the call that started the processing ran.
    pub started_at: SystemTime,
    /// The instant the processing falls due, one [`UPLOAD_JOB_PROCESSING_SPAN`] later.
    pub due_at: SystemTime,
    /// What the run came to, reported from `due_at` on.
    pub outcome: UploadJobOutcome,
}

impl UploadJobRun {
    /// Whether this run has fallen due as of `now`.
    pub fn is_due(&self, now: SystemTime) -> bool {
        now >= self.due_at
    }
}

/// One upload job as the wire reports it at `now`: the four members whose value depends on
/// how far the job's processing has got.
#[derive(Debug, Clone)]
pub struct ReportedUploadJob {
    /// `Status`.
    pub status: String,
    /// `StatusReason`.
    pub status_reason: Option<String>,
    /// `CompletedAt`: "the timestamp when the upload job was completed", so unset until
    /// its processing has fallen due.
    pub completed_at: Option<SystemTime>,
    /// `ResultsSummary`: present for every stored job, with its counters populated only
    /// once the run that counted them has fallen due.
    pub results_summary: ResultsSummary,
}

impl StoredUploadJob {
    /// This job as the wire reports it at `now`.
    ///
    /// A job with no run reports what the last call to it wrote. A started job reports
    /// `IN_PROGRESS` until its processing falls due, and its run's outcome from then on —
    /// completed at the instant the processing was due, which is a property of the job and
    /// not of the call that happened to read it.
    pub fn reported(&self, now: SystemTime) -> ReportedUploadJob {
        match &self.run {
            Some(run) if run.is_due(now) => ReportedUploadJob {
                status: run.outcome.status.clone(),
                status_reason: run.outcome.status_reason.clone(),
                completed_at: Some(run.due_at),
                results_summary: run.outcome.counters.map_or_else(
                    ResultsSummary::default,
                    |counters| ResultsSummary {
                        created_records: Some(counters.created),
                        updated_records: Some(counters.updated),
                        failed_records: Some(counters.failed),
                    },
                ),
            },
            _ => ReportedUploadJob {
                status: self.status.clone(),
                status_reason: self.status_reason.clone(),
                completed_at: self.completed_at,
                results_summary: ResultsSummary::default(),
            },
        }
    }

    /// The `UploadJobStatus` this job reports at `now` — what a caller branching on the
    /// job's state, this service included, reads.
    pub fn reported_status(&self, now: SystemTime) -> String {
        self.reported(now).status
    }
}

impl Domain {
    /// Start the upload job `job_id`: ingest its file into this domain's profiles and hold
    /// the outcome until the processing span falls due.
    ///
    /// The caller has already established that the job is stored and startable; `now` is
    /// the instant of the call.
    pub fn start_upload_job(&mut self, job_id: &str, now: SystemTime) {
        let outcome = self.ingest_upload_job_file(job_id, now);
        let Some(job) = self.upload_jobs.get_mut(job_id) else {
            return;
        };
        // The stored status is the one a CALL put the job in; the terminal status the run
        // came to is reported once the clock owes it.
        job.status = UPLOAD_JOB_IN_PROGRESS.to_string();
        job.status_reason = None;
        job.completed_at = None;
        job.run = Some(UploadJobRun {
            started_at: now,
            due_at: now + UPLOAD_JOB_PROCESSING_SPAN,
            outcome,
        });
    }

    /// Stop the processing of the upload job `job_id`: the run it is performing ends at
    /// `now`, so the job reports `STOPPED` from this call on and the completion the clock
    /// owed that run never falls due.
    ///
    /// The caller has already established that the job is stored and that its processing is
    /// under way; `now` is the instant of the call. What the interrupted run had already
    /// accounted for is kept — those profiles are in this domain — so the stopped job
    /// reports the records it processed before it was stopped.
    pub fn stop_upload_job(&mut self, job_id: &str, now: SystemTime) {
        let Some(job) = self.upload_jobs.get_mut(job_id) else {
            return;
        };
        let run = job.run.as_ref();
        let counters = run.and_then(|run| run.outcome.counters);
        let started_at = run.map_or(now, |run| run.started_at);
        job.status = UPLOAD_JOB_STOPPED.to_string();
        // No modelled `StatusReason` describes a stop: the two the model defines are the
        // failures (`VALIDATION_FAILURE`, `INTERNAL_FAILURE`).
        job.status_reason = None;
        job.completed_at = Some(now);
        // The run is kept, due as of this instant, so the job's answer comes from the one
        // reporting path every upload-job read uses and two reads of it agree. Both sides of
        // `StoredUploadJob::reported` therefore carry the same stopped state, and the run
        // carries one thing the stored members cannot: the counters of what it processed.
        job.run = Some(UploadJobRun {
            started_at,
            due_at: now,
            outcome: UploadJobOutcome {
                status: UPLOAD_JOB_STOPPED.to_string(),
                status_reason: None,
                counters,
            },
        });
    }

    /// Ingest the records of one job's upload file into this domain's profiles, answering
    /// what the run came to.
    ///
    /// The file is a CSV whose first record names its columns, which is what the job's
    /// `Fields` map ("the mapping between CSV Columns and Profile Object attributes")
    /// addresses: every field whose column the file carries writes that column's value onto
    /// the member its `Target` names. The job's `UniqueKey` columns identify the profile a
    /// record belongs to, so a record whose key matches a profile already indexed under it,
    /// or one already written by this run, UPDATES that profile instead of creating a second
    /// one.
    fn ingest_upload_job_file(&mut self, job_id: &str, now: SystemTime) -> UploadJobOutcome {
        let Some(job) = self.upload_jobs.get(job_id) else {
            return UploadJobOutcome::nothing_to_ingest();
        };
        let (Some(file), fields, unique_key) = (
            job.uploaded_file.clone(),
            job.fields.clone(),
            job.unique_key.clone(),
        ) else {
            // Nothing was ever written to the job's upload target, so the job has no
            // profile data to ingest.
            return UploadJobOutcome::nothing_to_ingest();
        };

        let mut reader = ::csv::ReaderBuilder::new()
            .has_headers(true)
            .flexible(true)
            .from_reader(&file[..]);
        let Ok(header) = reader.headers().cloned() else {
            // The bytes are not a CSV this run can read the columns of, so no record of
            // it maps onto anything.
            return UploadJobOutcome::nothing_to_ingest();
        };
        let columns: HashMap<&str, usize> = header
            .iter()
            .enumerate()
            .map(|(position, name)| (name.trim(), position))
            .collect();

        // Where each mapped field reads from and what it writes onto: resolved once, from
        // the header, rather than per record.
        let mapped = mapped_columns(&fields, &columns);
        let keyed = key_columns(&unique_key, &fields, &columns);

        let mut counters = UploadJobCounters::default();
        // The profiles this run has already written, by the identity its unique key
        // columns derived: two records carrying the same key are one profile.
        let mut written: BTreeMap<String, String> = BTreeMap::new();
        let mut records = 0_usize;
        for record in reader.records() {
            records = records.saturating_add(1);
            let Ok(record) = record else {
                counters.failed = counters.failed.saturating_add(1);
                continue;
            };
            if records > MAX_UPLOAD_JOB_RECORDS {
                counters.failed = counters.failed.saturating_add(1);
                continue;
            }
            let values: Vec<(&str, String)> = mapped
                .iter()
                .filter_map(|(position, target)| {
                    cell(&record, *position).map(|value| (*target, value))
                })
                .collect();
            // The key values that identify the record's profile. A record that carries no
            // value for one of the unique-key columns, or that maps no profile data,
            // identifies no profile: there is nothing to ingest it onto.
            let identity: Option<Vec<String>> = keyed
                .iter()
                .map(|key| cell(&record, key.position))
                .collect();
            let Some(identity) = identity.filter(|key_values| !key_values.is_empty()) else {
                counters.failed = counters.failed.saturating_add(1);
                continue;
            };
            if values.is_empty() {
                counters.failed = counters.failed.saturating_add(1);
                continue;
            }
            // The pairs the key values are searched for under, so a record whose key
            // matches a profile an earlier call stored updates that profile.
            let probes: Vec<(String, String)> = keyed
                .iter()
                .zip(&identity)
                .filter_map(|(key, value)| {
                    key.search_key
                        .map(|search_key| (search_key.to_string(), value.clone()))
                })
                .collect();

            let identity = identity.join(&UNIQUE_KEY_SEPARATOR.to_string());
            let existing = written
                .get(&identity)
                .cloned()
                .or_else(|| self.match_profile(&probes));
            match self.apply_record(existing.as_deref(), &values, now) {
                Some((profile_id, created)) => {
                    if created {
                        counters.created = counters.created.saturating_add(1);
                    } else {
                        counters.updated = counters.updated.saturating_add(1);
                    }
                    written.insert(identity, profile_id);
                }
                None => counters.failed = counters.failed.saturating_add(1),
            }
        }

        if records == 0 {
            // A file holding only its header row carries no profile data.
            return UploadJobOutcome::nothing_to_ingest();
        }
        UploadJobOutcome::of(counters)
    }

    /// Write one record's values onto the profile `existing` names, or onto a profile this
    /// creates for it, answering the `ProfileId` that was written and whether this record
    /// created that profile.
    ///
    /// `None` when the record wrote nothing: none of its values named a member of the
    /// standard profile model, or the domain already holds as many profiles as this
    /// emulator keeps for one.
    fn apply_record(
        &mut self,
        existing: Option<&str>,
        values: &[(&str, String)],
        now: SystemTime,
    ) -> Option<(String, bool)> {
        let held = existing
            .and_then(|profile_id| self.profiles.get(profile_id))
            .cloned();
        // Whether the record is written onto a profile this domain already holds decides
        // both which counter accounts for it and which action its history records, so it is
        // read once, off the store.
        let created = held.is_none();
        if created && self.profiles.len() >= MAX_PROFILES_PER_DOMAIN {
            return None;
        }
        let mut stored = held.unwrap_or_else(|| StoredProfile {
            profile: Profile::default(),
            created_at: now,
            last_updated_at: now,
        });
        let mut mapped = false;
        for (target, value) in values {
            mapped |= apply_profile_target(&mut stored.profile, target, value.clone());
        }
        if !mapped {
            return None;
        }
        let profile_id = stored
            .profile
            .profile_id
            .clone()
            .unwrap_or_else(new_profile_id);
        stored.profile.profile_id = Some(profile_id.clone());
        stored.last_updated_at = now;
        self.put_profile(stored);
        // The ingest wrote this profile, so its history records that the same way a call
        // writing a profile does.
        let action = if created { ACTION_CREATED } else { ACTION_UPDATED };
        self.record_profile_action(&profile_id, ProfileAction::on_profile(action), now);
        Some((profile_id, created))
    }
}

/// One cell of a record as a value: the trimmed text it carries, or `None` for a cell the
/// record does not hold or that carries nothing.
fn cell(record: &::csv::StringRecord, position: usize) -> Option<String> {
    let value = record.get(position)?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// The position each mapped field reads from and the `Target` it writes onto, for every
/// field whose column the file carries and that names a target.
///
/// A field is named by its entry in the `Fields` map, which is the CSV column it maps; a
/// field whose entry names no column of the file is addressed by the last segment of its
/// `Source`, the only other column name it spells.
fn mapped_columns<'fields>(
    fields: &'fields HashMap<String, ObjectTypeField>,
    columns: &HashMap<&str, usize>,
) -> Vec<(usize, &'fields str)> {
    let mut mapped: Vec<(usize, &str)> = fields
        .iter()
        .filter_map(|(name, field)| {
            let position = field_position(name, field, columns)?;
            Some((position, field.target.as_deref()?))
        })
        .collect();
    // Sorted, so two records carrying the same data write the same profile however the
    // field map was iterated.
    mapped.sort_unstable();
    mapped
}

/// One unique-key column: where it reads from, and the predefined search key a profile
/// carrying its value is indexed under, when the member it maps has one.
struct KeyColumn {
    /// The position in a record the key's value sits at.
    position: usize,
    /// The predefined `SearchProfiles` key the value is indexed under, so a record can be
    /// matched to a profile a previous call stored.
    search_key: Option<&'static str>,
}

/// The columns the job's `UniqueKey` names, in the order it names them.
///
/// `UniqueKey` is "the unique key columns for de-duping the profiles used to map data to
/// the profile": one text member, so several columns arrive in it as a list. A name that
/// is neither a column of the file nor a field of the map yields nothing, which leaves the
/// job with no key to de-dupe by.
fn key_columns(
    unique_key: &str,
    fields: &HashMap<String, ObjectTypeField>,
    columns: &HashMap<&str, usize>,
) -> Vec<KeyColumn> {
    unique_key
        .split(UNIQUE_KEY_SEPARATOR)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| {
            let field = fields.get(name);
            let position = match field {
                Some(field) => field_position(name, field, columns),
                None => columns.get(name).copied(),
            };
            let search_key = field
                .and_then(|field| field.target.as_deref())
                .and_then(standard_search_key_for_target);
            (position, search_key)
        })
        .map(|(position, search_key)| {
            position.map(|position| KeyColumn {
                position,
                search_key,
            })
        })
        .collect::<Option<Vec<KeyColumn>>>()
        .unwrap_or_default()
}

/// The position in a record one field reads from: its own entry's column when the file
/// carries one of that name, else the column the last segment of its `Source` names.
fn field_position(
    name: &str,
    field: &ObjectTypeField,
    columns: &HashMap<&str, usize>,
) -> Option<usize> {
    if let Some(position) = columns.get(name) {
        return Some(*position);
    }
    let source = field.source.as_deref()?;
    let column = source.rsplit('.').next()?;
    columns.get(column).copied()
}
