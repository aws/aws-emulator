// SPDX-License-Identifier: CC0-1.0

//! The health-check EVALUATOR: the periodic assessment a health check of the
//! account's is put through, and the CloudWatch series that assessment is
//! reported as.
//!
//! Route 53 does not only hold a health check's configuration; it evaluates the
//! thing the configuration describes, over and over, and publishes the aggregate
//! assessment of each health check as the CloudWatch metric
//! [`HEALTH_CHECK_STATUS_METRIC`] in the [`NAMESPACE`] namespace, dimensioned by
//! [`HEALTH_CHECK_ID_DIMENSION`] — `1` while the check is considered healthy and
//! `0` while it is considered unhealthy. That series is how a client watches a
//! health check: a CloudWatch alarm on it is what drives DNS-failover automation,
//! so a health check that publishes nothing leaves every such alarm without data.
//!
//! The datapoints go into the CloudWatch emulator's OWN metric store — the same
//! store `PutMetricData` writes and `GetMetricStatistics` / `GetMetricData` /
//! `ListMetrics` read — reached through the shared engine, so one metric has one
//! home and the publishers cannot drift.
//!
//! # What one probe observes
//!
//! `HealthCheckConfig$Type` documents how each kind of health check is assessed,
//! and [`observe`] follows it:
//!
//! * the endpoint types try to reach an endpoint of the caller's, which this
//!   emulator's checkers do over its own in-process transport
//!   ([`endpoint_probe`]): the endpoints they can reach are the ones this
//!   emulator itself serves, and the criterion the type documents — a
//!   connection, an accepted status, the `SearchString` the answer's body
//!   carries — decides whether the probe PASSES. An endpoint nothing here serves
//!   is one no checker could connect to, so its probe fails;
//! * `CALCULATED` "adds up the number of health checks that Route 53 health
//!   checkers consider to be healthy and compares that number with the value of
//!   `HealthThreshold`" — the children's own current assessments, held right
//!   here;
//! * `CLOUDWATCH_METRIC` reads the alarm it names: "If the state of the alarm is
//!   `OK`, the health check is considered healthy. If the state is `ALARM`, the
//!   health check is considered unhealthy. If CloudWatch doesn't have sufficient
//!   data […] the health check status depends on the setting for
//!   `InsufficientDataHealthStatus`";
//! * `RECOVERY_CONTROL` follows a routing control whose state this service does
//!   not hold, and the model gives it neither `RequestInterval` nor
//!   `FailureThreshold` ("not supported when you specify a value for `Type` of
//!   `RECOVERY_CONTROL`"), so it has no probe schedule and publishes no series
//!   rather than a status nothing assessed.
//!
//! # From observations to the reported status
//!
//! `FailureThreshold` is "the number of consecutive health checks that an
//! endpoint must pass or fail for Amazon Route 53 to change the current status of
//! the endpoint from unhealthy to healthy or vice versa", so the status only
//! moves once that many consecutive observations disagree with it
//! ([`HealthCheckEvaluation::observed`]). A check starts out HEALTHY, which is
//! the status `InsufficientDataHealthStatus` documents for one with no history
//! ("For new health checks that have no last known status, the default status for
//! the health check is healthy"). `Disabled` then overrides the whole assessment
//! ("After you disable a health check, Route 53 considers the status of the health
//! check to always be healthy") and `Inverted` flips what is reported, in that
//! order — which is what the `Disabled` documentation points a caller at when it
//! says to "change the value of `Inverted`" to stop routing traffic to a disabled
//! check's resource.
//!
//! # The clock the series is published on
//!
//! A probe is spaced `RequestInterval` seconds from the one before it, so the
//! first flip of a check pointed at an unreachable endpoint is
//! `FailureThreshold * RequestInterval` into the series — up to five minutes,
//! which no client is going to sit through against an emulator. So the series is
//! published on an ACCELERATED schedule: its probes come due
//! [`ACCELERATED_PROBE_SPACING`] apart until the series reaches the present, and
//! at the configured interval from then on, while every datapoint keeps the
//! `RequestInterval` SPACING the real series has. The run-up a threshold needs
//! therefore lands in the window that ends at the moment the check was created
//! ([`HealthCheckEvaluation::new`]), and the flip is observable about
//! `FailureThreshold * ACCELERATED_PROBE_SPACING` after the create instead of
//! minutes later. Nothing is instant: a caller that reads the series straight
//! after creating the check still sees the healthy run-up and not the flip.
//!
//! What makes that observable without any further Route 53 call is
//! [`drive_health_check_metrics`]: the write that creates or changes a health
//! check publishes what is due at that instant and starts the one bounded
//! in-process task that waits for the next due probe. Nothing here is on a READ
//! path, and nothing it publishes depends on who called: the probes it
//! materialises are the ones the stored due instants have already made due.
//!
//! One KNOWN DIVERGENCE follows from that compression, and it is deliberate:
//! because the run-up is laid out BEFORE the call that created the check, the
//! series carries assessments stamped up to `FailureThreshold * RequestInterval`
//! earlier than the check itself — where the real service has no data at all. A
//! client that windows from the instant of the create, or an alarm whose
//! evaluation range reaches back over it, therefore sees assessments for a span in
//! which Route 53 would have held no health check. The alternative is a series
//! whose first flip is minutes of real time away, which no client's polling budget
//! covers; the spacing, the order and the values are the real ones either way.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

use ::smithy_server_runtime::EmbeddedEmulator;

use super::endpoint_probe::{self, ProbeReport};
use super::{
    ComAmazonawsRoute53Awsdnsv20130401State, StoredHealthCheck, DEFAULT_FAILURE_THRESHOLD,
    DEFAULT_REQUEST_INTERVAL, ENDPOINT_HEALTH_CHECK_TYPES, HEALTH_CHECK_TYPE_CALCULATED,
    HEALTH_CHECK_TYPE_CLOUDWATCH_METRIC,
};
use crate::ops::types::HealthCheckConfig;

/// The namespace Route 53 publishes its health-check metrics into.
pub const NAMESPACE: &str = "AWS/Route53";

/// The metric carrying a health check's aggregate assessment.
pub const HEALTH_CHECK_STATUS_METRIC: &str = "HealthCheckStatus";

/// The dimension that series carries: the id of the health check it is the
/// assessment of.
pub const HEALTH_CHECK_ID_DIMENSION: &str = "HealthCheckId";

/// The value a datapoint carries while the health check is considered healthy.
pub const HEALTHY_VALUE: f64 = 1.0;

/// The value a datapoint carries while the health check is considered unhealthy.
pub const UNHEALTHY_VALUE: f64 = 0.0;

/// The resolution a datapoint of this series is stored at, in seconds, for a
/// check whose probes come a MINUTE or more apart — the regular resolution a
/// metric has unless its publisher asks for the high-resolution one ("This field
/// is optional, if you do not specify it the default of 60 is used",
/// `MetricDatum$StorageResolution`).
pub const REGULAR_STORAGE_RESOLUTION_SECS: i32 = 60;

/// The resolution a SUB-MINUTE check's series is stored at: a series is readable
/// at a period only as fine as the resolution it was stored at ("Data points with
/// a period of less than 60 seconds are available only for custom metrics defined
/// with a StorageResolution of 1"), and a regular-resolution datapoint is
/// aggregated in the minute it was published in. A check probing faster than once
/// a minute therefore publishes the high-resolution series its own cadence needs,
/// so a window of the check's interval holds the one assessment made in it.
pub const HIGH_STORAGE_RESOLUTION_SECS: i32 = 1;

/// Which of the two resolutions the series of a check probing every `interval`
/// carries.
fn storage_resolution_secs(interval: Duration) -> i32 {
    if interval < Duration::from_secs(60) {
        HIGH_STORAGE_RESOLUTION_SECS
    } else {
        REGULAR_STORAGE_RESOLUTION_SECS
    }
}

/// How far apart a health check's probes come DUE while its series is still
/// catching up with the present.
///
/// The emulator's own span, not an AWS one: the real schedule puts
/// `FailureThreshold` probes (up to ten) `RequestInterval` seconds (up to thirty)
/// apart, so a client would have to wait minutes for the first status flip. At
/// this spacing the run-up of a check created with the default threshold of three
/// takes 750 ms, which is long enough that a caller reading the series straight
/// after the create still sees the healthy run-up rather than the flip.
pub const ACCELERATED_PROBE_SPACING: Duration = Duration::from_millis(250);

/// The shortest a checker-clock wake-up waits, so a probe already past its due
/// instant still costs a yield rather than a CPU-bound spin.
const MIN_PROBE_WAIT: Duration = Duration::from_millis(50);

/// The longest a checker-clock wake-up waits, so the clock re-reads the store —
/// which a write may have changed since — at least this often.
const MAX_PROBE_WAIT: Duration = Duration::from_secs(5);

/// How many times one checker clock wakes up before it stands down.
///
/// A backstop, not a schedule: it bounds the work one request can start, and the
/// next write to a health check starts a clock again if none is running.
const MAX_CHECKER_WAKE_UPS: usize = 2_000;

/// What one probe of a health check observed, or `None` when the probe observed
/// nothing it could assess the check by — the case
/// `InsufficientDataHealthStatus`' `LastKnownStatus` describes, where the status
/// stays where it was.
type Observation = Option<bool>;

/// The periodic assessment of ONE health check: where its series has got to, and
/// the status that series is reporting.
///
/// A health check whose type has no probe schedule carries none of this (see the
/// module docs on `RECOVERY_CONTROL`), which is why
/// [`StoredHealthCheck::evaluation`] is an `Option`.
#[derive(Debug, Clone)]
pub struct HealthCheckEvaluation {
    /// The time stamp the next probe's datapoint carries. Advances by the check's
    /// interval per probe, so the series keeps the spacing the real one has.
    next_stamp: SystemTime,
    /// The instant the next probe comes due; see [`ACCELERATED_PROBE_SPACING`].
    next_probe_at: Instant,
    /// The status the series is currently reporting, before `Disabled` and
    /// `Inverted` are applied. A new check starts healthy (see the module docs).
    healthy: bool,
    /// Consecutive failing observations since the last passing one, against
    /// `FailureThreshold`.
    consecutive_failures: u32,
    /// Consecutive passing observations since the last failing one, against
    /// `FailureThreshold`.
    consecutive_successes: u32,
}

impl HealthCheckEvaluation {
    /// The evaluation a health check created at `at` with `config` starts with,
    /// or `None` for a configuration with no probe schedule at all.
    ///
    /// The first probe is due at once, and the series starts the run-up the
    /// threshold needs BEFORE `at`, so the flip the threshold delays lands around
    /// the instant the check was created rather than minutes into the future (see
    /// the module docs).
    pub fn new(config: &HealthCheckConfig, at: SystemTime) -> Option<Self> {
        let interval = series_interval(config)?;
        let run_up = interval.saturating_mul(failure_threshold(config));
        Some(Self {
            next_stamp: at.checked_sub(run_up).unwrap_or(at),
            next_probe_at: Instant::now(),
            healthy: true,
            consecutive_failures: 0,
            consecutive_successes: 0,
        })
    }

    /// Whether the next probe is due at `now`.
    fn is_due(&self, now: Instant) -> bool {
        now >= self.next_probe_at
    }

    /// The status this evaluation has reached, before `Disabled` and `Inverted`
    /// are applied to it ([`StoredHealthCheck::reported_healthy`] applies them).
    pub fn is_healthy(&self) -> bool {
        self.healthy
    }

    /// Bring the next probe forward to NOW, which is what a write that changed
    /// what this health check assesses leaves behind: the checkers assess the
    /// configuration the write stored rather than carrying on reporting an
    /// observation of the one it replaced.
    pub fn reassess_now(&mut self) {
        self.next_probe_at = Instant::now();
    }

    /// Fold one observation into the status, which moves only once `threshold`
    /// consecutive observations have disagreed with it.
    fn observed(&mut self, observation: Observation, threshold: u32) {
        match observation {
            Some(true) => {
                self.consecutive_failures = 0;
                self.consecutive_successes = self.consecutive_successes.saturating_add(1);
                if self.consecutive_successes >= threshold {
                    self.healthy = true;
                }
            }
            Some(false) => {
                self.consecutive_successes = 0;
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                if self.consecutive_failures >= threshold {
                    self.healthy = false;
                }
            }
            // Nothing was assessed, so neither run of consecutive observations
            // continues and the status stays where it was.
            None => {}
        }
    }

    /// Take the time stamp of the probe that is due and schedule the next one.
    fn advance(&mut self, interval: Duration) -> SystemTime {
        let stamp = self.next_stamp;
        self.next_stamp = stamp.checked_add(interval).unwrap_or(stamp);
        // While the series is still behind the present it is published at the
        // accelerated spacing; once it has caught up, the next probe is due when
        // the clock reaches its stamp, which is the configured interval apart.
        // Never longer than one interval: that is the most the schedule can owe,
        // so a clock that has stepped backwards costs one slow probe rather than
        // an unbounded wait.
        let wait = self
            .next_stamp
            .duration_since(SystemTime::now())
            .unwrap_or(ACCELERATED_PROBE_SPACING)
            .min(interval);
        self.next_probe_at = Instant::now()
            .checked_add(wait)
            .unwrap_or_else(Instant::now);
        stamp
    }
}

/// The spacing of a health check's series, or `None` for a type that has no probe
/// schedule.
///
/// The configured `RequestInterval` where the check has one — the store fills the
/// documented default in for every endpoint type — and that same default for the
/// two types that assess something other than an endpoint and so carry no
/// interval of their own.
fn series_interval(config: &HealthCheckConfig) -> Option<Duration> {
    let seconds = match config.r#type.as_str() {
        kind if ENDPOINT_HEALTH_CHECK_TYPES.contains(&kind) => config
            .request_interval
            .unwrap_or(DEFAULT_REQUEST_INTERVAL),
        HEALTH_CHECK_TYPE_CALCULATED | HEALTH_CHECK_TYPE_CLOUDWATCH_METRIC => {
            DEFAULT_REQUEST_INTERVAL
        }
        // `RECOVERY_CONTROL` (and any type a later model adds without a probe
        // cadence) has no schedule to publish.
        _ => return None,
    };
    Some(Duration::from_secs(u64::from(seconds.max(1).unsigned_abs())))
}

/// How many consecutive disagreeing observations move this check's status.
///
/// The configured `FailureThreshold` where the check has one; a type that takes
/// none follows its observation at once, which is a threshold of one.
fn failure_threshold(config: &HealthCheckConfig) -> u32 {
    let configured = match config.r#type.as_str() {
        kind if ENDPOINT_HEALTH_CHECK_TYPES.contains(&kind) => config
            .failure_threshold
            .unwrap_or(DEFAULT_FAILURE_THRESHOLD),
        _ => 1,
    };
    configured.max(1).unsigned_abs()
}

/// What one probe of `config` observes, given what the probe of its endpoint
/// reported (`probe`), the status the account's health checks report (`reported`)
/// and the state of the CloudWatch alarms the alarm-backed checks name
/// (`alarm_states`).
///
/// `None` is "nothing was assessed", which is what an
/// `InsufficientDataHealthStatus` of `LastKnownStatus` asks for: the status the
/// check already reports stays.
fn observe(
    config: &HealthCheckConfig,
    probe: Option<&ProbeReport>,
    reported: &HashMap<String, Option<bool>>,
    alarm_states: &HashMap<String, Option<String>>,
) -> Observation {
    match config.r#type.as_str() {
        // An endpoint of the caller's: what the probe of that endpoint observed
        // ([`endpoint_probe`]), which is the SAME observation the checkers report
        // as this check's `StatusReport` — so the series and the checkers cannot
        // disagree about one endpoint. A round in which no probe was made (a
        // disabled check, a configuration with no endpoint) assessed nothing.
        kind if ENDPOINT_HEALTH_CHECK_TYPES.contains(&kind) => {
            probe.map(|report| report.healthy)
        }
        HEALTH_CHECK_TYPE_CALCULATED => {
            // "Route 53 adds up the number of health checks that Route 53 health
            // checkers consider to be healthy and compares that number with the
            // value of HealthThreshold": a child with no assessment of its own is
            // not one of the healthy ones, and a child the list names twice is
            // still ONE health check, so the healthy ones are counted over the
            // DISTINCT children the configuration names. The list itself is held
            // to its published maximum where it is persisted
            // ([`super::MAX_CHILD_HEALTH_CHECKS`]).
            let healthy_children = config
                .child_health_checks
                .iter()
                .flatten()
                .collect::<::std::collections::BTreeSet<&String>>()
                .into_iter()
                .filter(|child| reported.get(child.as_str()) == Some(&Some(true)))
                .count();
            // "If you specify a number greater than the number of child health
            // checks, Route 53 always considers this health check to be
            // unhealthy. If you specify 0, Route 53 always considers this health
            // check to be healthy."
            let threshold = usize::try_from(config.health_threshold.unwrap_or(0).max(0))
                .unwrap_or(usize::MAX);
            Some(healthy_children >= threshold)
        }
        HEALTH_CHECK_TYPE_CLOUDWATCH_METRIC => {
            let state = config
                .alarm_identifier
                .as_ref()
                .and_then(|identifier| alarm_states.get(identifier.name.as_str()))
                .cloned()
                .flatten();
            match state.as_deref() {
                Some(cloudwatch::state::ALARM_STATE_OK) => Some(true),
                Some(cloudwatch::state::ALARM_STATE_ALARM) => Some(false),
                // `INSUFFICIENT_DATA`, and an alarm the account does not hold at
                // all, are both "CloudWatch doesn't have sufficient data".
                _ => match config.insufficient_data_health_status.as_deref() {
                    Some(INSUFFICIENT_DATA_HEALTHY) => Some(true),
                    Some(INSUFFICIENT_DATA_UNHEALTHY) => Some(false),
                    // `LastKnownStatus`, and a configuration that names no
                    // setting, keep the status the check already reports.
                    _ => None,
                },
            }
        }
        _ => None,
    }
}

/// The `InsufficientDataHealthStatus` that makes an alarm-backed check healthy
/// while CloudWatch has insufficient data
/// ([`crate::ops::types::INSUFFICIENT_DATA_HEALTH_STATUS_VALUES`]).
const INSUFFICIENT_DATA_HEALTHY: &str = "Healthy";

/// The `InsufficientDataHealthStatus` that makes it unhealthy
/// ([`crate::ops::types::INSUFFICIENT_DATA_HEALTH_STATUS_VALUES`]).
const INSUFFICIENT_DATA_UNHEALTHY: &str = "Unhealthy";

/// Publish every probe the clock has already made due, and start the one bounded
/// in-process task that waits for the next one.
///
/// Called by the health-check WRITES — the create that starts a check's
/// evaluation and the update that changes what it assesses — never by a read: the
/// probes this materialises are the ones the stored due instants have already
/// made due, so what a caller sees does not depend on whether anybody called.
///
/// `transport` is the handle this plane probes an endpoint over, which a write is
/// handed with the request it is serving (`OperationContext::callback`) and the
/// clock it starts keeps: every one of them serves this same emulator and this
/// same state, so which is kept cannot change what a probe reaches.
///
/// With no async runtime to spawn onto there is no clock, and the probes due at
/// the moment of the write are then the whole series until the next write — the
/// router always runs on one.
pub async fn drive_health_check_metrics<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    transport: EmbeddedEmulator,
) {
    publish_due_probes(&**engine, &transport).await;
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = claim_checker_clock(&mut state.lock()) else {
        return;
    };
    let engine = ::std::sync::Arc::clone(engine);
    let claimed = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        // `true` only if the wake-up cap is what ended the loop; a clock that ran
        // out of work has already released its claim inside the one critical
        // section that observed it.
        let mut capped = true;
        for _ in 0..MAX_CHECKER_WAKE_UPS {
            let Some(wait) = next_probe_wait_or_release(&mut state.lock(), generation) else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what
            // keeps a due probe from costing a CPU-bound spin, and it hands the
            // runtime back between wake-ups.
            ::tokio::time::sleep(wait).await;
            publish_due_probes(&*engine, &transport).await;
        }
        if capped {
            // Only THIS clock's claim, so a clock started since keeps its own.
            release_checker_clock(&mut state.lock(), generation);
        }
    });
    set_checker_clock(&mut claimed.lock(), generation, handle);
}

/// Keep the running clock's task handle, aborting the one it replaces — and only
/// while `generation` is still the CURRENT claim, so a starter whose own task has
/// already stood down installs nothing and aborts nothing.
///
/// The handle is held rather than dropped so the task this request started stays
/// stoppable.
fn set_checker_clock(
    store: &mut ComAmazonawsRoute53Awsdnsv20130401State,
    generation: u64,
    handle: ::tokio::task::JoinHandle<()>,
) {
    if store.checker_clock_generation != generation {
        handle.abort();
        return;
    }
    if let Some(previous) = store.checker_clock.replace(handle) {
        previous.abort();
    }
}

/// Claim the single checker clock for this store: the claim's GENERATION when the
/// caller may start one, `None` when one already runs.
fn claim_checker_clock(store: &mut ComAmazonawsRoute53Awsdnsv20130401State) -> Option<u64> {
    if store.checker_clock_running {
        return None;
    }
    store.checker_clock_running = true;
    store.checker_clock_generation = store.checker_clock_generation.wrapping_add(1);
    Some(store.checker_clock_generation)
}

/// Release the claim `generation` took — and ONLY that one, so a clock that ended
/// on the wake-up cap cannot clear a claim a clock started since is holding.
fn release_checker_clock(store: &mut ComAmazonawsRoute53Awsdnsv20130401State, generation: u64) {
    if store.checker_clock_generation == generation {
        store.checker_clock_running = false;
    }
}

/// How long the checker clock waits before its next pass, or `None` — having
/// RELEASED its claim — when the store holds no health check to evaluate.
///
/// Observing "nothing is owed" and standing down happen in ONE critical section,
/// which is what makes the hand-over race-free: a write holds this same lock to
/// store the health check BEFORE it tries to claim the clock, so either this call
/// sees that check and the running clock keeps it, or the write's claim finds the
/// flag cleared and starts a clock of its own.
fn next_probe_wait_or_release(
    store: &mut ComAmazonawsRoute53Awsdnsv20130401State,
    generation: u64,
) -> Option<Duration> {
    let now = Instant::now();
    let earliest = store
        .health_checks
        .values()
        .filter_map(|check| check.evaluation.as_ref())
        .map(|evaluation| evaluation.next_probe_at)
        .min();
    match earliest {
        Some(due) => Some(
            due.saturating_duration_since(now)
                .clamp(MIN_PROBE_WAIT, MAX_PROBE_WAIT),
        ),
        None => {
            release_checker_clock(store, generation);
            None
        }
    }
}

/// Assess every health check whose next probe is due and publish the datapoint
/// each assessment is reported as.
///
/// Three steps, so no store guard is ever held over work a caller's configuration
/// sizes, over a call into another service, or while that service's own store is
/// locked:
///
/// 1. under THIS service's guard, snapshot what the assessments read — which
///    checks are due, the configuration each is assessed by, and the status every
///    check of the account reports;
/// 2. with NO guard held, PROBE the endpoint each due endpoint-type check names
///    ([`endpoint_probe`]), read the alarm states the alarm-backed checks name
///    from CloudWatch's store, and compute every assessment;
/// 3. under this service's guard again, record each endpoint check's probe as the
///    observation its checkers report, fold each assessment into its check and
///    take the datapoint it is published as — arithmetic per check, with dueness
///    re-checked so two passes cannot publish one probe twice — and append the
///    datapoints once that guard is gone.
async fn publish_due_probes<Eng: Send + Sync + 'static>(
    engine: &Eng,
    transport: &EmbeddedEmulator,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let now = Instant::now();
    // The statuses a CALCULATED check adds up are the ones its children report
    // NOW, taken before any of this pass's probes move them, so the order the
    // store happens to hold two checks in does not decide what the parent
    // observed.
    let (due, reported) = {
        let store = state.lock();
        let due: Vec<(String, HealthCheckConfig)> = due_checks(&store, now)
            .into_iter()
            .map(|check| (check.id.clone(), check.config.clone()))
            .collect();
        if due.is_empty() {
            return;
        }
        let reported: HashMap<String, Option<bool>> = store
            .health_checks
            .iter()
            .map(|(id, check)| (id.clone(), check.reported_healthy()))
            .collect();
        (due, reported)
    };
    // The probes of this round, made with no guard held and BOUNDED as a whole by
    // [`PROBE_ROUND_BUDGET`], so neither the number of health checks an account holds nor an
    // endpoint that answers slowly decides how long the write that drove this round takes. A
    // DISABLED check is not probed at all — "Route 53 stops submitting requests to your
    // application, server, or other resource" — so no request is sent for it.
    let mut probes: HashMap<String, ProbeReport> = ::tokio::time::timeout(
        PROBE_ROUND_BUDGET,
        probe_round(transport, &due),
    )
    .await
    .unwrap_or_default();
    let alarm_names: Vec<String> = due
        .iter()
        .filter_map(|(_, config)| config.alarm_identifier.as_ref())
        .map(|identifier| identifier.name.clone())
        .collect();
    let alarm_states = alarm_states(engine, &alarm_names);
    let assessed: Vec<(String, Observation)> = due
        .into_iter()
        .map(|(id, config)| {
            let observation = observe(&config, probes.get(&id), &reported, &alarm_states);
            (id, observation)
        })
        .collect();

    let published: Vec<(String, SystemTime, f64, i32)> = {
        let mut store = state.lock();
        let mut published = Vec::with_capacity(assessed.len());
        for (id, observation) in assessed {
            let Some(check) = store.health_checks.get_mut(&id) else {
                continue;
            };
            let threshold = failure_threshold(&check.config);
            let Some(interval) = series_interval(&check.config) else {
                continue;
            };
            let disabled = check.config.disabled == Some(true);
            let probe = probes.remove(&id);
            let probed_nothing = disabled
                || !ENDPOINT_HEALTH_CHECK_TYPES.contains(&check.config.r#type.as_str());
            let Some(evaluation) = check.evaluation.as_mut() else {
                continue;
            };
            // Another pass has already taken this probe: its due instant has
            // moved on, so it is not published twice.
            if !evaluation.is_due(now) {
                continue;
            }
            // A disabled check is not probed at all ("Route 53 stops submitting
            // requests to your application, server, or other resource"), so no
            // observation is folded into it while it is off.
            if !disabled {
                evaluation.observed(observation, threshold);
            }
            let stamp = evaluation.advance(interval);
            // What this round's probe observed is what the checkers report for it
            // ([`StoredHealthCheck::reported_status`]). A check nothing probes has
            // nothing to report; one whose probe this round did not get to keeps
            // the observation its checkers last made, rather than losing it to a
            // round that ran out of budget.
            if let Some(report) = probe {
                check.last_probe = Some(report);
            } else if probed_nothing {
                check.last_probe = None;
            }
            let healthy = check.reported_healthy().unwrap_or(true);
            published.push((
                id,
                stamp,
                if healthy { HEALTHY_VALUE } else { UNHEALTHY_VALUE },
                // The series carries one assessment per probe, so it is stored at
                // the resolution this check's own cadence can be read at.
                storage_resolution_secs(interval),
            ));
        }
        published
    };
    append_datapoints(engine, &published);
}

/// Probe the endpoints of the due checks in `due`, answering what each probe observed.
///
/// Called under the round budget its caller holds it to, so the work one round makes is
/// bounded whatever the account holds. A disabled check is not probed ("Route 53 stops
/// submitting requests to your application, server, or other resource"), and a type with no
/// endpoint has nothing to probe.
///
/// Call it with no store guard held: each probe awaits an answer from whichever service of
/// this emulator serves the endpoint.
async fn probe_round(
    transport: &EmbeddedEmulator,
    due: &[(String, HealthCheckConfig)],
) -> HashMap<String, ProbeReport> {
    let mut probes: HashMap<String, ProbeReport> = HashMap::new();
    for (id, config) in due {
        if config.disabled == Some(true) {
            continue;
        }
        if let Some(report) = endpoint_probe::probe(transport, config).await {
            probes.insert(id.clone(), report);
        }
    }
    probes
}

/// The longest ONE round of probing may take in total, whatever its endpoints do and however
/// many of them a round has due at once.
///
/// One probe's own bound ([`endpoint_probe`]'s timeout), applied to the whole round: so a
/// write that drives a round, and the checker clock's own wake-up, are held for no longer
/// than a single probe can take, rather than for a span that grows with the number of health
/// checks an account holds. A check a round ran out of budget for keeps the observation its
/// checkers last made and is probed by the next round, whose due instant has not moved.
const PROBE_ROUND_BUDGET: Duration = Duration::from_secs(5);

/// The health checks whose next probe is due at `now`, in store order.
fn due_checks(
    store: &ComAmazonawsRoute53Awsdnsv20130401State,
    now: Instant,
) -> Vec<&StoredHealthCheck> {
    store
        .health_checks
        .values()
        .filter(|check| {
            check
                .evaluation
                .as_ref()
                .is_some_and(|evaluation| evaluation.is_due(now))
        })
        .collect()
}

/// The current state of each named CloudWatch alarm, read from CloudWatch's own
/// store; an alarm the account does not hold reads as no state at all.
///
/// Empty when CloudWatch is not mounted beside this service, which an alarm-backed
/// check then reads as insufficient data — the same answer it gets for an alarm
/// nobody created.
fn alarm_states<Eng: Send + Sync + 'static>(
    engine: &Eng,
    names: &[String],
) -> HashMap<String, Option<String>> {
    if names.is_empty() {
        return HashMap::new();
    }
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return HashMap::new();
    };
    let store = state.lock();
    names
        .iter()
        .map(|name| (name.clone(), store.alarm_state_value(name)))
        .collect()
}

/// Append one datapoint per assessed health check to the `AWS/Route53` series the
/// check's id dimensions.
///
/// Nothing is published when the CloudWatch store is not mounted beside this
/// service: a datapoint has one home, and this path never makes a second one.
fn append_datapoints<Eng: Send + Sync + 'static>(
    engine: &Eng,
    published: &[(String, SystemTime, f64, i32)],
) {
    if published.is_empty() {
        return;
    }
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    for (id, stamp, value, storage_resolution) in published {
        let key = cloudwatch::state::MetricKey {
            namespace: NAMESPACE.to_string(),
            metric_name: HEALTH_CHECK_STATUS_METRIC.to_string(),
            dimensions: [(HEALTH_CHECK_ID_DIMENSION.to_string(), id.clone())]
                .into_iter()
                .collect(),
        };
        let datum = cloudwatch::state::StoredDatum {
            // CloudWatch holds a datapoint's instant as epoch SECONDS, which is
            // the unit its own readers window by.
            timestamp_secs: epoch_secs(*stamp),
            // One observation: this probe's own assessment of the check.
            sample_count: 1.0,
            sum: *value,
            minimum: *value,
            maximum: *value,
            // The series carries a status rather than a quantity, so there is no
            // unit to declare for it.
            unit: None,
            storage_resolution: *storage_resolution,
        };
        if store.append_datapoints(key, vec![datum]).is_err() {
            // The metric store holds no more series. The health check itself was
            // already created, so nothing is failed for it; the dropped
            // observation is reported where a developer can see it.
            ::tracing::warn!(
                health_check_id = id.as_str(),
                "the CloudWatch metric store is at capacity, so this health-check \
                 assessment was dropped"
            );
        }
    }
}

/// `at` as the epoch seconds a stored datapoint carries.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An endpoint check with the interval and threshold the store fills in.
    fn endpoint_check(threshold: i32, interval: i32) -> HealthCheckConfig {
        HealthCheckConfig {
            r#type: "TCP".to_string(),
            ip_address: Some("192.0.2.10".to_string()),
            port: Some(443),
            request_interval: Some(interval),
            failure_threshold: Some(threshold),
            ..HealthCheckConfig::default()
        }
    }

    /// The series an unreachable endpoint check publishes: the status stays
    /// healthy for the probes the threshold allows, then flips and stays flipped.
    #[test]
    fn the_status_flips_on_the_threshold_probe() {
        // (case, threshold, how many of the first probes must report healthy)
        let rows: Vec<(&str, i32, usize)> = vec![
            ("the smallest published threshold", 1, 0),
            ("the documented default", 3, 2),
            ("the recorded threshold", 5, 4),
            ("the largest published threshold", 10, 9),
        ];
        for (case, threshold, healthy_probes) in rows {
            let config = endpoint_check(threshold, 30);
            let mut evaluation = HealthCheckEvaluation::new(&config, SystemTime::now())
                .unwrap_or_else(|| panic!("case {case:?}: an endpoint check has a schedule"));
            let mut values: Vec<f64> = Vec::new();
            for _ in 0..12 {
                evaluation.observed(Some(false), failure_threshold(&config));
                values.push(if evaluation.healthy {
                    HEALTHY_VALUE
                } else {
                    UNHEALTHY_VALUE
                });
            }
            let healthy_run = values.iter().take_while(|value| **value == HEALTHY_VALUE).count();
            assert_eq!(
                healthy_run, healthy_probes,
                "case {case:?}: expected {healthy_probes} healthy probes before the flip, got \
                 {values:?}"
            );
            assert!(
                values[healthy_probes..].iter().all(|value| *value == UNHEALTHY_VALUE),
                "case {case:?}: the status must stay flipped, got {values:?}"
            );
        }
    }

    /// The series' stamps are the configured interval apart, and the run-up ends
    /// at the instant the check was created, so the flip is inside the window a
    /// client reads.
    #[test]
    fn the_run_up_ends_at_the_instant_the_check_was_created() {
        // (case, threshold, interval)
        let rows: Vec<(&str, i32, i32)> = vec![
            ("the fastest published interval", 3, 10),
            ("the recorded configuration", 5, 30),
            ("the largest threshold at the fastest interval", 10, 10),
        ];
        for (case, threshold, interval) in rows {
            let config = endpoint_check(threshold, interval);
            let created = SystemTime::now();
            let mut evaluation = HealthCheckEvaluation::new(&config, created)
                .unwrap_or_else(|| panic!("case {case:?}: an endpoint check has a schedule"));
            let spacing = Duration::from_secs(u64::from(interval.unsigned_abs()));
            let mut stamps: Vec<SystemTime> = Vec::new();
            for _ in 0..=threshold {
                stamps.push(evaluation.advance(spacing));
            }
            let first = stamps[0];
            assert_eq!(
                created.duration_since(first).ok(),
                Some(spacing.saturating_mul(threshold.unsigned_abs())),
                "case {case:?}: expected the run-up the threshold needs before the create, got \
                 {stamps:?}"
            );
            for pair in stamps.windows(2) {
                assert_eq!(
                    pair[1].duration_since(pair[0]).ok(),
                    Some(spacing),
                    "case {case:?}: expected stamps one interval apart, got {stamps:?}"
                );
            }
            let flip = stamps[usize::try_from(threshold).expect("a published threshold is small")];
            assert!(
                flip <= created + spacing,
                "case {case:?}: the flip must land in the window around the create, got \
                 {stamps:?}"
            );
        }
    }

    /// A type with no probe cadence has no series, and the two that assess
    /// something other than an endpoint follow their observation at once.
    #[test]
    fn a_type_without_a_probe_cadence_publishes_no_series() {
        // (case, type, whether it has a schedule, the threshold it debounces by)
        let rows: Vec<(&str, &str, bool, u32)> = vec![
            ("an endpoint type", "HTTPS", true, 3),
            ("a calculated check", HEALTH_CHECK_TYPE_CALCULATED, true, 1),
            (
                "an alarm-backed check",
                HEALTH_CHECK_TYPE_CLOUDWATCH_METRIC,
                true,
                1,
            ),
            ("a routing-control follower", "RECOVERY_CONTROL", false, 1),
        ];
        for (case, kind, has_schedule, threshold) in rows {
            let config = HealthCheckConfig {
                r#type: kind.to_string(),
                ..HealthCheckConfig::default()
            };
            let config = super::super::health_check_config_with_defaults(&config);
            assert_eq!(
                series_interval(&config).is_some(),
                has_schedule,
                "case {case:?}: expected a schedule: {has_schedule}, got {:?}",
                series_interval(&config)
            );
            assert_eq!(
                failure_threshold(&config),
                threshold,
                "case {case:?}: expected a threshold of {threshold}, got {}",
                failure_threshold(&config)
            );
        }
    }
}
