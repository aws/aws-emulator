// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch Logs DESTINATION of a finished job run's log artifacts: which group and
//! stream they are delivered to, and which of them the configuration asks for.
//!
//! A monitoring configuration names destinations, and Amazon S3 is only one of them:
//! `MonitoringConfiguration$cloudWatchLoggingConfiguration` is "The Amazon CloudWatch
//! configuration for monitoring logs. You can configure your jobs to send log information to
//! CloudWatch", whose `enabled` member "Enables CloudWatch logging". So the same rule the
//! Amazon S3 side of [`super`] implements holds here: configuring a destination is not a
//! value to echo back — it makes the finished run's log content EXIST in that destination, so
//! a client that reads it once the run has finished finds the run's driver log there.
//!
//! # What is delivered, and where
//!
//! The same driver log the Amazon S3 artifact carries ([`super::driver_log_text`]), as LOG
//! EVENTS rather than as one gzip object: CloudWatch Logs holds a stream of events, one per
//! line, which is what `GetLogEvents` and `FilterLogEvents` read back.
//!
//! * the GROUP is `logGroupName`, "The name of the log group in Amazon CloudWatch Logs where
//!   you want to publish your logs", or [`DEFAULT_LOG_GROUP`] for a configuration that names
//!   none;
//! * the STREAM is keyed the way the run's Amazon S3 key is — by the application, the job
//!   run, the worker type and the log type — under the `logStreamNamePrefix` the
//!   configuration names ("Prefix for the CloudWatch log stream name"). The model states no
//!   stream scheme, so the keying is the recorded Amazon S3 layout
//!   ([`super::driver_log_key`]) and the two destinations cannot disagree about which run an
//!   artifact belongs to;
//! * WHICH artifacts are delivered is `logTypes`, keyed by worker type: "If you don't specify
//!   any log types, driver STDOUT and STDERR logs will be published to CloudWatch Logs by
//!   default" ([`DEFAULT_DRIVER_LOG_TYPES`]).
//!
//! What this service HAS of a finished run is its driver's standard error — the run's own
//! record, as [`super`] states — so that is what is delivered, to the stream its worker type
//! and log type name. A configuration that asks only for a stream this service has no content
//! for (the driver's standard output, an executor nothing ran) creates no stream and delivers
//! nothing, rather than an empty one that claims a worker wrote nothing.
//!
//! The delivery itself is [`super::publish_due`]'s, through the one vended-logs write every
//! emulated service delivering records on the account's behalf uses
//! ([`::service_common::cloudwatch_logs::delivered`]), so a delivered record and
//! `GetLogEvents` cannot disagree about it.

use std::collections::BTreeMap;

use super::*;
use crate::ops::types::{
    CloudWatchLoggingConfiguration, ConfigurationOverrides, MonitoringConfiguration,
};
use crate::state::JobRunRecord;

/// The log group a run's artifacts are delivered to when the configuration names none.
///
/// Source: EMR Serverless' own logging documentation, which publishes `/aws/emr-serverless`
/// as the group it sends a job run's logs to by default. It is not in the prefix CloudWatch
/// Logs keeps for groups of its own
/// ([`::service_common::cloudwatch_logs::RESERVED_LOG_GROUP_NAME_PREFIX`]), so the write
/// creates it the way it creates any other vended-logs destination.
pub const DEFAULT_LOG_GROUP: &str = "/aws/emr-serverless";

/// The log type of a driver's standard error, which is the artifact a finished run has.
///
/// Source: `CloudWatchLoggingConfiguration$logTypes`, whose "Array Members Valid Values" are
/// `STDOUT`, `STDERR`, `HIVE_LOG`, `TEZ_AM` and `SYSTEM_LOGS`.
pub const STDERR_LOG_TYPE: &str = "STDERR";

/// The log type of a driver's standard output; see [`STDERR_LOG_TYPE`] for the source.
pub const STDOUT_LOG_TYPE: &str = "STDOUT";

/// The log types a configuration that names none publishes: "If you don't specify any log
/// types, driver STDOUT and STDERR logs will be published to CloudWatch Logs by default"
/// (`CloudWatchLoggingConfiguration$logTypes`).
pub const DEFAULT_DRIVER_LOG_TYPES: &[&str] = &[STDOUT_LOG_TYPE, STDERR_LOG_TYPE];

/// Where one run's log artifacts are delivered in CloudWatch Logs.
///
/// Resolved from the configuration when the run is SUBMITTED and held on the run's record,
/// exactly as the Amazon S3 destination is ([`super::LogDestination`]): the destination a run
/// delivers to is the one its own submit was configured with, so a later update of the
/// application cannot move the artifacts of a run already in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudWatchDestination {
    /// The group the artifacts are delivered to.
    pub log_group: String,
    /// `logStreamNamePrefix`, with no trailing separator — `None` for a configuration that
    /// names none, whose streams are keyed by the run alone.
    pub stream_prefix: Option<String>,
    /// The `logTypes` the configuration asks for, keyed by worker type, or `None` for one
    /// that names none ([`DEFAULT_DRIVER_LOG_TYPES`]).
    pub log_types: Option<BTreeMap<String, Vec<String>>>,
}

/// The CloudWatch Logs destination a run submitted with these configurations delivers to, or
/// `None` when logging there is not enabled.
///
/// `ConfigurationOverrides$monitoringConfiguration` is "The override configurations for
/// monitoring", so a run that names a CloudWatch configuration of its own delivers as that
/// one says and a run that names none inherits the application's — the precedence the Amazon
/// S3 destination resolves by ([`super::log_destination`]).
#[must_use]
pub fn cloud_watch_destination(
    application: Option<&MonitoringConfiguration>,
    overrides: Option<&ConfigurationOverrides>,
) -> Option<CloudWatchDestination> {
    let overridden = overrides
        .and_then(|overrides| overrides.monitoring_configuration.as_ref())
        .and_then(configured_cloud_watch);
    let configuration = overridden.or_else(|| application.and_then(configured_cloud_watch))?;
    // "Enables CloudWatch logging" — a configuration carried with it off names no
    // destination, so nothing is delivered for the run.
    if !configuration.enabled {
        return None;
    }
    let log_group = configuration
        .log_group_name
        .as_deref()
        .map(str::trim)
        // `logGroupName` is modelled with CloudWatch Logs' own `@pattern` and `@length`, so
        // the framework refuses a value no group could carry before it reaches here; the
        // reading is kept because a value outside that domain names no group either way, and
        // the group a run then delivers to is the default one.
        .filter(|name| ::service_common::cloudwatch_logs::is_log_group_name(name))
        .unwrap_or(DEFAULT_LOG_GROUP)
        .to_string();
    let stream_prefix = configuration
        .log_stream_name_prefix
        .as_deref()
        .map(|prefix| prefix.trim().trim_matches('/'))
        .filter(|prefix| !prefix.is_empty())
        .map(str::to_string);
    let log_types = configuration.log_types.as_ref().and_then(|asked| {
        let held: BTreeMap<String, Vec<String>> = asked
            .iter()
            .map(|(worker_type, log_types)| (worker_type.clone(), log_types.clone()))
            .collect();
        (!held.is_empty()).then_some(held)
    });
    Some(CloudWatchDestination {
        log_group,
        stream_prefix,
        log_types,
    })
}

/// The `cloudWatchLoggingConfiguration` one monitoring configuration names, if any.
fn configured_cloud_watch(
    configuration: &MonitoringConfiguration,
) -> Option<CloudWatchLoggingConfiguration> {
    configuration.cloud_watch_logging_configuration.clone()
}

/// Whether `destination` asks for the artifact of `worker_type`'s `log_type`.
///
/// A configuration that names no `logTypes` asks for the DRIVER's standard output and
/// standard error ([`DEFAULT_DRIVER_LOG_TYPES`]); one that names them asks for exactly what
/// it names. Both halves are matched without regard to case: the worker and log types are
/// open strings in the model, so a configuration spelling one in another casing names the
/// same artifact.
#[must_use]
pub fn publishes(destination: &CloudWatchDestination, worker_type: &str, log_type: &str) -> bool {
    match destination.log_types.as_ref() {
        None => {
            is_driver_worker_type(worker_type)
                && DEFAULT_DRIVER_LOG_TYPES
                    .iter()
                    .any(|asked| asked.eq_ignore_ascii_case(log_type))
        }
        Some(asked) => asked.iter().any(|(worker, log_types)| {
            worker.trim().eq_ignore_ascii_case(worker_type)
                && log_types
                    .iter()
                    .any(|held| held.trim().eq_ignore_ascii_case(log_type))
        }),
    }
}

/// Whether `worker_type` is the worker type a DRIVER runs as, which is what the default log
/// types are the driver's of ([`super::driver_worker_type`] names the two).
fn is_driver_worker_type(worker_type: &str) -> bool {
    [SPARK_DRIVER_WORKER_TYPE, HIVE_DRIVER_WORKER_TYPE]
        .iter()
        .any(|driver| driver.eq_ignore_ascii_case(worker_type))
}

/// The stream one run's artifact is delivered to:
/// `<prefix>/applications/<applicationId>/jobs/<jobRunId>/<WORKER_TYPE>/<LOG_TYPE>`.
///
/// The keying under the prefix is the recorded Amazon S3 layout
/// ([`super::driver_log_key`]) — the model states no stream scheme — so an artifact names the
/// same run whichever destination it was delivered to.
#[must_use]
pub fn log_stream(
    destination: &CloudWatchDestination,
    application_id: &str,
    job_run_id: &str,
    worker_type: &str,
    log_type: &str,
) -> String {
    let keyed = format!(
        "{APPLICATIONS_SEGMENT}/{application_id}/{JOBS_SEGMENT}/{job_run_id}/\
         {worker_type}/{log_type}"
    );
    match destination.stream_prefix.as_deref() {
        Some(prefix) => format!("{prefix}/{keyed}"),
        None => keyed,
    }
}

/// The records one finished run's driver log is delivered as: its lines, in the order they
/// were recorded.
///
/// CloudWatch Logs holds EVENTS, so the log a reader fetches as one gzip object from Amazon
/// S3 is the same text here, one event per line. A blank line carries no event: an event's
/// message is what a reader reads, and CloudWatch Logs holds no empty one.
#[must_use]
pub fn log_records(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// The CloudWatch Logs delivery a finished run owes, or `None` when the run has no such
/// destination, the destination asks for no artifact this service has, or the run recorded no
/// line to deliver.
#[must_use]
pub fn delivery(record: &JobRunRecord) -> Option<PublishedStream> {
    let destination = record.cloud_watch_destination.as_ref()?;
    let worker_type = driver_worker_type(&record.job_run_type);
    // The artifact this service has of a finished run is its driver's standard error; a
    // destination that does not ask for it is delivered nothing.
    if !publishes(destination, worker_type, STDERR_LOG_TYPE) {
        return None;
    }
    let run = &record.job_run;
    let records = log_records(&driver_log_text(record));
    if records.is_empty() {
        return None;
    }
    Some(PublishedStream {
        log_group: destination.log_group.clone(),
        log_stream: log_stream(
            destination,
            &run.application_id,
            &run.job_run_id,
            worker_type,
            STDERR_LOG_TYPE,
        ),
        records,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// A `cloudWatchLoggingConfiguration`, as a monitoring configuration carries one.
    fn monitoring(configuration: CloudWatchLoggingConfiguration) -> MonitoringConfiguration {
        MonitoringConfiguration {
            cloud_watch_logging_configuration: Some(configuration),
            ..MonitoringConfiguration::default()
        }
    }

    /// An enabled configuration naming nothing else.
    fn enabled() -> CloudWatchLoggingConfiguration {
        CloudWatchLoggingConfiguration {
            enabled: true,
            ..CloudWatchLoggingConfiguration::default()
        }
    }

    /// One row of [`a_destination_is_what_an_enabled_configuration_names`]: the CloudWatch
    /// configuration the application carries, and the destination it has to resolve to.
    struct DestinationRow {
        case: &'static str,
        configuration: Option<CloudWatchLoggingConfiguration>,
        /// The group and stream prefix the destination carries, or `None` for a
        /// configuration that names no destination at all.
        expected: Option<(&'static str, Option<&'static str>)>,
    }

    /// RULE: a destination is the group and stream prefix the configuration names, with the
    /// published default group for one that names none, and no destination at all where
    /// CloudWatch logging is not enabled.
    #[test]
    fn a_destination_is_what_an_enabled_configuration_names() {
        let rows = [
            DestinationRow {
                case: "no configuration at all",
                configuration: None,
                expected: None,
            },
            DestinationRow {
                case: "logging not enabled",
                configuration: Some(CloudWatchLoggingConfiguration::default()),
                expected: None,
            },
            DestinationRow {
                case: "enabled, naming no group",
                configuration: Some(enabled()),
                expected: Some((DEFAULT_LOG_GROUP, None)),
            },
            DestinationRow {
                case: "a named group",
                configuration: Some(CloudWatchLoggingConfiguration {
                    log_group_name: Some("/jobs/emr".to_string()),
                    ..enabled()
                }),
                expected: Some(("/jobs/emr", None)),
            },
            DestinationRow {
                case: "a named stream prefix",
                configuration: Some(CloudWatchLoggingConfiguration {
                    log_stream_name_prefix: Some("runs/".to_string()),
                    ..enabled()
                }),
                expected: Some((DEFAULT_LOG_GROUP, Some("runs"))),
            },
            DestinationRow {
                case: "a group name no log group could carry",
                configuration: Some(CloudWatchLoggingConfiguration {
                    log_group_name: Some("has a space".to_string()),
                    ..enabled()
                }),
                expected: Some((DEFAULT_LOG_GROUP, None)),
            },
        ];
        for row in rows {
            let case = row.case;
            let application = row.configuration.map(monitoring);
            let actual = cloud_watch_destination(application.as_ref(), None);
            let expected = row.expected.map(|(log_group, stream_prefix)| {
                (log_group.to_string(), stream_prefix.map(str::to_string))
            });
            let seen = actual
                .as_ref()
                .map(|held| (held.log_group.clone(), held.stream_prefix.clone()));
            assert_eq!(
                seen, expected,
                "case {case:?}: expected {expected:?}, got {seen:?}"
            );
        }
    }

    /// One row of [`the_run_overrides_the_applications_destination`]: the group the
    /// application configures, the group the run's overrides configure (`None` for overrides
    /// naming no monitoring configuration at all, absent for a run that overrides nothing),
    /// and the group the two together have to resolve to.
    struct PrecedenceRow {
        case: &'static str,
        application: Option<&'static str>,
        run: Option<Option<&'static str>>,
        expected: Option<&'static str>,
    }

    /// RULE: a run's own `configurationOverrides` CloudWatch configuration wins over the
    /// application's, and a run that overrides nothing inherits the application's.
    #[test]
    fn the_run_overrides_the_applications_destination() {
        let group = |name: &str| CloudWatchLoggingConfiguration {
            log_group_name: Some(name.to_string()),
            ..enabled()
        };
        let rows = [
            PrecedenceRow {
                case: "neither",
                application: None,
                run: None,
                expected: None,
            },
            PrecedenceRow {
                case: "the application only",
                application: Some("/by-app"),
                run: None,
                expected: Some("/by-app"),
            },
            PrecedenceRow {
                case: "the run only",
                application: None,
                run: Some(Some("/by-run")),
                expected: Some("/by-run"),
            },
            PrecedenceRow {
                case: "the run over the application",
                application: Some("/by-app"),
                run: Some(Some("/by-run")),
                expected: Some("/by-run"),
            },
            PrecedenceRow {
                case: "overrides that name no monitoring configuration",
                application: Some("/by-app"),
                run: Some(None),
                expected: Some("/by-app"),
            },
        ];
        for row in rows {
            let case = row.case;
            let application = row.application.map(|name| monitoring(group(name)));
            let overrides = row.run.map(|named| ConfigurationOverrides {
                monitoring_configuration: named.map(|name| monitoring(group(name))),
                ..ConfigurationOverrides::default()
            });
            let actual = cloud_watch_destination(application.as_ref(), overrides.as_ref());
            let seen = actual.as_ref().map(|held| held.log_group.clone());
            let expected = row.expected.map(str::to_string);
            assert_eq!(
                seen, expected,
                "case {case:?}: expected the group {expected:?}, got {seen:?}"
            );
        }
    }

    /// RULE: a configuration naming no `logTypes` asks for the driver's standard output and
    /// standard error; one that names them asks for exactly what it names, whatever casing it
    /// spells the worker and log type in.
    #[test]
    fn which_artifacts_are_asked_for_follows_the_log_types() {
        let asked = |worker_type: &str, log_types: &[&str]| {
            let mut held = HashMap::new();
            held.insert(
                worker_type.to_string(),
                log_types.iter().map(|held| (*held).to_string()).collect(),
            );
            CloudWatchLoggingConfiguration {
                log_types: Some(held),
                ..enabled()
            }
        };
        // (case, configuration, worker type, log type, asked for)
        let rows: [(&str, CloudWatchLoggingConfiguration, &str, &str, bool); 8] = [
            (
                "no log types: the driver's standard error",
                enabled(),
                SPARK_DRIVER_WORKER_TYPE,
                STDERR_LOG_TYPE,
                true,
            ),
            (
                "no log types: the driver's standard output",
                enabled(),
                SPARK_DRIVER_WORKER_TYPE,
                STDOUT_LOG_TYPE,
                true,
            ),
            (
                "no log types: a Hive driver's standard error",
                enabled(),
                HIVE_DRIVER_WORKER_TYPE,
                STDERR_LOG_TYPE,
                true,
            ),
            (
                "no log types: an executor, which the default does not name",
                enabled(),
                "SPARK_EXECUTOR",
                STDERR_LOG_TYPE,
                false,
            ),
            (
                "no log types: a log type outside the default",
                enabled(),
                SPARK_DRIVER_WORKER_TYPE,
                "SYSTEM_LOGS",
                false,
            ),
            (
                "the driver's standard error asked for",
                asked(SPARK_DRIVER_WORKER_TYPE, &[STDERR_LOG_TYPE]),
                SPARK_DRIVER_WORKER_TYPE,
                STDERR_LOG_TYPE,
                true,
            ),
            (
                "only the driver's standard output asked for",
                asked(SPARK_DRIVER_WORKER_TYPE, &[STDOUT_LOG_TYPE]),
                SPARK_DRIVER_WORKER_TYPE,
                STDERR_LOG_TYPE,
                false,
            ),
            (
                "another worker type asked for",
                asked("TEZ_TASK", &[STDERR_LOG_TYPE]),
                SPARK_DRIVER_WORKER_TYPE,
                STDERR_LOG_TYPE,
                false,
            ),
        ];
        for (case, configuration, worker_type, log_type, expected) in rows {
            let application = monitoring(configuration);
            let destination = cloud_watch_destination(Some(&application), None)
                .expect("an enabled configuration names a destination");
            let actual = publishes(&destination, worker_type, log_type);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected asked-for={expected}, got {actual}"
            );
        }
    }

    /// RULE: the stream is the run's keying — the application, the job run, the worker type
    /// and the log type — under the configured prefix.
    #[test]
    fn the_stream_is_the_runs_keying_under_the_configured_prefix() {
        let rows: [(&str, Option<&str>, &str, &str); 3] = [
            (
                "no prefix",
                None,
                SPARK_DRIVER_WORKER_TYPE,
                "applications/app1/jobs/run1/SPARK_DRIVER/STDERR",
            ),
            (
                "a prefix",
                Some("runs/"),
                SPARK_DRIVER_WORKER_TYPE,
                "runs/applications/app1/jobs/run1/SPARK_DRIVER/STDERR",
            ),
            (
                "a nested prefix, a Hive driver",
                Some("/a/b/"),
                HIVE_DRIVER_WORKER_TYPE,
                "a/b/applications/app1/jobs/run1/HIVE_DRIVER/STDERR",
            ),
        ];
        for (case, prefix, worker_type, expected) in rows {
            let application = monitoring(CloudWatchLoggingConfiguration {
                log_stream_name_prefix: prefix.map(str::to_string),
                ..enabled()
            });
            let destination = cloud_watch_destination(Some(&application), None)
                .expect("an enabled configuration names a destination");
            let actual = log_stream(&destination, "app1", "run1", worker_type, STDERR_LOG_TYPE);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the records are the driver log's own lines, and a blank line carries no event.
    #[test]
    fn the_records_are_the_log_texts_lines() {
        let rows: [(&str, &str, usize); 4] = [
            ("one line", "a\n", 1),
            ("three lines", "a\nb\nc\n", 3),
            ("a blank line between two", "a\n\nb\n", 2),
            ("no line at all", "", 0),
        ];
        for (case, text, expected) in rows {
            let actual = log_records(text).len();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected} record(s), got {actual}"
            );
        }
    }
}
