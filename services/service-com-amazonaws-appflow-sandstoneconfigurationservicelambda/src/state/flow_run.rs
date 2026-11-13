// SPDX-License-Identifier: CC0-1.0

//! What a flow RUN does: the transfer `StartFlow` performs between the locations a flow's
//! definition names.
//!
//! A flow run is a data transfer, so a run this emulator records has to BE one: an
//! Amazon S3 source and an Amazon S3 destination both live in this emulator's own object
//! store (the Amazon S3 service's state, the single store every service writes an object
//! to), so such a run reads the records at the source location, applies the field mapping
//! the flow's `tasks` declare, writes the result at EVERY destination location the
//! definition names and reports the bytes and records it actually moved. Nothing here
//! reports a transfer it did not perform: a run with an endpoint that is a connector
//! application this emulator does not reach performs nothing and says so, and a run the
//! object store refuses ends in the service's own modelled failure state carrying the
//! reason.
//!
//! The work happens when the run is STARTED, which is when AppFlow does it. Only the
//! TRANSITION to the run's terminal status is owed by the clock (see
//! [`super::ExecutionOutcome`]), so a later read materialises the outcome of work already
//! done rather than performing any.
//!
//! # What one run may consume
//!
//! Every part of a run is sized by what a caller put in the source bucket, so each is
//! bounded here and the bound is applied BEFORE the work it caps:
//!
//! * the object store's guard is held only to read the source bodies and the bucket
//!   policies, and again to write the output — never across the parse, the mapping or the
//!   encode, so one run cannot serialise every other object-store request behind it;
//! * that read is capped by [`MAX_SOURCE_OBJECTS_PER_RUN`] and
//!   [`MAX_SOURCE_BYTES_PER_RUN`], checked as the bodies are collected;
//! * the records are capped by [`MAX_RECORDS_PER_RUN`], passed INTO the parse so no more
//!   than that many are ever materialised, and the encode — which walks every field of
//!   every record — is capped by [`MAX_FIELDS_PER_RUN`] and [`MAX_CELLS_PER_RUN`] over
//!   their product, before it begins;
//! * the destinations one run writes are capped by [`MAX_DESTINATIONS_PER_RUN`] and the
//!   bytes they come to by [`MAX_OUTPUT_BYTES_PER_RUN`], so the per-destination work and
//!   its output are bounded as well as the per-file factors;
//! * the parse, the mapping and the encode run on the blocking pool and carry their own
//!   deadline ([`RUN_WORK_BUDGET`], asked between every unit of work), so they occupy no
//!   async worker and stop themselves when it is spent.
//!
//! A run that meets any of those ceilings moves nothing and ends in the modelled failure
//! state naming the ceiling, rather than reporting a transfer of the part that fitted.
//!
//! # Lock order
//!
//! Everything here takes the OBJECT STORE's guard, and its callers hold this service's own
//! guard or nothing: the order is therefore always AppFlow then Amazon S3, never the
//! reverse. A later cross-service call that needs both must keep that order.
//! [`perform_run`] holds neither across its caller-sized work.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use super::{nothing_transferred, ExecutionOutcome, ExecutionStatus, Flow};
use crate::ops::types::{ErrorInfo, ExecutionResult, S3DestinationProperties, S3SourceProperties, Task};

/// The service principal Amazon AppFlow reads a source bucket and writes a destination
/// bucket AS, and so the principal a bucket policy has to grant those actions to. AppFlow
/// acts on the caller's buckets through its own service principal, not as the caller.
pub const APPFLOW_SERVICE_PRINCIPAL: &str = "appflow.amazonaws.com";

/// How AppFlow names the request it refused, in the envelope a definition-time refusal's
/// message opens with (see [`request_failed`]): the operation spelled as the service
/// spells it, so every operation that stores a definition names ITSELF rather than the
/// one whose validation they share.
pub const CREATE_FLOW_REQUEST: &str = "Create Flow";
pub const UPDATE_FLOW_REQUEST: &str = "Update Flow";

/// The envelope a definition-time refusal is stated in: the request AppFlow could not
/// serve, then the reason it could not, in brackets.
///
/// AppFlow validates a whole definition and reports what it found wrapped in the request
/// it was validating, so a client reads which call failed and why from the one sentence.
fn request_failed(request: &str, reason: &str) -> String {
    format!("{request} request failed: [{reason}]")
}

/// The IAM actions a run performs on a SOURCE location: it lists the bucket to find the
/// objects under the flow's prefix and reads each one. Both are part of the source bucket
/// policy AppFlow publishes for an Amazon S3 source.
const SOURCE_ACTIONS: [(&str, Addressed); 2] = [
    ("s3:ListBucket", Addressed::Bucket),
    ("s3:GetObject", Addressed::Object),
];

/// The IAM action a run performs on a DESTINATION location: it writes the transferred
/// records as an object under the flow's prefix.
const DESTINATION_ACTIONS: [(&str, Addressed); 1] = [("s3:PutObject", Addressed::Object)];

/// What one action of a run is taken ON: the bucket itself (a listing) or an object in it
/// (a read or a write). The two are different resources in a policy — `arn:aws:s3:::b`
/// and `arn:aws:s3:::b/<key>` — so a grant for one is not a grant for the other.
#[derive(Debug, Clone, Copy)]
enum Addressed {
    Bucket,
    Object,
}

/// The `fileType` a destination writes when its output format names none, and the
/// `s3InputFileType` a source is read as when its input format names none
/// (`smithy_query(query="shape", shape="FileType,S3InputFileType")` — the two enumerations
/// share `CSV` and `JSON`, and JSON is the format a record set needs no further schema to
/// be read as).
const DEFAULT_FILE_TYPE: &str = "JSON";

/// The file types the two enumerations name
/// (`smithy_query(query="shape", shape="FileType,S3InputFileType")`). `JSON` is
/// [`DEFAULT_FILE_TYPE`], the type a location that declares none is read and written as.
const FILE_TYPE_CSV: &str = "CSV";
const FILE_TYPE_PARQUET: &str = "PARQUET";

/// The task type that PROJECTS the source fields a run reads, and the one that MAPS a
/// read field onto the name it is written under, and the one that maps EVERY field
/// (`smithy_query(query="shape", shape="TaskType")`).
const TASK_TYPE_FILTER: &str = "Filter";
const TASK_TYPE_MAP: &str = "Map";
const TASK_TYPE_MAP_ALL: &str = "Map_all";

/// The task property of a `Map_all` task that names the fields it does NOT map, as one
/// comma-separated list (`smithy_query(query="shape", shape="OperatorPropertiesKeys")` —
/// `EXCLUDE_SOURCE_FIELDS_LIST`).
const EXCLUDE_SOURCE_FIELDS_PROPERTY: &str = "EXCLUDE_SOURCE_FIELDS_LIST";

/// The most source objects one run reads, the most source bytes it reads, the most
/// records it moves, the most FIELDS those records carry between them, the most cells
/// (fields x records) an encode walks, the most destinations one run writes and the most
/// output bytes it produces in all.
///
/// CHOSEN, not documented: AppFlow publishes no per-run ceiling of any of these, so they
/// are this emulator's own. Each is applied BEFORE the work it caps, and together they
/// bound the whole of one run rather than one factor of it: the byte ceiling bounds the
/// bodies read, the record ceiling is passed into the parse so the expansion of those
/// bytes into records is bounded, the field and cell ceilings bound the encode — which
/// walks every field of every record, so its cost is their PRODUCT and not either alone —
/// and the destination and output ceilings bound the per-destination work and the bytes it
/// produces, which are multiplied by how many destinations the definition names.
///
/// The destination ceiling is far above anything the API itself serves: "you can only
/// transfer data to one item in this list at a time. Amazon AppFlow does not currently
/// support flows to multiple destinations at once"
/// (`smithy_query(query="doc", shape="CreateFlow")`), so a definition naming more than a
/// handful is beyond the service; one past this ceiling is refused when it is STORED (see
/// [`beyond_destination_ceiling`]) as well as when it is run.
const MAX_SOURCE_OBJECTS_PER_RUN: usize = 1_000;
const MAX_SOURCE_BYTES_PER_RUN: usize = 8 * 1024 * 1024;
const MAX_RECORDS_PER_RUN: usize = 100_000;
const MAX_FIELDS_PER_RUN: usize = 1_000;
const MAX_CELLS_PER_RUN: usize = 10_000_000;
const MAX_OUTPUT_BYTES_PER_RUN: usize = 32 * 1024 * 1024;
pub const MAX_DESTINATIONS_PER_RUN: usize = 16;

/// How long the caller-sized part of one run may take: the parse, the field mapping and
/// the encode, which run together on the blocking pool.
///
/// CHOSEN, not documented: AppFlow publishes no per-run duration. It is the bound on
/// DURATION that the ceilings above do not give, and it is enforced INSIDE that work (see
/// [`Budget`]) — between source files, between records and between the rows of an encode —
/// so the work stops ITSELF when the budget is spent. The `tokio::time::timeout` around it
/// is only a backstop for the caller's wait: a blocking task that has already started
/// cannot be cancelled from outside, so the deadline the closure checks is what bounds the
/// CPU and the memory one request consumes.
pub const RUN_WORK_BUDGET: Duration = Duration::from_secs(5);

/// Why a run could not be performed, as the sentence its `errorInfo` carries.
///
/// Each one names the ROLE of the location it is about (source or destination) and no
/// caller-supplied text: `ExecutionMessage`'s own pattern admits only
/// `^[\s\w/!@#+=.-]*$` (`smithy_query(query="shape", shape="ExecutionMessage")`), which a
/// bucket name (`^\S+$`) or an object key need not satisfy, so interpolating one could
/// put a value on the wire that the modelled member cannot carry.
const NO_SOURCE_BUCKET: &str = "The Amazon S3 bucket the source location names does not exist.";
const NO_DESTINATION_BUCKET: &str =
    "The Amazon S3 bucket the destination location names does not exist.";
const SOURCE_ACCESS_DENIED: &str = "Access denied reading the source location. Grant the AppFlow service principal appflow.amazonaws.com access to that bucket with a bucket policy.";
const DESTINATION_ACCESS_DENIED: &str = "Access denied writing the destination location. Grant the AppFlow service principal appflow.amazonaws.com access to that bucket with a bucket policy.";
const UNREADABLE_SOURCE: &str =
    "A file at the source location could not be read as the input file type the flow declares.";
const DESTINATION_REFUSED_WRITE: &str =
    "The Amazon S3 destination did not accept the file this flow run wrote.";
const OBJECT_STORE_UNREACHABLE: &str =
    "The Amazon S3 object store is not reachable from this emulator so the flow run was not performed.";
const RECORDS_NOT_ENCODED: &str =
    "The records this flow run read could not be encoded as the output file type the flow declares.";
const TOO_MANY_SOURCE_FILES: &str =
    "The source location holds more files than one run of this emulator transfers.";
const TOO_MUCH_SOURCE_DATA: &str =
    "The source location holds more data than one run of this emulator transfers.";
const TOO_MANY_RECORDS: &str =
    "The source location holds more records than one run of this emulator transfers.";
const TOO_MANY_FIELDS: &str =
    "The records at the source location carry more fields than one run of this emulator transfers.";
const TOO_MANY_CELLS: &str =
    "The records at the source location hold more values together than one run of this emulator transfers.";
const TOO_MANY_DESTINATIONS: &str =
    "This flow names more destinations than one run of this emulator writes.";
const TOO_MUCH_OUTPUT_DATA: &str =
    "This flow run would write more data than one run of this emulator writes.";
const RUN_TOOK_TOO_LONG: &str =
    "This flow run took longer than one run of this emulator is given and was abandoned.";

/// What the caller-sized part of one run may spend: the instant it has to be finished by,
/// and the output bytes it may produce in all.
///
/// Checked INSIDE that work rather than around it, because the one thing an outer
/// `tokio::time::timeout` cannot do is stop a blocking task that has already started: it
/// abandons the result while the closure keeps running. So the closure carries its own
/// deadline and stops itself, which is what makes [`RUN_WORK_BUDGET`] a bound on the CPU
/// and the memory a request consumes rather than only on the caller's wait. The output
/// allowance is spent the same way — as each row or column is produced, not once a whole
/// file has been built — so an encode that would amplify its source past
/// [`MAX_OUTPUT_BYTES_PER_RUN`] is abandoned while it is building rather than after the
/// bytes are already held.
#[derive(Debug)]
struct Budget {
    deadline: ::std::time::Instant,
    output_bytes_left: usize,
}

impl Budget {
    fn new() -> Self {
        Self {
            deadline: ::std::time::Instant::now() + RUN_WORK_BUDGET,
            output_bytes_left: MAX_OUTPUT_BYTES_PER_RUN,
        }
    }

    /// `Err` once the deadline has passed — asked between units of work, so the step that
    /// would follow is never begun.
    fn still_running(&self) -> Result<(), &'static str> {
        if ::std::time::Instant::now() > self.deadline {
            return Err(RUN_TOOK_TOO_LONG);
        }
        Ok(())
    }

    /// Spend `bytes` of this run's output allowance, or `Err` when the run would write
    /// more than one run writes in all.
    fn spend_output(&mut self, bytes: usize) -> Result<(), &'static str> {
        self.output_bytes_left = self
            .output_bytes_left
            .checked_sub(bytes)
            .ok_or(TOO_MUCH_OUTPUT_DATA)?;
        Ok(())
    }
}

/// Why a definition names more destinations than one run of this emulator writes, as the
/// sentence the refusal carries, or `None` when it names at most
/// [`MAX_DESTINATIONS_PER_RUN`].
///
/// A definition AppFlow stores has to be one it could run, and the work of a run is
/// multiplied by the destinations it writes — so a definition past that ceiling is refused
/// where it is stored rather than accepted and left to fail every run. Shared by the two
/// operations that store a definition (`CreateFlow`, `UpdateFlow`).
pub fn beyond_destination_ceiling(
    destination_flow_config_list: &[crate::ops::types::DestinationFlowConfig],
) -> Option<String> {
    let named = destination_flow_config_list.len();
    (named > MAX_DESTINATIONS_PER_RUN).then(|| {
        format!(
            "A flow transfers to at most {MAX_DESTINATIONS_PER_RUN} destinations and this \
             definition names {named}."
        )
    })
}

/// What a RUN needs from the flow it runs: the flow's identity, and the part of its
/// definition that decides what moves where.
///
/// Taken from the stored flow under the store guard ([`Flow::run_plan`]) and then used
/// WITHOUT it, which is what lets the transfer run on no lock at all.
#[derive(Debug, Clone)]
pub struct RunPlan {
    pub flow_name: String,
    pub flow_arn: String,
    source: S3SourceProperties,
    destinations: Vec<S3DestinationProperties>,
    tasks: Vec<Task>,
    /// Whether a run of this flow PULLS a window of new or updated records from its
    /// source — see [`Flow::pulls_incrementally`].
    incremental_pull: bool,
}

/// The `dataPullMode` of a scheduled flow whose every run transfers only what changed
/// since the last one (`smithy_query(query="shape", shape="DataPullMode")`).
pub const DATA_PULL_MODE_INCREMENTAL: &str = "Incremental";

impl Flow {
    /// What a run of this flow would move, or `None` when an endpoint of it is NOT an
    /// Amazon S3 location.
    ///
    /// `None` is the case this emulator performs no transfer for: every other connector is
    /// an application endpoint reached over an outbound connection this emulator does not
    /// make, so such a run is reported as not performed rather than as a transfer nothing
    /// backs. A flow with SEVERAL destinations is planned in full — each one is written —
    /// and one whose destinations are not all Amazon S3 locations is not planned at all,
    /// so no run of it reports a partial transfer as a success.
    pub fn run_plan(&self) -> Option<RunPlan> {
        let source = self
            .source_flow_config
            .source_connector_properties
            .s3
            .clone()?;
        if self.destination_flow_config_list.is_empty() {
            return None;
        }
        let mut destinations = Vec::with_capacity(self.destination_flow_config_list.len());
        for destination in &self.destination_flow_config_list {
            destinations.push(destination.destination_connector_properties.s3.clone()?);
        }
        Some(RunPlan {
            flow_name: self.flow_name.clone(),
            flow_arn: self.flow_arn.clone(),
            source,
            destinations,
            tasks: self.tasks.clone(),
            incremental_pull: self.pulls_incrementally(),
        })
    }

    /// `true` when a run of this flow QUERIES its source for the records that are new or
    /// updated, rather than transferring everything the source location holds.
    ///
    /// It is what decides whether a run has a data-pull window to report at all:
    /// `dataPullStartTime` and `dataPullEndTime` are "[t]he timestamp that determines the
    /// first new or updated record to be transferred in the flow run" and the one "that
    /// indicates the last" (`smithy_query(query="shape", shape="ExecutionRecord")`) — the
    /// bounds of that query — so a run that made no such query has neither.
    ///
    /// A flow declares the query in its definition, in either of the two places the model
    /// puts it: `incrementalPullConfig` "[d]efines the configuration for a scheduled
    /// incremental data pull" and names the datetime field it is made over
    /// (`smithy_query(query="doc", shape="SourceFlowConfig",
    /// member="incrementalPullConfig")`), and a scheduled trigger's `dataPullMode`
    /// "[s]pecifies whether a scheduled flow has an incremental data transfer or a
    /// complete data transfer for each flow run" (`smithy_query(query="doc",
    /// shape="ScheduledTriggerProperties", member="dataPullMode")`).
    pub fn pulls_incrementally(&self) -> bool {
        let names_datetime_field = self
            .source_flow_config
            .incremental_pull_config
            .as_ref()
            .and_then(|config| config.datetime_type_field_name.as_deref())
            .is_some_and(|field| !field.is_empty());
        let incremental_schedule = self
            .trigger_config
            .trigger_properties
            .as_ref()
            .and_then(|properties| properties.scheduled.as_ref())
            .and_then(|scheduled| scheduled.data_pull_mode.as_deref())
            == Some(DATA_PULL_MODE_INCREMENTAL);
        names_datetime_field || incremental_schedule
    }
}

/// One file a run writes: where it goes and in what file type.
#[derive(Debug, Clone)]
struct Written {
    bucket: String,
    key: String,
    file_type: String,
}

/// What the READ phase of a run took out of the object store: the source bodies, the
/// volume they came to, and the files the run has to write.
#[derive(Debug)]
struct ReadSource {
    bodies: Vec<Vec<u8>>,
    bytes_processed: i64,
    written: Vec<Written>,
}

/// The output of the caller-sized phase: the bytes of each destination file, and how many
/// records they hold.
#[derive(Debug)]
struct Prepared {
    files: Vec<Vec<u8>>,
    records: i64,
}

/// Perform the run `execution_id` of the flow `plan` describes, and report its outcome.
///
/// The outcome is the status the run ENDS in and the volume it moved, decided here and
/// now: the transfer itself happens in this call, as it does in AppFlow, and the clock
/// owes the run only the transition to that status (see [`super::ExecutionOutcome`]).
///
/// Three phases, so no lock is held across the caller-sized work: the object store is read
/// under its guard, the parse and encode run on the blocking pool under
/// [`RUN_WORK_BUDGET`], and the output is written under the guard again.
pub async fn perform_run<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    plan: &RunPlan,
    execution_id: &str,
    started_at: SystemTime,
) -> ExecutionOutcome {
    let read = match read_source(engine, plan, execution_id) {
        Ok(read) => read,
        Err(reason) => return failed(reason),
    };

    // The parse, the field mapping and the encode are sized by what the caller put in the
    // source bucket, so they run OFF the async worker and under a wall-clock budget: a run
    // that exceeds it is abandoned rather than occupying the process.
    let input_file_type = input_file_type(&plan.source).to_string();
    let output_file_types: Vec<String> = read
        .written
        .iter()
        .map(|written| written.file_type.clone())
        .collect();
    let tasks = plan.tasks.clone();
    let bodies = read.bodies;
    let prepared = ::tokio::time::timeout(
        RUN_WORK_BUDGET,
        ::tokio::task::spawn_blocking(move || {
            prepare(&bodies, &input_file_type, &tasks, &output_file_types)
        }),
    )
    .await;
    let prepared = match prepared {
        Ok(Ok(Ok(prepared))) => prepared,
        Ok(Ok(Err(reason))) => return failed(reason),
        // The blocking task could not be joined at all, which is this service's own
        // failure rather than anything the request asked for.
        Ok(Err(_)) => return failed(RECORDS_NOT_ENCODED),
        Err(_) => return failed(RUN_TOOK_TOO_LONG),
    };
    let data_pull_end_time = SystemTime::now();

    match write_output(engine, &read.written, prepared.files) {
        Ok(bytes_written) => ExecutionOutcome {
            status: ExecutionStatus::Successful,
            result: ExecutionResult {
                // Nothing went wrong, so there is no error information to report.
                error_info: None,
                bytes_processed: Some(read.bytes_processed),
                bytes_written: Some(bytes_written),
                records_processed: Some(prepared.records),
                // One process read the source and wrote the destinations, and no page size
                // bounded it: `maxPageSize` is "the maximum number of records that Amazon
                // AppFlow receives in each page of the response from your SAP application"
                // (`smithy_query(query="shape", shape="ExecutionResult")`), which an
                // Amazon S3 source has none of — reported as the zero it is, so a client
                // reading the counter can tell "none" from "not reported".
                num_parallel_processes: Some(1),
                max_page_size: Some(0),
            },
            // The window of new or updated records this run pulled, for a run that
            // queried its source for one at all (see [`Flow::pulls_incrementally`]): from
            // when it began reading the source to when it had finished. A run that
            // transfers everything the source location holds made no such query, so it
            // reports neither bound rather than describing a data pull as having happened.
            data_pull_start_time: plan.incremental_pull.then_some(started_at),
            data_pull_end_time: plan.incremental_pull.then_some(data_pull_end_time),
        },
        Err(reason) => failed(reason),
    }
}

/// The READ phase: everything this run needs out of the object store, taken under its
/// guard and nothing else done while it is held.
///
/// It also resolves every location the run will write and checks the grant on it BEFORE
/// anything is read into records, so a run that could not write anywhere reports that
/// instead of the work it would have done. `Err` is the sentence the run reports.
fn read_source<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    plan: &RunPlan,
    execution_id: &str,
) -> Result<ReadSource, &'static str> {
    let Some(object_store) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)
    else {
        return Err(OBJECT_STORE_UNREACHABLE);
    };
    // The work of a run is multiplied by the destinations it writes, so the count is
    // capped before anything is read — a definition past the ceiling is refused where it
    // is stored, and this is what holds for one that was stored before that refusal
    // existed.
    if plan.destinations.len() > MAX_DESTINATIONS_PER_RUN {
        return Err(TOO_MANY_DESTINATIONS);
    }

    // Held for this block only: the bodies and the policy documents are copied out, and
    // every judgement over them is made after the guard is dropped.
    let (bodies, bytes_processed, source_policy, written) = {
        let store = object_store.lock();
        let Some(source_bucket) = store.bucket(&plan.source.bucket_name) else {
            return Err(NO_SOURCE_BUCKET);
        };
        let source_prefix = plan.source.bucket_prefix.clone().unwrap_or_default();
        let source_policy = source_bucket.policy.clone();
        let mut keyed: Vec<(&str, &Vec<u8>)> = source_bucket
            .objects()
            .filter(|(key, _)| key.starts_with(&source_prefix))
            .map(|(key, object)| (key, &object.body))
            .collect();
        if keyed.len() > MAX_SOURCE_OBJECTS_PER_RUN {
            return Err(TOO_MANY_SOURCE_FILES);
        }
        keyed.sort_by(|left, right| left.0.cmp(right.0));
        // The byte ceiling is applied BEFORE the bodies are copied, so the copy itself is
        // bounded rather than checked afterwards.
        let mut bytes_processed = 0usize;
        for (_, body) in &keyed {
            bytes_processed += body.len();
            if bytes_processed > MAX_SOURCE_BYTES_PER_RUN {
                return Err(TOO_MUCH_SOURCE_DATA);
            }
        }
        let bodies: Vec<Vec<u8>> = keyed.into_iter().map(|(_, body)| body.clone()).collect();

        let mut written = Vec::with_capacity(plan.destinations.len());
        for destination in &plan.destinations {
            let Some(bucket) = store.bucket(&destination.bucket_name) else {
                return Err(NO_DESTINATION_BUCKET);
            };
            written.push((
                Written {
                    bucket: destination.bucket_name.clone(),
                    key: destination_key(
                        destination.bucket_prefix.as_deref(),
                        &plan.flow_name,
                        execution_id,
                        output_file_type(destination),
                    ),
                    file_type: output_file_type(destination).to_string(),
                },
                bucket.policy.clone(),
            ));
        }
        (bodies, bytes_processed as i64, source_policy, written)
    };

    if !admits(
        source_policy.as_deref(),
        &SOURCE_ACTIONS,
        &plan.source.bucket_name,
        plan.source.bucket_prefix.as_deref().unwrap_or_default(),
        &plan.flow_arn,
    ) {
        return Err(SOURCE_ACCESS_DENIED);
    }
    let mut destinations = Vec::with_capacity(written.len());
    for (destination, policy) in written {
        if !admits(
            policy.as_deref(),
            &DESTINATION_ACTIONS,
            &destination.bucket,
            &destination.key,
            &plan.flow_arn,
        ) {
            return Err(DESTINATION_ACCESS_DENIED);
        }
        destinations.push(destination);
    }

    Ok(ReadSource {
        bodies,
        bytes_processed,
        written: destinations,
    })
}

/// The caller-sized phase: read the records out of the source bodies, map each one, and
/// encode the result once per destination file type.
///
/// Runs on the blocking pool with no lock held, and bounds ITSELF: at most
/// [`MAX_RECORDS_PER_RUN`] records are allocated (the remaining budget is passed INTO the
/// parse, so a source holding more is reported rather than materialised), the encode is
/// refused before it begins when the records carry more fields than
/// [`MAX_FIELDS_PER_RUN`] or more cells than [`MAX_CELLS_PER_RUN`] between them, the bytes
/// it produces are spent from one allowance, and the deadline is asked between every unit
/// of work — so a run that exceeds any of them stops here rather than running on.
fn prepare(
    bodies: &[Vec<u8>],
    input_file_type: &str,
    tasks: &[Task],
    output_file_types: &[String],
) -> Result<Prepared, &'static str> {
    let mut budget = Budget::new();
    let mapping = FieldMapping::of(tasks);
    let mut records: Vec<BTreeMap<String, ::serde_json::Value>> = Vec::new();
    for body in bodies {
        budget.still_running()?;
        // One past the ceiling, so a source with more records than a run moves is
        // DETECTED without the rest of it ever being allocated.
        let remaining = MAX_RECORDS_PER_RUN - records.len() + 1;
        let parsed = parse_records(body, input_file_type, remaining).ok_or(UNREADABLE_SOURCE)?;
        for record in parsed {
            if records.len() >= MAX_RECORDS_PER_RUN {
                return Err(TOO_MANY_RECORDS);
            }
            records.push(mapping.applied_to(&record));
        }
    }

    // The encode walks every field of every record, so its cost is their PRODUCT: both
    // are capped here, before any encoder runs.
    let columns = columns_of(&records);
    if columns.len() > MAX_FIELDS_PER_RUN {
        return Err(TOO_MANY_FIELDS);
    }
    if columns.len().saturating_mul(records.len()) > MAX_CELLS_PER_RUN {
        return Err(TOO_MANY_CELLS);
    }
    let mut files = Vec::with_capacity(output_file_types.len());
    for file_type in output_file_types {
        budget.still_running()?;
        // The output allowance is spent INSIDE the encode, row by row, so a file that
        // would exceed it is abandoned while it is being built rather than after it has
        // been held whole in memory.
        files.push(encode_records(&records, &columns, file_type, &mut budget)?);
    }
    Ok(Prepared {
        files,
        records: records.len() as i64,
    })
}

/// The WRITE phase: one file per destination, under the object store's guard, answering
/// the bytes written in all. `Err` is the sentence the run reports.
fn write_output<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    written: &[Written],
    files: Vec<Vec<u8>>,
) -> Result<i64, &'static str> {
    let Some(object_store) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)
    else {
        return Err(OBJECT_STORE_UNREACHABLE);
    };
    let mut store = object_store.lock();
    let mut bytes_written = 0i64;
    for (destination, body) in written.iter().zip(files) {
        bytes_written += body.len() as i64;
        let object = stored_object(&store, &destination.bucket, body);
        if !matches!(
            store.put_object(&destination.bucket, &destination.key, object),
            Some(
                ::service_com_amazonaws_s3_amazons3::state::ObjectWrite::Created
                    | ::service_com_amazonaws_s3_amazons3::state::ObjectWrite::Replaced
            )
        ) {
            return Err(DESTINATION_REFUSED_WRITE);
        }
    }
    Ok(bytes_written)
}

/// One end of a transfer, as the definition names it: the bucket, the key the actions are
/// taken on, and the actions that end's grant has to cover.
type Location<'definition> = (
    &'definition str,
    String,
    &'static [(&'static str, Addressed)],
);

/// Why an Amazon S3 location a definition names is one AppFlow could not use, as the
/// sentence the refusal carries, or `None` when every location it names is usable.
///
/// A definition AppFlow STORES has to be one it could run: the bucket each Amazon S3
/// location names has to exist, and its policy has to grant the AppFlow service principal
/// the actions a run takes on that end of the transfer (see [`admits`]) — so a flow that
/// could never move data is refused when it is defined rather than created and left to
/// fail every run. A location naming any other connector is a connector application
/// endpoint, which no bucket policy governs.
///
/// Shared by every operation that stores a definition (`CreateFlow`, `UpdateFlow`), so one
/// cannot accept a flow the other would refuse; `request` is how the refusal names the
/// call it refused ([`CREATE_FLOW_REQUEST`], [`UPDATE_FLOW_REQUEST`]). The object store's
/// guard is held only to copy the policy documents; the decision over them is made after
/// it is dropped.
pub fn unusable_s3_location<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    request: &str,
    flow_name: &str,
    flow_arn: &str,
    source_flow_config: &crate::ops::types::SourceFlowConfig,
    destination_flow_config_list: &[crate::ops::types::DestinationFlowConfig],
) -> Option<String> {
    let object_store = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let mut locations: Vec<Location<'_>> = Vec::new();
    if let Some(source) = &source_flow_config.source_connector_properties.s3 {
        locations.push((
            &source.bucket_name,
            source.bucket_prefix.clone().unwrap_or_default(),
            &SOURCE_ACTIONS,
        ));
    }
    for destination in destination_flow_config_list {
        if let Some(target) = &destination.destination_connector_properties.s3 {
            locations.push((
                &target.bucket_name,
                // The path a run writes under, which is what the destination grant has to
                // cover; the run itself appends its own identifier below it.
                format!(
                    "{}{flow_name}",
                    target
                        .bucket_prefix
                        .as_deref()
                        .map(|prefix| format!("{}/", prefix.trim_end_matches('/')))
                        .unwrap_or_default()
                ),
                &DESTINATION_ACTIONS,
            ));
        }
    }

    let policies: Vec<Option<Option<String>>> = {
        let store = object_store.lock();
        locations
            .iter()
            .map(|(bucket, _, _)| store.bucket(bucket).map(|held| held.policy.clone()))
            .collect()
    };
    for ((bucket, key, actions), policy) in locations.into_iter().zip(policies) {
        let Some(policy) = policy else {
            return Some(format!(
                "Amazon S3 bucket {bucket} does not exist. Create the bucket and try again."
            ));
        };
        let missing = missing_actions(policy.as_deref(), actions, bucket, &key, flow_arn);
        if !missing.is_empty() {
            // What the caller has to repair, named as AppFlow names it: the bucket, the
            // actions its policy still does not grant, the principal they have to be
            // granted to, and the two resources the grant has to cover — the bucket itself
            // (a listing) and every object in it (a read or a write).
            return Some(request_failed(
                request,
                &format!(
                    "The provided S3 Bucket [{bucket}] is missing the following permitted \
                     actions: [{missing}]\n. These policies must be applied to the \
                     [{APPFLOW_SERVICE_PRINCIPAL}] service principal and must be attached to \
                     all the following resources: [{bucket_arn}, {objects_arn}]",
                    missing = missing.join(", "),
                    bucket_arn =
                        ::service_com_amazonaws_s3_amazons3::state::bucket_arn(bucket),
                    objects_arn =
                        ::service_com_amazonaws_s3_amazons3::state::object_arn(bucket, "*"),
                ),
            ));
        }
    }
    None
}

/// The outcome of a run that ended in the service's modelled failure state, carrying why.
///
/// Every counter is a true zero: a run that failed moved nothing, and reporting a volume
/// for it would be a fabricated result. `putFailuresCount` is zero for the same reason —
/// no record was ever put.
fn failed(message: &str) -> ExecutionOutcome {
    ExecutionOutcome {
        status: ExecutionStatus::Error,
        result: nothing_transferred(Some(ErrorInfo {
            put_failures_count: Some(0),
            execution_message: Some(message.to_string()),
        })),
        // No data-pull window: nothing was pulled.
        data_pull_start_time: None,
        data_pull_end_time: None,
    }
}

/// Whether the bucket policy `policy` grants the AppFlow service principal every action a
/// run takes on that location — that is, whether [`missing_actions`] finds none missing.
fn admits(
    policy: Option<&str>,
    actions: &[(&str, Addressed)],
    bucket: &str,
    key: &str,
    flow_arn: &str,
) -> bool {
    missing_actions(policy, actions, bucket, key, flow_arn).is_empty()
}

/// Which of `actions` the bucket policy `policy` does NOT grant the AppFlow service
/// principal on that location, in the order a run takes them, each as IAM spells it in a
/// policy but lowercased — the form AppFlow enumerates a missing grant in.
///
/// Each action is decided on its own, by the tree's one evaluation of a resource-based
/// policy against a request (`service_common::iam_authorization`) over the resource that
/// action is taken on, so AppFlow reads a bucket policy exactly as the object store itself
/// does: an explicit `Deny` wins, and a policy that says nothing about the action grants
/// nothing — which is what makes every action missing at a bucket with NO policy at all,
/// and only the rest missing at one whose policy covers part of what a run does.
///
/// The request carries the two condition keys AppFlow answers when it acts on a caller's
/// bucket — the account the flow belongs to and the flow's own ARN — so a policy that
/// confines its grant to this account's flows still grants it.
fn missing_actions(
    policy: Option<&str>,
    actions: &[(&str, Addressed)],
    bucket: &str,
    key: &str,
    flow_arn: &str,
) -> Vec<String> {
    let Some(document) = policy else {
        return actions
            .iter()
            .map(|(action, _)| action.to_lowercase())
            .collect();
    };
    let principals = [APPFLOW_SERVICE_PRINCIPAL.to_string()];
    let context = [
        (
            "aws:SourceAccount".to_string(),
            ::engine::EMULATOR_ACCOUNT_ID.to_string(),
        ),
        ("aws:SourceArn".to_string(), flow_arn.to_string()),
    ];
    actions
        .iter()
        .filter(|(action, addressed)| {
            let resource = match addressed {
                Addressed::Bucket => {
                    ::service_com_amazonaws_s3_amazons3::state::bucket_arn(bucket)
                }
                Addressed::Object => {
                    ::service_com_amazonaws_s3_amazons3::state::object_arn(bucket, key)
                }
            };
            let request =
                ::service_common::iam_authorization::Request::new(action, &resource, &principals)
                    .with_context(&context);
            ::service_common::iam_authorization::resource_decision(document, &request)
                != ::service_common::iam_authorization::Decision::Allowed
        })
        .map(|(action, _)| action.to_lowercase())
        .collect()
}

/// The object a run writes at a destination location.
///
/// Everything a stored object carries that a WRITE does not choose is left as a
/// `PutObject` of the same bytes would leave it: the entity tag is computed from the bytes
/// by the object store's own function, and the encryption is the destination bucket's
/// default, because that is what any other write to that bucket is stored under.
fn stored_object(
    store: &::service_com_amazonaws_s3_amazons3::state::ComAmazonawsS3Amazons3State,
    bucket: &str,
    body: Vec<u8>,
) -> ::service_com_amazonaws_s3_amazons3::state::Object {
    ::service_com_amazonaws_s3_amazons3::state::Object {
        etag: ::service_com_amazonaws_s3_amazons3::state::object_etag(&body),
        body,
        version_id: None,
        last_modified: SystemTime::now(),
        storage_class: ::service_com_amazonaws_s3_amazons3::state::DEFAULT_STORAGE_CLASS
            .to_string(),
        content_type: None,
        cache_control: None,
        content_disposition: None,
        content_encoding: None,
        content_language: None,
        expires: None,
        website_redirect_location: None,
        metadata: ::std::collections::BTreeMap::new(),
        tags: Vec::new(),
        encryption: store
            .bucket(bucket)
            .map(::service_com_amazonaws_s3_amazons3::state::Bucket::default_object_encryption)
            .unwrap_or_default(),
        // A transferred object carries no Object Lock retention, legal hold, restore,
        // annotation, ACL or checksum of its own, and is written whole: each of those is
        // placed on a stored object afterwards by Amazon S3's own operation for it.
        retention: None,
        legal_hold: None,
        restore: None,
        annotations: ::std::collections::BTreeMap::new(),
        acl: None,
        checksum: None,
        parts: Vec::new(),
        replication: None,
    }
}

/// The key a run writes its file at: the destination prefix, then the flow and the run
/// that produced the file, then the file itself.
///
/// The run's own identifier is in the path, so two runs of one flow never overwrite each
/// other's output and a reader can tell which run wrote what; the flow name is above it,
/// so two flows writing under one prefix stay apart. A destination naming no prefix writes
/// at the top of the bucket.
fn destination_key(
    prefix: Option<&str>,
    flow_name: &str,
    execution_id: &str,
    file_type: &str,
) -> String {
    let file = format!("{execution_id}.{}", file_type.to_ascii_lowercase());
    let within_flow = format!("{flow_name}/{execution_id}/{file}");
    match prefix
        .map(|prefix| prefix.trim_end_matches('/'))
        .filter(|prefix| !prefix.is_empty())
    {
        Some(prefix) => format!("{prefix}/{within_flow}"),
        None => within_flow,
    }
}

/// The file type a source location's files are read as: the one its input format declares,
/// or [`DEFAULT_FILE_TYPE`] when it declares none.
fn input_file_type(source: &S3SourceProperties) -> &str {
    source
        .s3_input_format_config
        .as_ref()
        .and_then(|config| config.s3_input_file_type.as_deref())
        .unwrap_or(DEFAULT_FILE_TYPE)
}

/// The file type a destination location's file is written as: the one its output format
/// declares, or [`DEFAULT_FILE_TYPE`] when it declares none.
fn output_file_type(destination: &S3DestinationProperties) -> &str {
    destination
        .s3_output_format_config
        .as_ref()
        .and_then(|config| config.file_type.as_deref())
        .unwrap_or(DEFAULT_FILE_TYPE)
}

/// At most `budget` records out of one source file, or `None` when it is not that file
/// type at all.
///
/// A `JSON` file is read as the records it carries in any of the three shapes a record set
/// is written in — one object, an array of them, or one object per line — because all
/// three are JSON and a flow declares only the type, not the shape. A `CSV` file is read
/// through a maintained CSV reader, whose first row names the fields.
fn parse_records(
    body: &[u8],
    file_type: &str,
    budget: usize,
) -> Option<Vec<BTreeMap<String, ::serde_json::Value>>> {
    match file_type {
        FILE_TYPE_CSV => parse_csv(body, budget),
        // Every other declared input type is JSON: `S3InputFileType` names only the two
        // (`smithy_query(query="shape", shape="S3InputFileType")`).
        _ => parse_json(body, budget),
    }
}

/// At most `budget` records out of a JSON source file.
fn parse_json(
    body: &[u8],
    budget: usize,
) -> Option<Vec<BTreeMap<String, ::serde_json::Value>>> {
    if let Ok(value) = ::serde_json::from_slice::<::serde_json::Value>(body) {
        return match value {
            ::serde_json::Value::Array(entries) => entries
                .into_iter()
                .take(budget)
                .map(as_record)
                .collect::<Option<Vec<_>>>(),
            ::serde_json::Value::Object(_) => as_record(value).map(|record| vec![record]),
            _ => None,
        };
    }
    // One record per line: the shape a record set is written in when it is appended to
    // rather than built as one document.
    String::from_utf8(body.to_vec())
        .ok()?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(budget)
        .map(|line| {
            ::serde_json::from_str::<::serde_json::Value>(line)
                .ok()
                .and_then(as_record)
        })
        .collect()
}

/// One JSON value as a record: a record is an object, so anything else is not one.
fn as_record(value: ::serde_json::Value) -> Option<BTreeMap<String, ::serde_json::Value>> {
    match value {
        ::serde_json::Value::Object(members) => Some(members.into_iter().collect()),
        _ => None,
    }
}

/// At most `budget` records out of a CSV source file, read by the `csv` crate: the first
/// row names the fields and every later row is a record of string values.
fn parse_csv(
    body: &[u8],
    budget: usize,
) -> Option<Vec<BTreeMap<String, ::serde_json::Value>>> {
    let mut reader = ::csv::Reader::from_reader(body);
    let headers: Vec<String> = reader
        .headers()
        .ok()?
        .iter()
        .map(::std::string::ToString::to_string)
        .collect();
    let mut records = Vec::new();
    for row in reader.records().take(budget) {
        let row = row.ok()?;
        records.push(
            headers
                .iter()
                .zip(row.iter())
                .map(|(field, value)| {
                    (
                        field.clone(),
                        ::serde_json::Value::String(value.to_string()),
                    )
                })
                .collect(),
        );
    }
    Some(records)
}

/// The bytes of the destination file holding `records`, in `file_type`, whose columns are
/// `columns` — spending `budget` as it goes, so an encode stops as soon as the run's
/// deadline has passed or its output allowance is gone instead of building the rest.
fn encode_records(
    records: &[BTreeMap<String, ::serde_json::Value>],
    columns: &[String],
    file_type: &str,
    budget: &mut Budget,
) -> Result<Vec<u8>, &'static str> {
    match file_type {
        FILE_TYPE_CSV => encode_csv(records, columns, budget),
        FILE_TYPE_PARQUET => encode_parquet(records, columns, budget),
        // Every other declared output type is JSON: `FileType` names only the three
        // (`smithy_query(query="shape", shape="FileType")`).
        _ => encode_json(records, budget),
    }
}

/// `records` as a JSON file: one record per line, which is the shape a record set is read
/// back from a file it was appended to.
///
/// Each line is charged to the run's output allowance BEFORE it is appended, so the file
/// this builds can never be larger than the allowance that was left.
fn encode_json(
    records: &[BTreeMap<String, ::serde_json::Value>],
    budget: &mut Budget,
) -> Result<Vec<u8>, &'static str> {
    let mut body = String::new();
    for record in records {
        budget.still_running()?;
        let object: ::serde_json::Map<String, ::serde_json::Value> = record
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let line = ::serde_json::Value::Object(object).to_string();
        budget.spend_output(line.len() + 1)?;
        body.push_str(&line);
        body.push('\n');
    }
    Ok(body.into_bytes())
}

/// `records` as a CSV file, written by the `csv` crate: one header row naming every field
/// any record carries, then one row per record with an empty cell for a field that record
/// does not carry.
///
/// Every row is charged to the run's output allowance as it lands — the bytes the writer
/// actually produced, read back off its own buffer — so the file this builds can never be
/// larger than the allowance that was left.
fn encode_csv(
    records: &[BTreeMap<String, ::serde_json::Value>],
    columns: &[String],
    budget: &mut Budget,
) -> Result<Vec<u8>, &'static str> {
    let mut writer = ::csv::Writer::from_writer(Vec::new());
    let mut charged = 0usize;
    writer
        .write_record(columns)
        .map_err(|_| RECORDS_NOT_ENCODED)?;
    for record in records {
        budget.still_running()?;
        let row: Vec<String> = columns
            .iter()
            .map(|column| record.get(column).map(cell).unwrap_or_default())
            .collect();
        writer.write_record(&row).map_err(|_| RECORDS_NOT_ENCODED)?;
        // Flushed so what the writer has produced so far IS what its buffer holds, and
        // the growth since the last row is what this row cost.
        writer.flush().map_err(|_| RECORDS_NOT_ENCODED)?;
        let produced = writer.get_ref().len();
        budget.spend_output(produced.saturating_sub(charged))?;
        charged = produced;
    }
    writer.into_inner().map_err(|_| RECORDS_NOT_ENCODED)
}

/// `records` as an Apache Parquet file, written by the Apache implementation of the
/// format: one OPTIONAL UTF-8 column per field any record carries, and one row per record
/// whose cell is absent where that record carries no value for the field.
///
/// Each column's values are charged to the run's output allowance before they are handed
/// to the writer, so the bytes this holds can never exceed the allowance that was left.
fn encode_parquet(
    records: &[BTreeMap<String, ::serde_json::Value>],
    columns: &[String],
    budget: &mut Budget,
) -> Result<Vec<u8>, &'static str> {
    use ::parquet::basic::{ConvertedType, Repetition, Type as PhysicalType};
    use ::parquet::data_type::{ByteArray, ByteArrayType};
    use ::parquet::file::properties::WriterProperties;
    use ::parquet::file::writer::SerializedFileWriter;
    use ::parquet::schema::types::Type as SchemaType;

    let mut fields = Vec::with_capacity(columns.len());
    for column in columns {
        fields.push(::std::sync::Arc::new(
            SchemaType::primitive_type_builder(column, PhysicalType::BYTE_ARRAY)
                // Optional: a record need not carry a value for every field the file has
                // a column for.
                .with_repetition(Repetition::OPTIONAL)
                .with_converted_type(ConvertedType::UTF8)
                .build()
                .map_err(|_| RECORDS_NOT_ENCODED)?,
        ));
    }
    let schema = SchemaType::group_type_builder("flow_record")
        .with_fields(fields)
        .build()
        .map_err(|_| RECORDS_NOT_ENCODED)?;
    let mut writer = SerializedFileWriter::new(
        Vec::new(),
        ::std::sync::Arc::new(schema),
        ::std::sync::Arc::new(WriterProperties::builder().build()),
    )
    .map_err(|_| RECORDS_NOT_ENCODED)?;
    if !records.is_empty() {
        let mut row_group = writer.next_row_group().map_err(|_| RECORDS_NOT_ENCODED)?;
        let mut column = 0usize;
        while let Some(mut column_writer) =
            row_group.next_column().map_err(|_| RECORDS_NOT_ENCODED)?
        {
            budget.still_running()?;
            // One column per field, in the order the schema declares them, so the writer
            // and the schema cannot disagree about which field is being written.
            let name = columns.get(column).ok_or(RECORDS_NOT_ENCODED)?;
            let mut values: Vec<ByteArray> = Vec::new();
            let mut levels: Vec<i16> = Vec::with_capacity(records.len());
            for record in records {
                match record.get(name) {
                    Some(value) => {
                        let text = cell(value);
                        budget.spend_output(text.len())?;
                        values.push(ByteArray::from(text.as_bytes()));
                        levels.push(1);
                    }
                    None => levels.push(0),
                }
            }
            column_writer
                .typed::<ByteArrayType>()
                .write_batch(&values, Some(&levels), None)
                .map_err(|_| RECORDS_NOT_ENCODED)?;
            column_writer.close().map_err(|_| RECORDS_NOT_ENCODED)?;
            column += 1;
        }
        row_group.close().map_err(|_| RECORDS_NOT_ENCODED)?;
    }
    writer
        .into_inner()
        .map_err(|_| RECORDS_NOT_ENCODED)
        .map(|buffer| buffer.to_vec())
}

/// Every field any record carries, in ascending field order — the columns of a tabular
/// destination file. Ordered, so the same records always produce the same file.
fn columns_of(records: &[BTreeMap<String, ::serde_json::Value>]) -> Vec<String> {
    let mut columns: BTreeSet<&str> = BTreeSet::new();
    for record in records {
        columns.extend(record.keys().map(String::as_str));
    }
    columns
        .into_iter()
        .map(::std::string::ToString::to_string)
        .collect()
}

/// One field's value as the text a tabular file's cell holds: a string value as itself,
/// anything else as the JSON it is.
fn cell(value: &::serde_json::Value) -> String {
    match value {
        ::serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The field mapping a flow's `tasks` declare: which source fields a run reads, and the
/// name each one is written under.
///
/// The three rules it holds are the task types' own
/// (`smithy_query(query="shape", shape="TaskType")`):
///
/// * a `Filter` task PROJECTS the source fields a run reads, so a flow that carries one
///   reads those fields and no others, and a flow that carries none reads every field a
///   record holds;
/// * a `Map` task maps one read field onto the `destinationField` it is written under, and
///   a field no task maps keeps its own name;
/// * a `Map_all` task maps every field, except the ones its
///   `EXCLUDE_SOURCE_FIELDS_LIST` property names — which are therefore not written at all.
#[derive(Debug, Default)]
struct FieldMapping {
    /// The fields a run reads, or `None` for every field a record holds.
    projected: Option<BTreeSet<String>>,
    /// The fields no run writes.
    excluded: BTreeSet<String>,
    /// The name a read field is written under, for the fields a task renames.
    renamed: BTreeMap<String, String>,
}

impl FieldMapping {
    /// The mapping `tasks` declare.
    fn of(tasks: &[Task]) -> Self {
        let mut mapping = Self::default();
        for task in tasks {
            match task.task_type.as_str() {
                TASK_TYPE_FILTER => {
                    mapping
                        .projected
                        .get_or_insert_with(BTreeSet::new)
                        .extend(task.source_fields.iter().cloned());
                }
                TASK_TYPE_MAP => {
                    if let (Some(source), Some(destination)) =
                        (task.source_fields.first(), task.destination_field.as_ref())
                    {
                        mapping.renamed.insert(source.clone(), destination.clone());
                    }
                }
                TASK_TYPE_MAP_ALL => {
                    let excluded = task
                        .task_properties
                        .as_ref()
                        .and_then(|properties| properties.get(EXCLUDE_SOURCE_FIELDS_PROPERTY));
                    if let Some(excluded) = excluded {
                        mapping.excluded.extend(
                            excluded
                                .split(',')
                                .map(str::trim)
                                .filter(|field| !field.is_empty())
                                .map(::std::string::ToString::to_string),
                        );
                    }
                }
                // Every other task type transforms a VALUE rather than deciding which
                // field is written where, so it is not part of the mapping.
                _ => {}
            }
        }
        mapping
    }

    /// `record` as this mapping writes it.
    fn applied_to(
        &self,
        record: &BTreeMap<String, ::serde_json::Value>,
    ) -> BTreeMap<String, ::serde_json::Value> {
        record
            .iter()
            .filter(|(field, _)| !self.excluded.contains(*field))
            .filter(|(field, _)| {
                self.projected
                    .as_ref()
                    .is_none_or(|projected| projected.contains(*field))
            })
            .map(|(field, value)| {
                let written_as = self.renamed.get(field).unwrap_or(field);
                (written_as.clone(), value.clone())
            })
            .collect()
    }
}
