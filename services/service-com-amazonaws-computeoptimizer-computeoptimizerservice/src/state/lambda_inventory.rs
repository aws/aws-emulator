// SPDX-License-Identifier: CC0-1.0

//! The AWS Lambda function versions Compute Optimizer has analysed FOR AN ACCOUNT.
//!
//! Compute Optimizer "generates recommendations for Lambda functions that meet a specific set
//! of requirements", so the functions it can report are the ones the account really holds in
//! the Region the request addressed and nothing else: a function the Lambda emulator does not
//! hold has no recommendation at all, and an account that holds no function reads the empty
//! list. The inventory is AWS Lambda's own store — the service that owns a function — and the
//! utilization is what that service publishes for it: the `AWS/Lambda` `Invocations` and
//! `Duration` series of the invoked resource, and the memory an invocation used as the run
//! report Lambda writes into the function's log group states it.
//!
//! The same shape as the other inventory analyses of this service: the Amazon EBS analysis
//! reads the Amazon EC2 inventory and the `AWS/EBS` series ([`super::ebs_inventory`]), the
//! Amazon ECS analysis the ECS inventory and the `AWS/ECS` series ([`super::ecs_analysis`]),
//! and the database analysis the Amazon RDS inventory ([`super::rds_inventory`]).
//!
//! [`analysed_functions`] is the ONE selection every Lambda function surface of this service
//! reads — the recommendation reader, the summary that counts those rows and the export of the
//! same rows — so no two of them can disagree about which functions the account has an
//! analysis for. What is DERIVED from one observation lives in [`super::functions`].
//!
//! Reads only: the Lambda inventory, the metric store and the log store are other services'
//! state, and nothing here writes to any of them, nor to this service's own store. Each of
//! those stores is read under its OWN lock, taken and released in turn, so no two of them are
//! ever held at once; and every pass is bounded by a ceiling stated here
//! ([`MAX_ANALYSED_FUNCTION_VERSIONS`] and the ceilings beside it), so no number of functions,
//! versions, log streams or log events another service's store holds can size the work one
//! request does.

use super::*;

use ::service_com_amazonaws_cloudwatchlogs_logs_20140328 as logs;
use ::service_com_amazonaws_lambda_awsgirapiservice as lambda;

/// The CloudWatch namespace AWS Lambda publishes a function's metrics in, and the two series
/// this analysis reads out of it: how often the function ran, and how long an invocation took.
///
/// Taken from the service that AUTHORS them (`lambda::state::metrics`) rather than spelled
/// again here, so the series this analysis reads are the series that service writes.
pub const LAMBDA_METRIC_NAMESPACE: &str = lambda::state::metrics::METRIC_NAMESPACE;
/// See [`LAMBDA_METRIC_NAMESPACE`].
pub const LAMBDA_INVOCATIONS_METRIC: &str = lambda::state::metrics::INVOCATIONS_METRIC;
/// See [`LAMBDA_METRIC_NAMESPACE`].
pub const LAMBDA_DURATION_METRIC: &str = lambda::state::metrics::DURATION_METRIC;

/// How many function versions ONE analysis builds a row for, whatever the Lambda inventory
/// holds: the ceiling of the sweep itself, so the work behind a read is bounded by a number
/// THIS service states rather than by how many functions and versions a caller created.
///
/// Stated as a multiple of the largest page any reader of this service hands out
/// ([`MAX_PAGE_SIZE`]) rather than as a figure of its own, so the bound can never cut a page
/// short; past it the sweep stops rather than growing with the inventory.
pub const MAX_ANALYSED_FUNCTION_VERSIONS: usize = 2 * MAX_PAGE_SIZE;

/// How many log events ONE analysis examines per function log group, and how many run reports
/// it keeps per function version — the ceilings of the memory side of the sweep, for the same
/// reason: a caller can write log events and streams without limit, so what this service reads
/// out of them per request is bounded here.
const MAX_SCANNED_LOG_EVENTS: usize = 20_000;
/// See [`MAX_SCANNED_LOG_EVENTS`].
const MAX_ANALYSED_RUN_REPORTS: usize = 2_000;
/// How many observation intervals ONE analysis folds a function version's published series
/// into — the same ceiling for the metric side of the sweep.
const MAX_ANALYSED_SAMPLES: usize = 2_000;

/// What the plain-text run report states the memory an invocation used under, and the member
/// the structured record carries the same figure in.
///
/// AWS Lambda closes every invocation with a report of what the run spent — "`REPORT
/// RequestId: … Memory Size: 256 MB Max Memory Used: 42 MB`" in the runtime's plain-text
/// framing, and the `maxMemoryUsedMB` metric of its `platform.report` record in the structured
/// one — and that figure is the only place the memory an invocation really used is observed.
/// So it is what this analysis sizes a function's memory from, whichever format the function's
/// logs are delivered in. The framing is the runtime's own and the service that writes it
/// exports no reader for it, so the two forms are read here.
const REPORT_MAX_MEMORY_FIELD: &str = "Max Memory Used: ";
/// See [`REPORT_MAX_MEMORY_FIELD`].
const REPORT_MAX_MEMORY_MEMBER: &str = "maxMemoryUsedMB";
/// The unit the plain-text field states that figure in, which closes the field.
const REPORT_MEMORY_UNIT: &str = " MB";
/// The member a structured log record names its own kind under, and the kind that carries the
/// run report ([`REPORT_MAX_MEMORY_MEMBER`] lives in its `metrics`).
const RECORD_TYPE_MEMBER: &str = "type";
/// See [`RECORD_TYPE_MEMBER`].
const REPORT_RECORD_TYPE: &str = "platform.report";
/// See [`RECORD_TYPE_MEMBER`].
const RECORD_MEMBER: &str = "record";
/// See [`RECORD_TYPE_MEMBER`].
const METRICS_MEMBER: &str = "metrics";
/// The character a structured log record begins with: a log event that does not is not one, so
/// it is never handed to the JSON reader.
const RECORD_OPENING: char = '{';

/// The analysed Lambda function versions of one account at the endpoint of `region`, by the
/// resource part of their ARN — the order the Lambda store holds them in, so a page boundary
/// is stable across calls.
///
/// A function version is an analysed resource once the account is enrolled and the analysis
/// its enrollment started is due ([`Enrollment::analysis_visible`], the gate every reader of
/// this service applies) and AWS Lambda holds that version in this endpoint's Region. What the
/// analysis then FINDS for it depends on what was observed: a version the look-back window
/// holds too few invocations or no memory observation for is reported `Unavailable` — "Compute
/// Optimizer was unable to generate a recommendation for the function" — which the
/// per-function reader withholds unless a request asks for that finding by name, rather than
/// sized from an observation this service does not hold.
///
/// Three bounded passes, each under ONE store's lock: the inventory is snapshotted from the
/// Lambda store, the run reports are read from the log store a group at a time, and the
/// published series from the metric store. The ceilings
/// [`MAX_ANALYSED_FUNCTION_VERSIONS`], [`MAX_SCANNED_LOG_EVENTS`],
/// [`MAX_ANALYSED_RUN_REPORTS`] and [`MAX_ANALYSED_SAMPLES`] bound every one of them.
pub fn analysed_functions<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &ComAmazonawsComputeoptimizerComputeoptimizerserviceState,
    region: &str,
    account_id: &str,
    now: f64,
) -> Vec<ObservedFunction> {
    // The gate every recommendation reader applies, ahead of any inventory read: an account
    // that never opted in, one that opted out and one that has just opted in hold no analysis
    // at all, whatever their inventory holds.
    if !store.enrollment.analysis_visible() {
        return Vec::new();
    }
    // The inventory holds the resources of the ONE account this emulator presents
    // ([`engine::emulator_account_id`]), so an account the request named that is not it — a
    // member account of the organization this service records — owns no function here and has
    // no inventory swept on its behalf.
    if account_id != ::engine::emulator_account_id() {
        return Vec::new();
    }
    // An emulator that does not hold the Lambda inventory, or the metric store the analysis
    // reads, has no function to analyse — the same answer as an account that holds none —
    // rather than a finding computed from an input nothing provided.
    let (Some(inventory), Some(observations)) = (
        lambda::state::service_state(engine),
        cloudwatch::state::service_state(engine),
    ) else {
        return Vec::new();
    };
    // The longest window any look-back preference can ask for: the observations kept here are
    // the ones some preference could analyse, and the preference in force for a given read
    // narrows them further ([`ObservedFunction::analysed_samples`]).
    let oldest = now - longest_look_back_days() * SECONDS_PER_DAY;

    // Pass one, under the Lambda store's lock alone: WHICH function versions this endpoint
    // analyses, and what each is configured with. The lock is released before any other store
    // is read.
    let candidates = analysed_candidates(&inventory.lock(), region);

    // Pass two, under the log store's lock alone: the run reports of each function's log
    // group, read ONCE per group and grouped by the version its stream names, so no version
    // re-walks a group another version already walked. A function whose reports this service
    // cannot read is a function whose memory utilization was not observed — the `Inconclusive`
    // case the analysis already reports — not a function the account does not hold.
    let mut reported_memory: BTreeMap<String, BTreeMap<String, Vec<MemoryReport>>> =
        BTreeMap::new();
    if let Some(log_store) = logs::state::service_state(engine) {
        let log_store = log_store.lock();
        for candidate in &candidates {
            if reported_memory.contains_key(&candidate.log_group) {
                continue;
            }
            reported_memory.insert(
                candidate.log_group.clone(),
                run_reports_in(&log_store, &candidate.log_group, oldest, now),
            );
        }
    }

    // Pass three, under the metric store's lock alone: what was published for each version.
    let observations = observations.lock();
    candidates
        .into_iter()
        .map(|candidate| {
            let memory_reports = reported_memory
                .get(&candidate.log_group)
                .and_then(|by_version| by_version.get(&candidate.function_version))
                .cloned()
                .unwrap_or_default();
            let samples = observed_samples(
                &observations,
                &candidate.function_name,
                &candidate.function_version,
                oldest,
                now,
            );
            ObservedFunction {
                function_name: candidate.function_name,
                function_version: candidate.function_version,
                account_id: account_id.to_string(),
                configured_memory_mb: candidate.configured_memory_mb,
                configured_timeout_seconds: candidate.configured_timeout_seconds,
                tags: candidate.tags,
                samples,
                memory_reports,
            }
        })
        .collect()
}

/// One function version this endpoint analyses, as the Lambda inventory describes it: the
/// parts of the record the analysis reads, taken while that store's lock is held so the lock
/// is not held while anything else is read.
struct AnalysedCandidate {
    /// The function's name, which with the version below is its identity.
    function_name: String,
    /// The version analysed: the unpublished one, or a published version number.
    function_version: String,
    /// "The amount of memory, in MB, that's allocated to the current function" — the memory
    /// THAT version is configured with.
    configured_memory_mb: i32,
    /// The `Timeout` that version is configured with, in seconds: how long one of its
    /// invocations may run.
    configured_timeout_seconds: i32,
    /// The function's tags, as AWS Lambda holds them.
    tags: Vec<(String, String)>,
    /// The log group the function's run reports are delivered to.
    log_group: String,
}

/// The function versions of `region` this analysis builds a row for, in the order the Lambda
/// store holds them, up to [`MAX_ANALYSED_FUNCTION_VERSIONS`].
///
/// "If you specify an unqualified ARN without a function version suffix, Compute Optimizer
/// will return recommendations for the latest ($LATEST) version of the function. If you
/// specify a qualified ARN with a version suffix, Compute Optimizer will return
/// recommendations for the specified function version": a function VERSION is the analysed
/// resource, so the unpublished version and every version the function has published are each
/// one of them, each with the memory IT is configured with.
fn analysed_candidates(
    inventory: &lambda::state::ComAmazonawsLambdaAwsgirapiserviceState,
    region: &str,
) -> Vec<AnalysedCandidate> {
    let mut candidates: Vec<AnalysedCandidate> = Vec::new();
    for record in inventory.functions.values() {
        if record.region != region {
            continue;
        }
        let tags: Vec<(String, String)> = record
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let log_group = lambda::state::configured_log_group(
            record.config.logging_config.as_ref(),
            &record.config.name,
        );
        let versions = ::std::iter::once((
            LATEST_FUNCTION_VERSION.to_string(),
            record.config.memory_size,
            record.config.timeout,
        ))
        .chain(record.versions.values().map(|published| {
            (
                published.version.to_string(),
                published.config.memory_size,
                published.config.timeout,
            )
        }));
        for (function_version, configured_memory_mb, configured_timeout_seconds) in versions {
            // The ceiling of the sweep: whatever the inventory holds, one analysis builds at
            // most this many rows, so the work behind a read is a number this service states.
            if candidates.len() >= MAX_ANALYSED_FUNCTION_VERSIONS {
                return candidates;
            }
            candidates.push(AnalysedCandidate {
                function_name: record.config.name.clone(),
                function_version,
                configured_memory_mb,
                configured_timeout_seconds,
                tags: tags.clone(),
                log_group: log_group.clone(),
            });
        }
    }
    candidates
}

/// The dimension set AWS Lambda publishes one function VERSION's series under: the function,
/// and the resource an invocation of that version addressed
/// (`lambda::state::metrics::invoked_resource` — the function itself for the unpublished
/// version, `name:version` for a published one, the same distinction the unqualified and
/// `:`-suffixed forms of a function ARN draw).
fn version_dimensions(
    function_name: &str,
    function_version: &str,
) -> cloudwatch::state::DimensionSet {
    lambda::state::metrics::resource_dimensions(
        function_name,
        &lambda::state::metrics::invoked_resource(function_name, function_version),
    )
}

/// What AWS Lambda published for one function version over the window `oldest..now`, as the
/// samples a recommendation is computed from: one sample per interval CloudWatch aggregates
/// the series in ([`cloudwatch::state::StoredDatum::aggregated_instant`]), oldest first and at
/// most [`MAX_ANALYSED_SAMPLES`] of them.
///
/// One sample holds both series' observations for that interval, so the invocations a sample
/// counts and the durations it reports are the ones observed AT THE SAME TIME. The
/// `Invocations` series is a COUNT per datapoint, so an interval's invocations are the sum
/// over it; the `Duration` series is milliseconds per invocation, so an interval's shortest,
/// mean and longest invocation are that span's minimum, mean and maximum.
fn observed_samples(
    observations: &cloudwatch::state::ComAmazonawsCloudwatchGraniteserviceversion20100801State,
    function_name: &str,
    function_version: &str,
    oldest: f64,
    now: f64,
) -> Vec<FunctionSample> {
    let dimensions = version_dimensions(function_name, function_version);
    let series = |metric_name: &str| cloudwatch::state::MetricKey {
        namespace: LAMBDA_METRIC_NAMESPACE.to_string(),
        metric_name: metric_name.to_string(),
        dimensions: dimensions.clone(),
    };
    let mut intervals: BTreeMap<i64, Interval> = BTreeMap::new();
    for (metric_name, counts_invocations) in [
        (LAMBDA_INVOCATIONS_METRIC, true),
        (LAMBDA_DURATION_METRIC, false),
    ] {
        let Some(data) = observations.metrics.get(&series(metric_name)) else {
            continue;
        };
        for datum in data {
            // A datapoint whose aggregates are not finite describes no utilization, so it is
            // left out of the fold exactly as one outside the window is — the same rule the
            // other analyses of this service fold a series by.
            if !datum.timestamp_secs.is_finite()
                || !datum.sum.is_finite()
                || !datum.minimum.is_finite()
                || !datum.maximum.is_finite()
                || !datum.sample_count.is_finite()
            {
                continue;
            }
            let at = datum.aggregated_instant();
            if at < oldest || at > now {
                continue;
            }
            let at = at as i64;
            // The ceiling of the metric side of the sweep: a series a caller published
            // without limit folds into at most this many intervals.
            if !intervals.contains_key(&at) && intervals.len() >= MAX_ANALYSED_SAMPLES {
                continue;
            }
            let interval = intervals.entry(at).or_insert_with(|| Interval {
                days_ago: ((now - at as f64) / SECONDS_PER_DAY).max(0.0),
                ..Interval::default()
            });
            if counts_invocations {
                interval.invocations += datum.sum.max(0.0);
                continue;
            }
            interval.duration_sum += datum.sum.max(0.0);
            interval.duration_count += datum.sample_count.max(0.0);
            interval.duration_minimum = interval.duration_minimum.min(datum.minimum.max(0.0));
            interval.duration_maximum = interval.duration_maximum.max(datum.maximum.max(0.0));
        }
    }
    intervals.into_values().map(Interval::into_sample).collect()
}

/// One interval of a function version's published series, while the fold is still running:
/// the counts and spans observed in it so far.
#[derive(Debug, Clone, Copy)]
struct Interval {
    /// How long before now the interval was observed.
    days_ago: f64,
    /// How many invocations the `Invocations` series counted in it.
    invocations: f64,
    /// The total and the number of the durations the `Duration` series carried in it, which
    /// is what the interval's MEAN invocation is computed from.
    duration_sum: f64,
    /// See [`Interval::duration_sum`].
    duration_count: f64,
    /// The shortest and the longest invocation the interval observed.
    duration_minimum: f64,
    /// See [`Interval::duration_minimum`].
    duration_maximum: f64,
}

impl Default for Interval {
    /// An interval nothing has been folded into yet: no invocation, no duration, and a
    /// shortest-invocation accumulator above every span a datapoint can carry, so the first
    /// one folded in becomes it.
    fn default() -> Self {
        Self {
            days_ago: 0.0,
            invocations: 0.0,
            duration_sum: 0.0,
            duration_count: 0.0,
            duration_minimum: f64::INFINITY,
            duration_maximum: 0.0,
        }
    }
}

impl Interval {
    /// The sample this interval's fold came to.
    fn into_sample(self) -> FunctionSample {
        FunctionSample {
            days_ago: self.days_ago,
            invocations: self.invocations.min(i64::MAX as f64).round() as i64,
            duration_ms_minimum: if self.duration_minimum.is_finite() {
                self.duration_minimum
            } else {
                0.0
            },
            duration_ms_average: if self.duration_count > 0.0 {
                self.duration_sum / self.duration_count
            } else {
                0.0
            },
            duration_ms_maximum: self.duration_maximum,
        }
    }
}

/// The run reports one function log group holds over the window `oldest..now`, by the function
/// VERSION that produced them.
///
/// A function's reports are delivered to the log group it is configured with
/// (`lambda::state::configured_log_group` — the one reading of `LoggingConfig$LogGroup` and of
/// the default group a function that names none logs to), in the log stream of the execution
/// environment that served the run, whose name states the VERSION that ran it between square
/// brackets ([`stream_version`]). So ONE pass over the group's streams yields every version's
/// reports at once, which is what keeps a published version's observation apart from the
/// unpublished one's without re-reading the group per version.
///
/// Bounded twice: at most [`MAX_SCANNED_LOG_EVENTS`] events of the group are examined and at
/// most [`MAX_ANALYSED_RUN_REPORTS`] reports are kept per version, so neither the streams nor
/// the events a caller wrote can size this pass. An event that is not a run report is skipped
/// on a cheap test before anything parses it ([`reported_memory_mb`]).
fn run_reports_in(
    reports: &logs::state::ComAmazonawsCloudwatchlogsLogs20140328State,
    group_name: &str,
    oldest: f64,
    now: f64,
) -> BTreeMap<String, Vec<MemoryReport>> {
    let mut by_version: BTreeMap<String, Vec<MemoryReport>> = BTreeMap::new();
    let Some(group) = reports.log_groups.get(group_name) else {
        return by_version;
    };
    let mut examined = 0_usize;
    for (stream_name, stream) in &group.streams {
        let Some(version) = stream_version(stream_name) else {
            continue;
        };
        for event in &stream.events {
            if examined >= MAX_SCANNED_LOG_EVENTS {
                return by_version;
            }
            examined += 1;
            // A log event's time stamp is milliseconds and this window is seconds, so the two
            // observations of one run are placed on the same clock.
            let at = event.timestamp as f64 / MILLISECONDS_PER_SECOND;
            if at < oldest || at > now {
                continue;
            }
            let Some(used_mb) = reported_memory_mb(&event.message) else {
                continue;
            };
            let observed = by_version.entry(version.to_string()).or_default();
            if observed.len() >= MAX_ANALYSED_RUN_REPORTS {
                continue;
            }
            observed.push(MemoryReport {
                days_ago: ((now - at) / SECONDS_PER_DAY).max(0.0),
                used_mb,
            });
        }
    }
    by_version
}

/// The function version a log stream's name states, or `None` for a name that states none.
///
/// AWS Lambda names a function's log streams
/// `YYYY/MM/DD/[<version>]<execution environment>`, so the version the environment serves is
/// the text between the brackets.
fn stream_version(stream_name: &str) -> Option<&str> {
    let (_, after) = stream_name.split_once('[')?;
    let (version, _) = after.split_once(']')?;
    (!version.is_empty()).then_some(version)
}

/// The memory one run report states the invocation used, in MB, or `None` for a log event that
/// is not a run report.
///
/// Both formats a function's logs are delivered in carry the figure: the plain-text `REPORT`
/// line states it as its [`REPORT_MAX_MEMORY_FIELD`] field, and the structured
/// `platform.report` record as the [`REPORT_MAX_MEMORY_MEMBER`] metric of its record. A figure
/// that is not a non-negative number is no observation, so it is read as none.
///
/// An event is tested for the marker of each form — a substring search over bytes — BEFORE
/// anything parses it, so an event that is not a run report costs that test and nothing more,
/// and only a structured report is ever handed to the JSON reader.
fn reported_memory_mb(message: &str) -> Option<f64> {
    if message.starts_with(RECORD_OPENING) {
        return message
            .contains(REPORT_RECORD_TYPE)
            .then(|| structured_memory_mb(message))
            .flatten();
    }
    let (_, after) = message.split_once(REPORT_MAX_MEMORY_FIELD)?;
    let figure = after
        .split_once(REPORT_MEMORY_UNIT)
        .map_or(after, |(figure, _)| figure);
    let used_mb: f64 = figure.trim().parse().ok()?;
    (used_mb.is_finite() && used_mb >= 0.0).then_some(used_mb)
}

/// The same figure out of a STRUCTURED run report, or `None` for a log event that is not one.
fn structured_memory_mb(message: &str) -> Option<f64> {
    let record = ::serde_json::from_str::<::serde_json::Value>(message).ok()?;
    if record.get(RECORD_TYPE_MEMBER)?.as_str()? != REPORT_RECORD_TYPE {
        return None;
    }
    let used_mb = record
        .get(RECORD_MEMBER)?
        .get(METRICS_MEMBER)?
        .get(REPORT_MAX_MEMORY_MEMBER)?
        .as_f64()?;
    (used_mb.is_finite() && used_mb >= 0.0).then_some(used_mb)
}
