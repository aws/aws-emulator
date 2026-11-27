// SPDX-License-Identifier: CC0-1.0

//! The LOG DIRECTORY a finished step leaves in Amazon S3.
//!
//! "LogUri — the location in Amazon S3 to write the log files of the job flow"
//! (`RunJobFlow$LogUri` doc). Naming that location is not a value to echo back: Amazon EMR's
//! log push agent makes the step's log files EXIST there, so a client that waits for a step
//! to end and then reads its logs out of Amazon S3 finds them — for a step that failed
//! exactly as for one that completed, since reading those files is how a failure is diagnosed
//! at all.
//!
//! # What is written, and from what
//!
//! One directory per step, `<LogUri>/<cluster-id>/steps/<step-id>/`, holding the three files
//! a step's run produces ([`CONTROLLER_LOG_OBJECT`], [`STDOUT_LOG_OBJECT`],
//! [`STDERR_LOG_OBJECT`]), each gzipped — the recorded layout, which this service's Smithy
//! model states nothing about.
//!
//! Their text is the step's OWN run, never anything invented for the file:
//!
//! * the two console files are what the step's work wrote to its output streams
//!   ([`StepRunLog`]) — the submitted application's own `stdout` and `stderr` as the
//!   interpreter that ran it captured them ([`super::spark_step_run`]), or the two streams of
//!   the session that performed a submitted command ([`super::step_command_status`]). A step
//!   whose work wrote nothing to a stream leaves that file empty, which is what a real step
//!   that printed nothing leaves too;
//! * the controller file is what this service recorded about the step: the command it
//!   submitted, the instants of its own timeline and the status its work exited with, in the
//!   lines Amazon EMR's step controller writes them as ([`controller_log_text`]). A step whose
//!   work this service ran NOTHING for claims no exit status in it — it states the failure the
//!   step itself reports — so no file says a process ran that none did.
//!
//! # When it is written
//!
//! The delivery is owed by the CLOCK, from the instant the step ENDED: it belongs to the
//! step's own end rather than to any call, so whichever later operation reports the step or
//! its cluster drains what is owed ([`deliver_due_step_logs`]) with this service's store
//! already released. Nothing sleeps for it, nothing is written for a step still in flight,
//! and one step's files are written once however many calls arrive afterwards
//! ([`StepRecord::claimed_log_uri`], [`StepRecord::delivered_log_uri`]).
//!
//! The two records are kept APART because the write takes time and the calls that report a
//! step do not wait for it: the drain that takes a step's files CLAIMS the directory
//! ([`StepRecord::claimed_log_uri`]) so no second drain writes them, and the directory is
//! recorded as the step's ([`StepRecord::delivered_log_uri`]) only once every file of it has
//! landed. So a read that arrives while a delivery is in flight reports no log file — the files
//! are not there yet — and no read ever points a client at an object the store does not hold.
//!
//! Delivery is BEST EFFORT, as it is for the real agent: a destination that does not resolve
//! (no such bucket) or refuses the write loses that round and is reported in this service's own
//! tracing — it never fails the call that happened to drain it, and it leaves the step owed its
//! files again ([`ComAmazonawsEmrElasticmapreduceState::forget_delivered_step_logs`]), so
//! nothing ever reports a log file the object store does not hold. A delivery that is IN FLIGHT
//! is CLAIMED rather than recorded as delivered ([`StepRecord::claimed_log_uri`]), which is what
//! keeps a second drainer from writing the same files beside the first while leaving a read
//! taken meanwhile reporting no file: the path is reported only once the write has landed
//! ([`ComAmazonawsEmrElasticmapreduceState::record_delivered_step_logs`]). The write is made the
//! way any emulated service reaches another, over the in-process callback with the one shared
//! client builder ([`service_common::s3_client`]), so the files land in the very store
//! `s3:ListObjectsV2` and `s3:GetObject` read and no connection leaves this process.
//!
//! # Whose permissions decide whether a file lands
//!
//! The push is the NODE's: the agent runs on the cluster's Amazon EC2 instances, so it writes
//! with what those instances hold — the role of the instance profile the launch named, or a
//! step's own runtime role where it carries one ([`super::work_role_arn`]). So the permissions
//! in force on THAT role decide whether a file lands: a statement that denies `s3:PutObject`
//! on the key a file would be written under ([`PUT_OBJECT_ACTION`], asked of IAM through the
//! tree's one reading of what a role may do, [`::service_common::iam_role::role_denies_action`])
//! suppresses that write, and the step is then owed its files like any other round that did not
//! land — so a prefix whose write is refused stays EMPTY however many calls report the cluster,
//! and nothing points a reader at a file nobody could write.
//!
//! An empty or unreadable policy set is no such statement and suppresses nothing, exactly as
//! that reading states: a role IAM holds no identity policy for — the routine state of one
//! provisioned out of band — says nothing about what its nodes may do, and an emulator with no
//! IAM mounted answers nothing at all.

use std::io::Write;

use super::*;

/// The media type each delivered file carries: the object IS a gzip stream, which a reader
/// fetches and decompresses, rather than a transport encoding of plain text.
pub const LOG_OBJECT_MEDIA_TYPE: &str = "application/x-gzip";

/// The IAM action ONE delivered file's write is decided on: the action Amazon S3 names an
/// object write by.
///
/// SOURCE: the Service Authorization Reference for Amazon S3, where `PutObject` is the action
/// for "the `PUT Object` operation" — the call this delivery makes for each file.
pub const PUT_OBJECT_ACTION: &str = "s3:PutObject";

/// The ARN of the object at `key` in `bucket`, which is the resource that write is decided
/// against: an Amazon S3 object ARN names no Region and no account, so it is the bucket and
/// the key alone.
#[must_use]
pub fn object_arn(bucket: &str, key: &str) -> String {
    format!("arn:aws:s3:::{bucket}/{key}")
}

/// The key segment under one cluster's log prefix that keys the directories by step.
pub const STEPS_SEGMENT: &str = "steps";

/// The step controller's own log: what the service did with the step.
pub const CONTROLLER_LOG_OBJECT: &str = "controller.gz";

/// What the step's work wrote to its standard output.
pub const STDOUT_LOG_OBJECT: &str = "stdout.gz";

/// What the step's work wrote to its standard error.
pub const STDERR_LOG_OBJECT: &str = "stderr.gz";

/// Every file one step's log directory holds, in the order they are listed under it.
pub const STEP_LOG_OBJECTS: &[&str] = &[CONTROLLER_LOG_OBJECT, STDERR_LOG_OBJECT, STDOUT_LOG_OBJECT];

/// How much of ONE console stream a step's log file keeps.
///
/// CHOSEN, not an AWS limit: a step's work is caller-supplied, so what one step's files take
/// into this process is bounded here. Past it the stream is cut on a character boundary, so
/// what is delivered is still text.
pub const MAX_STEP_LOG_BYTES: usize = 16 * 1024;

/// The form a controller log line writes an instant in: the date and time, in UTC, as Amazon
/// EMR's controller writes them.
const CONTROLLER_TIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3fZ";

/// How much of a step's command line one controller log line writes.
///
/// CHOSEN, for the same reason the console streams are bounded: the command line is the
/// caller's own, so what a log line carries of it is capped rather than following from how long
/// a submitted command is.
const MAX_CONTROLLER_COMMAND_BYTES: usize = 4 * 1024;

/// What a step's work WROTE to its output streams, and the status it exited with.
///
/// Kept on the step's outcome ([`StepOutcome::log`]) by whatever ran the work, so the files
/// delivered for a step are that run's own output and nothing else. `exit_status` is `None`
/// for a step whose work this service ran nothing for: there was no process, so there is no
/// status, and the controller log says so instead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StepRunLog {
    /// What the work wrote to its standard output.
    pub stdout: String,
    /// What the work wrote to its standard error.
    pub stderr: String,
    /// The status the work exited with, when something ran it.
    pub exit_status: Option<i32>,
}

impl StepRunLog {
    /// This log with each stream cut to [`MAX_STEP_LOG_BYTES`], on a character boundary.
    #[must_use]
    pub fn bounded(self) -> Self {
        Self {
            stdout: bounded_stream(self.stdout),
            stderr: bounded_stream(self.stderr),
            exit_status: self.exit_status,
        }
    }
}

/// `text` cut to `limit` bytes on a character boundary.
fn bounded_text(text: String, limit: usize) -> String {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut kept = text;
    kept.truncate(end);
    kept
}

/// `text` cut to [`MAX_STEP_LOG_BYTES`] on a character boundary.
fn bounded_stream(text: String) -> String {
    bounded_text(text, MAX_STEP_LOG_BYTES)
}

/// Where one cluster's logs are written: the bucket, and the key prefix inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogDestination {
    /// The bucket the files are written into.
    pub bucket: String,
    /// The key prefix inside it, with no leading or trailing separator — empty for a URI
    /// that names a bucket alone.
    pub prefix: String,
}

impl LogDestination {
    /// The destination `log_uri` names, or `None` for a value that names no bucket.
    ///
    /// `LogUri` targets `XmlString`, which is only bounded in length, so the value is read as
    /// the Amazon S3 URI a cluster's own paths are written as ([`S3_URI_SCHEMES`]): the
    /// authority is the bucket and the path is the key prefix. A URI written with a trailing
    /// separator names the same prefix as one without, and a URI naming only a bucket writes
    /// at that bucket's root.
    ///
    /// Only the bucket and the prefix are read out of it: the write itself is made to this
    /// emulator's own object store, whose address comes from the Region alone, so no part of
    /// a caller's value is ever a destination.
    #[must_use]
    pub fn parse(log_uri: &str) -> Option<Self> {
        let trimmed = log_uri.trim();
        let rest = S3_URI_SCHEMES
            .iter()
            .find_map(|scheme| trimmed.strip_prefix(scheme))?;
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        if bucket.is_empty() {
            return None;
        }
        Some(Self {
            bucket: bucket.to_string(),
            prefix: prefix.trim_matches('/').to_string(),
        })
    }

    /// The key `relative` names under this destination: the prefix and the tail joined by a
    /// single separator (a destination with no prefix writes at the bucket's root).
    #[must_use]
    pub fn key(&self, relative: &str) -> String {
        if self.prefix.is_empty() {
            relative.to_string()
        } else {
            format!("{}/{relative}", self.prefix)
        }
    }

    /// The `s3://` URI the key `relative` is addressed by, which is what a reader is handed.
    #[must_use]
    pub fn uri(&self, relative: &str) -> String {
        format!("s3://{}/{}", self.bucket, self.key(relative))
    }
}

/// The key one step's log directory holds its files under, relative to the bucket:
/// `<prefix>/<cluster-id>/steps/<step-id>`.
#[must_use]
pub fn step_log_prefix(destination: &LogDestination, cluster_id: &str, step_id: &str) -> String {
    destination.key(&format!("{cluster_id}/{STEPS_SEGMENT}/{step_id}"))
}

/// The controller log of one step, as text.
///
/// Built from the step's own persisted record, so it cannot disagree with what `DescribeStep`
/// reports: the jar and command line it submitted, the instants of its timeline, and how it
/// ended — the status its work exited with where something ran it, and otherwise the failure
/// the step itself reports. `ordinal` is the step's place in its cluster's queue, which is how
/// the controller names a step beside its id.
#[must_use]
pub fn controller_log_text(step: &StepRecord, ordinal: usize) -> String {
    let at = |instant: Option<SystemTime>| -> String {
        ::chrono::DateTime::<::chrono::Utc>::from(instant.unwrap_or(step.creation_date_time))
            .format(CONTROLLER_TIME_FORMAT)
            .to_string()
    };
    let created = at(Some(step.creation_date_time));
    let started = at(step.start_date_time);
    let ended = at(step.end_date_time);
    let command = bounded_text(
        ::shell_words::join(&step.args),
        MAX_CONTROLLER_COMMAND_BYTES,
    );
    let mut lines = vec![
        format!(
            "{created} INFO Ensure step {ordinal} jar file {}",
            step.jar
        ),
        format!("{started} INFO StepRunner: Created Runner for step {ordinal}"),
        format!("{started} INFO startExec '{} {command}'", step.jar),
        format!(
            "{started} INFO HadoopJarStepRunner.Runner: startRun() called for step {ordinal} \
             ({})",
            step.id
        ),
    ];
    let status = step.outcome.as_ref().and_then(|outcome| {
        outcome
            .log
            .as_ref()
            .and_then(|log: &StepRunLog| log.exit_status)
    });
    if let Some(status) = status {
        lines.push(format!(
            "{ended} INFO waitProcessCompletion ended with exit code {status} : {} {command}",
            step.jar
        ));
        if let Some(seconds) = run_seconds(step) {
            lines.push(format!(
                "{ended} INFO total process run time: {seconds} seconds"
            ));
        }
        lines.push(format!("{ended} INFO Step created jobs: "));
        lines.push(if status == 0 {
            format!("{ended} INFO Step succeeded with exitCode {status} and Error Message: ")
        } else {
            format!(
                "{ended} WARN Step failed with exitCode {status} and Error Message: {}",
                failure_message(step)
            )
        });
    } else {
        // Nothing ran the step's work, so there is no process and no status to report: what
        // the controller states is what the step itself reports about its end.
        lines.push(format!(
            "{ended} INFO Step reached state {} with Error Message: {}",
            step.state,
            failure_message(step)
        ));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// How long the step's own timeline says its work took, in whole seconds — `None` for a step
/// whose timeline is not closed.
fn run_seconds(step: &StepRecord) -> Option<u64> {
    let started = step.start_date_time?;
    let ended = step.end_date_time?;
    Some(ended.duration_since(started).unwrap_or_default().as_secs())
}

/// The message the step reports about its end, or the empty string when it reports none: the
/// controller log states exactly what `DescribeStep` does.
fn failure_message(step: &StepRecord) -> String {
    step.outcome
        .as_ref()
        .and_then(|outcome| outcome.failure.as_ref())
        .map(|failure| failure.message.clone())
        .unwrap_or_default()
}

/// What the step's standard-error file holds: what its work wrote there, and the line the
/// command runner ends with once the work has exited.
#[must_use]
pub fn stderr_log_text(log: Option<&StepRunLog>) -> String {
    let mut text = log.map(|log| log.stderr.clone()).unwrap_or_default();
    if let Some(status) = log.and_then(|log| log.exit_status) {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("Command exiting with ret '{status}'\n"));
    }
    text
}

/// What the step's standard-output file holds: what its work wrote there, and nothing else.
#[must_use]
pub fn stdout_log_text(log: Option<&StepRunLog>) -> String {
    log.map(|log| log.stdout.clone()).unwrap_or_default()
}

/// `text`, gzipped — or `None` when the framing itself failed.
///
/// Writing into a `Vec` cannot fail for want of space, so a failure here would be the
/// encoder's own; it answers `None` rather than a truncated stream, because delivering no file
/// at all is more honest than one a reader cannot decompress.
#[must_use]
pub fn gzipped(text: &str) -> Option<Vec<u8>> {
    let mut encoder =
        ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    encoder.write_all(text.as_bytes()).ok()?;
    encoder.finish().ok()
}

/// One file a finished step's log directory is owed.
#[derive(Debug, Clone)]
pub struct PendingLogObject {
    /// The bucket the cluster's `LogUri` named.
    pub bucket: String,
    /// The key the file is written under.
    pub key: String,
    /// The file's bytes: its text, gzipped.
    pub body: Vec<u8>,
}

/// The files one finished step owes its cluster's log destination.
#[derive(Debug, Clone)]
pub struct PendingStepLogs {
    /// The step they belong to, for the tracing of a failed write.
    pub step_id: String,
    /// The `s3://` directory they are being written to, which is the path the step reports its
    /// log file under ONCE they have landed there.
    pub directory: String,
    /// WHO the push is made by, as the launch named it: the step's own runtime role where it
    /// carries one, and otherwise the cluster's instance profile, whose role the nodes that
    /// push the files assume. The permissions in force on that role are what decide whether a
    /// file lands (see the module docs); `None` for a cluster that named neither.
    pub pushed_by: Option<String>,
    /// The files themselves, one per name in [`STEP_LOG_OBJECTS`].
    pub objects: Vec<PendingLogObject>,
}

impl ComAmazonawsEmrElasticmapreduceState {
    /// The log files the finished steps of the cluster `cluster_id` are owed, with the delivery
    /// of each CLAIMED.
    ///
    /// Called with the store held, right after the step transitions were materialised, and
    /// answers nothing for a cluster this account holds none of or one whose launch named no
    /// `LogUri`: with no destination configured there is nowhere a log file could exist, which
    /// is also what the real service does with such a cluster.
    ///
    /// A step owes its files from the instant it ENDED, and owes them once: the claim
    /// ([`StepRecord::claimed_log_uri`]) is what keeps a second drainer from writing the same
    /// files beside this one, and the directory they LANDED in is recorded separately, once the
    /// write has gone through ([`Self::record_delivered_step_logs`]) — which is the path a
    /// failed step reports its log file under, so a delivery still in flight points a reader at
    /// nothing.
    pub fn take_due_step_logs(&mut self, cluster_id: &str) -> Vec<PendingStepLogs> {
        let Some(cluster) = self.clusters.get_mut(cluster_id) else {
            return Vec::new();
        };
        // A step's own `StepMonitoringConfiguration` names where that step's logs go, and the
        // cluster's `LogUri` is where the rest of them go.
        let cluster_destination = cluster
            .log_uri
            .as_deref()
            .and_then(LogDestination::parse);
        // WHO the cluster's nodes push with, read before the steps are walked: a step with no
        // runtime role of its own is pushed by the instance profile the launch named.
        let cluster_pushed_by = cluster.job_flow_role.clone();
        let mut owed: Vec<PendingStepLogs> = Vec::new();
        for (index, step) in cluster.steps.iter_mut().enumerate() {
            // Only a step that really RAN leaves a log directory: one the queue dropped
            // before its turn came started no work, so it wrote nothing anywhere.
            if !step.is_terminal() || step.start_date_time.is_none() {
                continue;
            }
            if step.delivered_log_uri.is_some() || step.claimed_log_uri.is_some() {
                continue;
            }
            let Some(destination) = step
                .log_uri
                .as_deref()
                .and_then(LogDestination::parse)
                .or_else(|| cluster_destination.clone())
            else {
                continue;
            };
            let prefix = step_log_prefix(&destination, cluster_id, &step.id);
            let log = step.outcome.as_ref().and_then(|outcome| outcome.log.as_ref());
            let texts = [
                (CONTROLLER_LOG_OBJECT, controller_log_text(step, index + 1)),
                (STDERR_LOG_OBJECT, stderr_log_text(log)),
                (STDOUT_LOG_OBJECT, stdout_log_text(log)),
            ];
            let objects: Vec<PendingLogObject> = texts
                .iter()
                .filter_map(|(name, text)| {
                    Some(PendingLogObject {
                        bucket: destination.bucket.clone(),
                        key: format!("{prefix}/{name}"),
                        body: gzipped(text)?,
                    })
                })
                .collect();
            if objects.len() != STEP_LOG_OBJECTS.len() {
                continue;
            }
            let directory = format!("s3://{}/{prefix}", destination.bucket);
            step.claimed_log_uri = Some(directory.clone());
            owed.push(PendingStepLogs {
                step_id: step.id.clone(),
                directory,
                pushed_by: step
                    .execution_role_arn
                    .clone()
                    .or_else(|| cluster_pushed_by.clone()),
                objects,
            });
        }
        owed
    }

    /// Record that every file of the steps `delivered` — each `(step id, directory)` — landed in
    /// the directory it was written to, which is the path those steps report their log file
    /// under from now on, and release the claim the delivery was made under.
    pub fn record_delivered_step_logs(&mut self, cluster_id: &str, delivered: &[(String, String)]) {
        let Some(cluster) = self.clusters.get_mut(cluster_id) else {
            return;
        };
        for step in &mut cluster.steps {
            if let Some((_, directory)) = delivered.iter().find(|(id, _)| *id == step.id) {
                step.delivered_log_uri = Some(directory.clone());
                step.claimed_log_uri = None;
            }
        }
    }

    /// Give back the claim the delivery for the steps `step_ids` of the cluster `cluster_id` was
    /// made under: their write did not go through, so the next call that reports the cluster
    /// writes those files rather than the step reporting a log file nothing holds.
    pub fn forget_delivered_step_logs(&mut self, cluster_id: &str, step_ids: &[String]) {
        let Some(cluster) = self.clusters.get_mut(cluster_id) else {
            return;
        };
        for step in &mut cluster.steps {
            if step_ids.contains(&step.id) {
                step.claimed_log_uri = None;
                step.delivered_log_uri = None;
            }
        }
    }
}

/// Write the log directory every finished step of the cluster `cluster_id` is owed.
///
/// Called by the operations that report a step or its cluster, the way a step's own run is
/// performed: the delivery was owed from the instant the step ended, whoever asks about it
/// next, and the write may not happen under this service's lock. The store is taken only to
/// materialise the steps' transitions and to take what is owed.
///
/// Each file is written only where the permissions in force on the role the push is made by
/// do not deny the object write (see the module docs), so a step whose files IAM refuses is
/// left owed them rather than reporting a directory the object store does not hold — and the
/// directory is recorded on the step only once EVERY file of it has landed, so a read taken
/// while this write is in flight reports no log file either.
pub async fn deliver_due_step_logs<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    cluster_id: &str,
    now: SystemTime,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let owed = {
        let mut store = state.lock();
        // The queue is run up to this instant first, so a step whose span has passed has
        // ENDED and its log directory is among the owed ones.
        run_due_steps(engine, &mut store, cluster_id, now);
        store.take_due_step_logs(cluster_id)
    };
    if owed.is_empty() {
        return;
    }
    let client = ::service_common::s3_client::s3_client(ctx.callback(), region);
    let mut undelivered: Vec<String> = Vec::new();
    let mut delivered: Vec<(String, String)> = Vec::new();
    for logs in owed {
        // The ROLE the node's push is authorized against, resolved once for this step's
        // directory: its own runtime role, or the role the cluster's instances assume through
        // their instance profile. Nothing resolved is nothing known about the permissions in
        // force, which withholds no write (see the module docs).
        let pushed_by = match logs.pushed_by.as_deref() {
            Some(named) => super::work_role_arn(ctx, region, named).await,
            None => None,
        };
        let mut landed = true;
        for object in &logs.objects {
            if let Some(role_arn) = pushed_by.as_deref() {
                let resource = object_arn(&object.bucket, &object.key);
                if ::service_common::iam_role::role_denies_action(
                    ctx,
                    region,
                    role_arn,
                    PUT_OBJECT_ACTION,
                    &resource,
                )
                .await
                {
                    ::tracing::warn!(
                        cluster_id = %cluster_id,
                        step_id = %logs.step_id,
                        role_arn = %role_arn,
                        resource = %resource,
                        "an Amazon EMR step log file was not written: the permissions in force \
                         on the role the cluster's nodes run as deny the object write"
                    );
                    landed = false;
                    break;
                }
            }
            if let Err(error) = client
                .put_object()
                .bucket(&object.bucket)
                .key(&object.key)
                .content_type(LOG_OBJECT_MEDIA_TYPE)
                .body(::aws_sdk_s3::primitives::ByteStream::from(
                    object.body.clone(),
                ))
                .send()
                .await
            {
                ::tracing::warn!(
                    cluster_id = %cluster_id,
                    step_id = %logs.step_id,
                    bucket = %object.bucket,
                    key = %object.key,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "an Amazon EMR step log file could not be delivered to Amazon S3"
                );
                landed = false;
                break;
            }
        }
        if landed {
            delivered.push((logs.step_id, logs.directory));
        } else {
            undelivered.push(logs.step_id);
        }
    }
    // A step whose files LANDED reports its directory from now on, and one whose files did not
    // reports no log file and is owed them again: nothing points a reader at an object the
    // object store does not hold.
    if !delivered.is_empty() || !undelivered.is_empty() {
        let mut store = state.lock();
        store.record_delivered_step_logs(cluster_id, &delivered);
        store.forget_delivered_step_logs(cluster_id, &undelivered);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    /// RULE: a destination is the bucket and the key prefix a `LogUri` names, however the URI
    /// spells that prefix; a value naming no bucket names no destination.
    #[test]
    fn a_destination_is_the_bucket_and_prefix_a_log_uri_names() {
        let rows = [
            ("a prefix with a trailing separator", "s3://bucket/logs/", Some(("bucket", "logs"))),
            ("the same prefix without one", "s3://bucket/logs", Some(("bucket", "logs"))),
            ("a nested prefix", "s3://bucket/a/b/c/", Some(("bucket", "a/b/c"))),
            ("a bucket alone", "s3://bucket", Some(("bucket", ""))),
            ("a bucket with an empty path", "s3://bucket/", Some(("bucket", ""))),
            ("the S3A scheme", "s3a://bucket/logs/", Some(("bucket", "logs"))),
            ("surrounding whitespace", "  s3://bucket/logs/  ", Some(("bucket", "logs"))),
            ("another scheme", "https://bucket/logs/", None),
            ("no scheme at all", "bucket/logs", None),
            ("no bucket", "s3:///logs", None),
        ];
        for (case, log_uri, expected) in rows {
            let actual = LogDestination::parse(log_uri);
            let expected = expected.map(|(bucket, prefix)| LogDestination {
                bucket: bucket.to_string(),
                prefix: prefix.to_string(),
            });
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a step's log directory is `<LogUri>/<cluster-id>/steps/<step-id>/`.
    #[test]
    fn a_steps_directory_is_keyed_by_its_cluster_and_its_id() {
        let rows = [
            ("a prefix", "s3://bucket/logs/", "logs/j-1/steps/s-1"),
            ("no prefix", "s3://bucket", "j-1/steps/s-1"),
            ("a nested prefix", "s3://bucket/a/b/", "a/b/j-1/steps/s-1"),
        ];
        for (case, log_uri, expected) in rows {
            let destination =
                LogDestination::parse(log_uri).expect("the row's URI names a destination");
            let actual = step_log_prefix(&destination, "j-1", "s-1");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: each delivered file is a gzip stream of its own text, whatever that text's
    /// length.
    #[test]
    fn a_delivered_file_is_a_gzip_stream_of_its_text() {
        for text in ["", "one line\n", "a\nb\nc\n"] {
            let body = gzipped(text).expect("the framing succeeds for text of any length");
            assert_eq!(
                body.get(..2),
                Some([0x1f, 0x8b].as_slice()),
                "case {text:?}: expected a gzip stream, got {:?}",
                body.get(..2)
            );
            let mut decoded = String::new();
            ::flate2::read::GzDecoder::new(body.as_slice())
                .read_to_string(&mut decoded)
                .unwrap_or_else(|error| panic!("case {text:?}: not a gzip stream: {error}"));
            assert_eq!(
                decoded, text,
                "case {text:?}: expected the text back, got {decoded:?}"
            );
        }
    }

    /// RULE: the standard-error file carries what the work wrote there and the status it
    /// exited with; a step whose work nothing ran claims no status at all.
    #[test]
    fn the_error_file_carries_the_streams_text_and_the_exit_status() {
        let rows: [(&str, Option<StepRunLog>, &str); 4] = [
            ("nothing ran", None, ""),
            (
                "a clean exit with no output",
                Some(StepRunLog {
                    exit_status: Some(0),
                    ..StepRunLog::default()
                }),
                "Command exiting with ret '0'\n",
            ),
            (
                "a failure with a traceback",
                Some(StepRunLog {
                    stderr: "Traceback (most recent call last):\n".to_string(),
                    exit_status: Some(1),
                    ..StepRunLog::default()
                }),
                "Traceback (most recent call last):\nCommand exiting with ret '1'\n",
            ),
            (
                "output with no trailing newline",
                Some(StepRunLog {
                    stderr: "cut short".to_string(),
                    exit_status: Some(3),
                    ..StepRunLog::default()
                }),
                "cut short\nCommand exiting with ret '3'\n",
            ),
        ];
        for (case, log, expected) in rows {
            let actual = stderr_log_text(log.as_ref());
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a stream is cut to what one file keeps, on a character boundary, and shorter
    /// text is kept whole.
    #[test]
    fn a_kept_stream_is_bounded_and_still_text() {
        let rows: [(&str, usize); 3] = [
            ("well inside the bound", 8),
            ("exactly the bound", MAX_STEP_LOG_BYTES),
            ("past the bound", MAX_STEP_LOG_BYTES + 64),
        ];
        for (case, characters) in rows {
            // Three bytes per character, so a cut that ignored boundaries would split one.
            let written = "☃".repeat(characters);
            let kept = StepRunLog {
                stdout: written.clone(),
                ..StepRunLog::default()
            }
            .bounded()
            .stdout;
            assert!(
                kept.len() <= MAX_STEP_LOG_BYTES,
                "case {case:?}: expected at most {MAX_STEP_LOG_BYTES} bytes, got {}",
                kept.len()
            );
            assert!(
                kept.chars().all(|character| character == '☃'),
                "case {case:?}: expected whole characters, got {kept:?}"
            );
            if written.len() <= MAX_STEP_LOG_BYTES {
                assert_eq!(
                    kept, written,
                    "case {case:?}: text inside the bound is kept whole"
                );
            }
        }
    }
}
