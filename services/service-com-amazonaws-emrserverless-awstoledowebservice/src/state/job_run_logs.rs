// SPDX-License-Identifier: CC0-1.0

//! LOG PUBLICATION: the artifacts a job run writes into the destinations its monitoring
//! configuration names.
//!
//! An application may carry a `monitoringConfiguration` whose
//! `s3MonitoringConfiguration.logUri` is "The Amazon S3 destination URI for log
//! publishing" and whose `cloudWatchLoggingConfiguration` is "The Amazon CloudWatch
//! configuration for monitoring logs", and a job run may override either through
//! `configurationOverrides.monitoringConfiguration` ("The override configurations for
//! monitoring"). Configuring a destination is not a value to echo back: it makes the
//! run's log content EXIST in that destination, so a client that reads it
//! once the run has finished finds the run's driver log there — for a run
//! that ended in the modelled failure state exactly as for one that succeeded, since the
//! log is how a terminal state is diagnosed at all.
//!
//! # What is published, and what is not
//!
//! ONE artifact per finished run and destination: the DRIVER's standard error, under the
//! worker type the run's engine drives ([`driver_worker_type`]) — gzipped as one object in
//! Amazon S3, and as the stream of its own lines in CloudWatch Logs, which holds log events
//! rather than objects ([`cloud_watch`] states that destination's keying and which artifacts
//! it asks for). The Amazon S3 key layout
//! (`applications/<applicationId>/jobs/<jobRunId>/<WORKER_TYPE>/stderr.gz` under the
//! configured URI) is RECORDED behaviour — this service's Smithy model states no key
//! scheme — and it is written here once, for the publication that produces it and for
//! any later reader of this module.
//!
//! No executor artifact is published, for the same reason the run reports no resource
//! utilisation: nothing ran an executor, so there is no executor whose log could be
//! written. Nothing is published for a run that is still in flight either: the artifacts
//! are what the finished run left behind.
//!
//! The log's TEXT is the run's own persisted record — the submit, the identity that made
//! it, the engine and release it was admitted on, and every state the clock moved it
//! through with the `stateDetails` of each — rendered as a driver's log lines. It states
//! what this service did and nothing it did not: no line is written as though a workload
//! had been executed. The driver's `@sensitive` members (a `sparkSubmit` entry point and
//! its parameters, a `hive` query) are deliberately left OUT of the published text: the
//! value a caller marked sensitive is not written into an object a bucket policy exposes
//! more widely than the API does.
//!
//! # When it is published
//!
//! The publication is owed by the CLOCK, at the instant the run reached its terminal
//! state: [`super::JobRunRecord::settle`] marks it owed as it materialises that state, and
//! whichever later call settles the store drains what is owed
//! ([`super::ComAmazonawsEmrserverlessAwstoledowebserviceState::take_due_job_run_log_publications`])
//! and writes it, with the state guard already released. Nothing sleeps for it, nothing is
//! written before the run finished, and the artifacts of one run are written once however
//! many calls arrive afterwards.
//!
//! Publication is BEST EFFORT, as it is for the real service: a destination that does not
//! resolve (no such bucket) or refuses the write loses the artifact and is reported in
//! this service's own tracing — it never fails the call that happened to drain it, which
//! asked for none of this. Each destination is reached the way any emulated service reaches
//! another, through the in-process callback with the one shared client builder
//! ([`service_common::s3_client`]) or the one shared vended-logs write
//! ([`service_common::cloudwatch_logs::delivered`]): the artifact lands in the very store
//! `s3:ListObjectsV2` / `s3:GetObject` and `logs:GetLogEvents` read, and no connection
//! leaves this process.

/// The CloudWatch Logs destination of the same artifacts: which group and stream they are
/// delivered to, and which of them a configuration asks for.
pub mod cloud_watch;

use std::io::Write;
use std::time::SystemTime;

use super::*;
use crate::ops::types::{ConfigurationOverrides, JobDriver};

/// The media type the published artifact carries: the object IS a gzip stream, which a
/// reader fetches and decompresses, rather than a transport encoding of plain text.
pub const LOG_OBJECT_MEDIA_TYPE: &str = "application/x-gzip";

/// The first key segment under the configured URI, which keys the artifacts by the
/// application the run was submitted to.
pub const APPLICATIONS_SEGMENT: &str = "applications";

/// The segment under one application that keys the artifacts by job run.
pub const JOBS_SEGMENT: &str = "jobs";

/// The artifact one finished run publishes: the driver's standard error, gzipped.
pub const DRIVER_STDERR_OBJECT: &str = "stderr.gz";

/// The worker type a Spark application's driver runs as.
pub const SPARK_DRIVER_WORKER_TYPE: &str = "SPARK_DRIVER";

/// The worker type a Hive application's driver runs as.
pub const HIVE_DRIVER_WORKER_TYPE: &str = "HIVE_DRIVER";

/// The `Application$type` whose driver is [`HIVE_DRIVER_WORKER_TYPE`], matched without
/// regard to case: `EngineType` is an open string, and this service reports it in its own
/// casing ([`SUPPORTED_ENGINES`]) whatever casing the create asked in.
const HIVE_ENGINE_TYPE: &str = "hive";

/// How many lifecycle lines ONE run's driver log keeps.
///
/// A whole run produces one line for the submit and one per state the clock moves it
/// through ([`JOB_RUN_STAGES`], or the two a cancel inserts), so this is an order of
/// magnitude above what any run can produce; past it a line is dropped rather than
/// queued, so a record nobody reads cannot grow without limit.
pub const MAX_JOB_RUN_LOG_EVENTS: usize = 32;

/// The scheme of the `s3://` URI a log destination is named by.
const S3_SCHEME: &str = "s3";

/// Where one run's log artifacts are written: the bucket, and the key prefix inside it.
///
/// Resolved from the configuration when the run is SUBMITTED and held on the run's record,
/// so the destination a run publishes to is the one its own submit was configured with — a
/// later update of the application cannot move the artifacts of a run already in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogDestination {
    /// The bucket the artifacts are written into.
    pub bucket: String,
    /// The key prefix inside it, with no leading or trailing separator.
    pub prefix: String,
}

impl LogDestination {
    /// The destination `log_uri` names, or `None` for a value that names no bucket.
    ///
    /// `S3MonitoringConfiguration$logUri` targets `UriString`, which is only bounded in
    /// length and admits any text, so the value is read by the maintained URL parser
    /// rather than split by hand: the bucket is the URI's authority and the key prefix is
    /// its path. A URI written with a trailing separator names the same prefix as one
    /// without, and a URI that names only a bucket publishes at that bucket's root.
    ///
    /// Only the bucket and the prefix are read out of it. The write itself is made to the
    /// emulator's own object store over the in-process transport, whose address comes from
    /// the Region alone, so no part of a caller's value is ever a destination.
    #[must_use]
    pub fn parse(log_uri: &str) -> Option<Self> {
        let url = ::url::Url::parse(log_uri.trim()).ok()?;
        if url.scheme() != S3_SCHEME {
            return None;
        }
        let bucket = url.host_str()?;
        if bucket.is_empty() {
            return None;
        }
        let prefix = ::percent_encoding::percent_decode_str(url.path())
            .decode_utf8_lossy()
            .trim_matches('/')
            .to_string();
        Some(Self {
            bucket: bucket.to_string(),
            prefix,
        })
    }

    /// The key `relative` names under this destination: the prefix and the tail joined by
    /// a single separator (a destination with no prefix writes at the bucket's root).
    #[must_use]
    pub fn key(&self, relative: &str) -> String {
        if self.prefix.is_empty() {
            relative.to_string()
        } else {
            format!("{}/{relative}", self.prefix)
        }
    }
}

/// One line of a run's driver log: the instant the event happened, the state the run
/// carried from then on, and the `stateDetails` that went with it.
#[derive(Debug, Clone)]
pub struct JobRunLogEvent {
    pub at: SystemTime,
    pub state: String,
    pub state_details: Option<String>,
}

/// What the clock owes one finished run's destinations, taken out of the store as an owned
/// value so the state guard is released before anything is written.
///
/// A run may be configured with either destination, both or neither: each half is present
/// only when the run's own configuration named it.
#[derive(Debug, Clone)]
pub struct PendingPublication {
    /// The job run the artifacts belong to, for the tracing of a failed write.
    pub job_run_id: String,
    /// The object the Amazon S3 destination is owed, when one is configured.
    pub object: Option<PublishedObject>,
    /// The log events the CloudWatch Logs destination is owed, when one is configured and
    /// asks for the artifact this service has ([`cloud_watch::delivery`]).
    pub stream: Option<PublishedStream>,
}

/// One object an Amazon S3 destination is owed.
#[derive(Debug, Clone)]
pub struct PublishedObject {
    /// The bucket the configuration named.
    pub bucket: String,
    /// The key the artifact is written under ([`driver_log_key`]).
    pub key: String,
    /// The object's bytes: the driver log, gzipped.
    pub body: Vec<u8>,
}

/// The log events one CloudWatch Logs destination is owed.
#[derive(Debug, Clone)]
pub struct PublishedStream {
    /// The group the records are delivered to.
    pub log_group: String,
    /// The stream inside it ([`cloud_watch::log_stream`]).
    pub log_stream: String,
    /// The records themselves: the driver log's lines, in the order they were recorded.
    pub records: Vec<String>,
}

/// The destination a run submitted with these configurations publishes to, or `None` when
/// neither names an Amazon S3 destination.
///
/// `ConfigurationOverrides$monitoringConfiguration` is "The override configurations for
/// monitoring", so a run that names a `logUri` of its own publishes there and a run that
/// names none inherits the application's.
#[must_use]
pub fn log_destination(
    application: Option<&MonitoringConfiguration>,
    overrides: Option<&ConfigurationOverrides>,
) -> Option<LogDestination> {
    let overridden = overrides
        .and_then(|overrides| overrides.monitoring_configuration.as_ref())
        .and_then(configured_log_uri);
    overridden
        .or_else(|| application.and_then(configured_log_uri))
        .and_then(|log_uri| LogDestination::parse(&log_uri))
}

/// The `s3MonitoringConfiguration.logUri` one monitoring configuration names, if any.
fn configured_log_uri(configuration: &MonitoringConfiguration) -> Option<String> {
    configuration
        .s3_monitoring_configuration
        .as_ref()
        .and_then(|s3| s3.log_uri.clone())
        .filter(|log_uri| !log_uri.trim().is_empty())
}

/// The worker type the DRIVER of an application of type `job_run_type` runs as, which is
/// the key segment its artifacts are published under.
#[must_use]
pub fn driver_worker_type(job_run_type: &str) -> &'static str {
    if job_run_type.eq_ignore_ascii_case(HIVE_ENGINE_TYPE) {
        HIVE_DRIVER_WORKER_TYPE
    } else {
        SPARK_DRIVER_WORKER_TYPE
    }
}

/// The key one run's driver log is published under, relative to the bucket:
/// `<prefix>/applications/<applicationId>/jobs/<jobRunId>/<WORKER_TYPE>/stderr.gz`.
#[must_use]
pub fn driver_log_key(
    destination: &LogDestination,
    application_id: &str,
    job_run_id: &str,
    worker_type: &str,
) -> String {
    destination.key(&format!(
        "{APPLICATIONS_SEGMENT}/{application_id}/{JOBS_SEGMENT}/{job_run_id}/\
         {worker_type}/{DRIVER_STDERR_OBJECT}"
    ))
}

/// The driver log of one run, as text: what this service recorded about the run, in the
/// order it happened.
///
/// Built from the run's own persisted record, so the log cannot disagree with what
/// `GetJobRun` reports. See the module docs for what is deliberately absent from it.
#[must_use]
pub fn driver_log_text(record: &JobRunRecord) -> String {
    let run = &record.job_run;
    let submitted_at = run
        .created_at
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    let mut lines = vec![
        format!(
            "{submitted_at} INFO Job run {} submitted on application {} ({}).",
            run.job_run_id, run.application_id, run.arn
        ),
        format!(
            "{submitted_at} INFO Amazon EMR release {}, application type {}, driver {}.",
            run.release_label,
            record.job_run_type,
            driver_name(&run.job_driver)
        ),
        format!(
            "{submitted_at} INFO Submitted by {} under execution role {}.",
            run.created_by, run.execution_role
        ),
    ];
    for event in &record.log_events {
        let at = DateTime::from(event.at)
            .fmt(::aws_smithy_types::date_time::Format::DateTime)
            .unwrap_or_default();
        let level = level_for(&event.state);
        let details = event.state_details.clone().unwrap_or_default();
        lines.push(format!(
            "{at} {level} Job run {} is {}. {details}",
            run.job_run_id, event.state
        ));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// The `JobDriver` member the submit named, which is the driver the log belongs to. The
/// member's own values are `@sensitive` and are not written.
fn driver_name(driver: &JobDriver) -> &'static str {
    match driver {
        JobDriver::sparkSubmit(_) => "sparkSubmit",
        JobDriver::Hive(_) => "hive",
    }
}

/// The severity a lifecycle line carries: a run that failed is an error, one that was
/// cancelled is a warning, and everything else is informational.
fn level_for(state: &str) -> &'static str {
    match state {
        "FAILED" => "ERROR",
        "CANCELLED" | "CANCELLING" => "WARN",
        _ => "INFO",
    }
}

/// `text`, gzipped — or `None` when the framing itself failed.
///
/// Writing into a `Vec` cannot fail for want of space, so a failure here would be the
/// encoder's own; it answers `None` rather than a truncated stream, because publishing no
/// object at all is more honest than one a reader cannot decompress.
#[must_use]
pub fn gzipped(text: &str) -> Option<Vec<u8>> {
    let mut encoder = ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    encoder.write_all(text.as_bytes()).ok()?;
    encoder.finish().ok()
}

/// The artifacts a finished run owes its destinations, or `None` when it owes none — no
/// destination was configured, or neither takes the artifact this service has.
#[must_use]
pub fn publication(record: &JobRunRecord) -> Option<PendingPublication> {
    let object = record.log_destination.as_ref().and_then(|destination| {
        let run = &record.job_run;
        let key = driver_log_key(
            destination,
            &run.application_id,
            &run.job_run_id,
            driver_worker_type(&record.job_run_type),
        );
        let body = gzipped(&driver_log_text(record))?;
        Some(PublishedObject {
            bucket: destination.bucket.clone(),
            key,
            body,
        })
    });
    let stream = cloud_watch::delivery(record);
    if object.is_none() && stream.is_none() {
        return None;
    }
    Some(PendingPublication {
        job_run_id: record.job_run.job_run_id.clone(),
        object,
        stream,
    })
}

/// Materialise what the clock owes this service's store and publish the log artifacts of
/// every run it just finished.
///
/// Called by each operation that settles the store, at the one instant it settles it: the
/// publication belongs to the state change rather than to the call, so a client that only
/// ever polls one job run still finds the artifacts of the run it polled. The state guard
/// is taken and released before anything is written — nothing here holds it across an
/// await.
pub async fn publish_due<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let now = SystemTime::now();
    let publications = {
        let mut store = state.lock();
        store.settle_all(now);
        store.take_due_job_run_log_publications()
    };
    if publications.is_empty() {
        return;
    }
    let region = request_region(ctx.parts());
    let client = ::service_common::s3_client::s3_client(ctx.callback(), &region);
    for publication in publications {
        if let Some(object) = publication.object.as_ref() {
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
                    job_run_id = %publication.job_run_id,
                    bucket = %object.bucket,
                    key = %object.key,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a job run's log artifact could not be published to Amazon S3"
                );
            }
        }
        if let Some(stream) = publication.stream.as_ref() {
            let delivered = ::service_common::cloudwatch_logs::delivered(
                ctx,
                &region,
                &stream.log_group,
                &stream.log_stream,
                &stream.records,
            )
            .await;
            if !delivered {
                ::tracing::warn!(
                    job_run_id = %publication.job_run_id,
                    log_group = %stream.log_group,
                    log_stream = %stream.log_stream,
                    "a job run's log artifact could not be delivered to CloudWatch Logs"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;
    use crate::ops::types::S3MonitoringConfiguration;

    /// RULE: a destination is the bucket and the key prefix a `logUri` names, however the
    /// URI spells that prefix; a value naming no bucket names no destination.
    #[test]
    fn a_destination_is_the_bucket_and_prefix_a_log_uri_names() {
        let rows = [
            ("a prefix with a trailing separator", "s3://bucket/logs/", Some(("bucket", "logs"))),
            ("the same prefix without one", "s3://bucket/logs", Some(("bucket", "logs"))),
            ("a nested prefix", "s3://bucket/a/b/c/", Some(("bucket", "a/b/c"))),
            ("a bucket alone", "s3://bucket", Some(("bucket", ""))),
            ("a bucket with an empty path", "s3://bucket/", Some(("bucket", ""))),
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

    /// RULE: the key is the recorded layout under the configured prefix, keyed by the
    /// application, the job run and the worker type the engine's driver runs as.
    #[test]
    fn the_key_is_the_recorded_layout_under_the_configured_prefix() {
        let rows = [
            (
                "a prefix, a Spark driver",
                "s3://bucket/logs/",
                "Spark",
                "logs/applications/app1/jobs/run1/SPARK_DRIVER/stderr.gz",
            ),
            (
                "no prefix, a Spark driver",
                "s3://bucket",
                "SPARK",
                "applications/app1/jobs/run1/SPARK_DRIVER/stderr.gz",
            ),
            (
                "a nested prefix, a Hive driver",
                "s3://bucket/a/b/",
                "Hive",
                "a/b/applications/app1/jobs/run1/HIVE_DRIVER/stderr.gz",
            ),
        ];
        for (case, log_uri, engine_type, expected) in rows {
            let destination =
                LogDestination::parse(log_uri).expect("the fixture URI names a destination");
            let actual = driver_log_key(
                &destination,
                "app1",
                "run1",
                driver_worker_type(engine_type),
            );
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the artifact is a gzip stream whose text is what this service recorded about
    /// the run.
    #[test]
    fn the_artifact_is_a_gzip_stream_of_the_log_text() {
        for text in ["one line\n", "", "a\nb\nc\n"] {
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

    /// RULE: a run's own `configurationOverrides` destination wins over the application's,
    /// and a run that overrides nothing inherits the application's.
    #[test]
    fn the_run_overrides_the_applications_destination() {
        /// One row: the application's configured URI, the run's, and the prefix the two
        /// together have to resolve to.
        struct Row {
            case: &'static str,
            application: Option<&'static str>,
            run: Option<Option<&'static str>>,
            expected: Option<&'static str>,
        }
        let monitoring = |log_uri: &str| MonitoringConfiguration {
            s3_monitoring_configuration: Some(S3MonitoringConfiguration {
                log_uri: Some(log_uri.to_string()),
                encryption_key_arn: None,
            }),
            ..MonitoringConfiguration::default()
        };
        let rows = [
            Row {
                case: "neither",
                application: None,
                run: None,
                expected: None,
            },
            Row {
                case: "the application only",
                application: Some("s3://b/app/"),
                run: None,
                expected: Some("app"),
            },
            Row {
                case: "the run only",
                application: None,
                run: Some(Some("s3://b/run/")),
                expected: Some("run"),
            },
            Row {
                case: "the run over the application",
                application: Some("s3://b/app/"),
                run: Some(Some("s3://b/run/")),
                expected: Some("run"),
            },
            Row {
                case: "overrides that name no monitoring configuration",
                application: Some("s3://b/app/"),
                run: Some(None),
                expected: Some("app"),
            },
        ];
        for row in rows {
            let application = row.application.map(monitoring);
            let overrides = row.run.map(|log_uri| ConfigurationOverrides {
                monitoring_configuration: log_uri.map(monitoring),
                ..ConfigurationOverrides::default()
            });
            let actual = log_destination(application.as_ref(), overrides.as_ref());
            let expected_prefix = row.expected.map(str::to_string);
            let actual_prefix = actual.as_ref().map(|destination| destination.prefix.clone());
            assert_eq!(
                actual_prefix, expected_prefix,
                "case {:?}: expected the prefix {expected_prefix:?}, got {actual_prefix:?}",
                row.case
            );
        }
    }
}
