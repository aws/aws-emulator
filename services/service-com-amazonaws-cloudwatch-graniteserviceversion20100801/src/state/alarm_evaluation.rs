// SPDX-License-Identifier: CC0-1.0

//! A METRIC alarm's `StateValue`, COMPUTED from the datapoints `PutMetricData` stored.
//!
//! A metric alarm's state is not a value a caller writes: it is the alarm's own comparison
//! ("The arithmetic operation to use when comparing the specified statistic and threshold",
//! `MetricAlarm$ComparisonOperator`) applied to the statistic of the metric over its
//! evaluation periods, so it is derived here from the series the metric store holds — the
//! same way a composite alarm's state is derived from its `AlarmRule`
//! ([`super::ComAmazonawsCloudwatchGraniteserviceversion20100801State::evaluate_composite_alarms`]).
//!
//! The rule, as `PutMetricAlarm` and the CloudWatch alarm documentation state it:
//!
//! * the metric is aggregated into periods of `Period` seconds each, ANCHORED on the grid the
//!   alarm's own evaluation window lays — "CloudWatch aggregates all data points with time
//!   stamps that fall within each one-hour period" — so which period a datapoint belongs to is
//!   decided by its own time stamp against that grid and not by where in a period the evaluation
//!   happens to run, while WHICH of those periods an evaluation grades is decided by its
//!   evaluated range — the range of the window the alarm's `EvaluationWindow` names
//!   ([`EvaluationGrid`]): a rolling one that "advances each time the alarm is evaluated"
//!   (`SlidingWindow`), which is also the window an alarm naming none uses and whose periods are
//!   counted back from the end that range has advanced to, or one aligned to the clock
//!   boundaries the alarm's period repeats at (`WallClockWindow`) — and either way only the
//!   periods that have CLOSED are graded, the period the clock is in still being published into;
//! * each window yields one datapoint — the alarm's `Statistic` or `ExtendedStatistic` over
//!   the datapoints in it — or no datapoint at all when the window holds none;
//! * the evaluation looks over a RANGE of windows wider than `EvaluationPeriods`, and
//!   evaluates the `EvaluationPeriods` most recent datapoints it really found there:
//!   CloudWatch "attempts to retrieve a higher number of data points than the number
//!   specified as Evaluation Periods" and "all real data points in the evaluation range are
//!   included in the evaluation", filling what is still missing with the missing-data
//!   treatment "only as few times as possible" — and how many it attempts "depends on the
//!   length of the alarm period and whether it is based on a metric with standard resolution or
//!   high resolution", so the range reaches back over a SPAN of clock rather than over a
//!   multiple of the period alone ([`MINIMUM_EVALUATION_RANGE_SPAN_SECS`]): an alarm whose
//!   newest non-missing datapoint still breaches keeps `ALARM` while a client's publishing is
//!   paused, instead of being moved by the absence of new data;
//! * a datapoint BREACHES when the comparison between it and `Threshold` holds;
//! * an alarm whose statistic is a PERCENTILE may be configured to withhold its state while a
//!   window is too thinly sampled for that percentile to mean anything — "If you specify
//!   `ignore`, the alarm state does not change during periods with too few data points to be
//!   statistically significant" (`MetricAlarm$EvaluateLowSampleCountPercentile`) — so such a
//!   window's value is not graded at all and no missing-data arm stands in for it, and an
//!   evaluation whose every real datapoint was withheld that way computes no state
//!   ([`low_sample_floor`]);
//! * a slot no real datapoint filled counts as whatever `TreatMissingData` says ("Valid
//!   Values: breaching | notBreaching | ignore | missing", default `missing`) — `breaching`
//!   and `notBreaching` substitute a datapoint for it, `missing` leaves it out of the
//!   evaluation altogether, and `ignore` leaves it out too but, when NO datapoint at all
//!   could be evaluated, keeps the alarm in the state it has ("the current alarm state is
//!   maintained");
//! * the predicate an "M out of N" alarm is decided by has TWO directions, each with a minimum
//!   of its own built from the alarm's configuration alone: the alarm is in `ALARM` once at
//!   least `DatapointsToAlarm` of the evaluated datapoints breach ("The number of data points
//!   that must be breaching to trigger the alarm" — the M, defaulting to `EvaluationPeriods`,
//!   which is the N), and in `OK` once at least `EvaluationPeriods - DatapointsToAlarm + 1` of
//!   them do not — the count that makes M breaching datapoints unreachable inside the same
//!   window, which is the minimum CloudWatch's own explanation of an `OK` state reports;
//! * a window neither direction's minimum is reached over is one the evaluation cannot decide
//!   — only a treatment that substitutes NO datapoint (`missing`, `ignore`) can leave it that
//!   short of datapoints — so the alarm is in `INSUFFICIENT_DATA`, or keeps the state it has
//!   where the treatment is `ignore` ("the current alarm state is maintained");
//! * the `StateReason` ("An explanation for the alarm state, in text format.") explains how
//!   the state was decided: it NAMES the datapoints the evaluation graded — the value the
//!   statistic took over each window and the instant that window ends — beside the counts it
//!   made over them, and a window that held NO datapoint of the metric is reported as the hole
//!   it was, naming the missing-data arm that was applied to it, rather than being counted as a
//!   datapoint the evaluation graded — a client can tell an alarm that saw clean data from one
//!   that saw none. The count it reports is of the datapoints that satisfied the DIRECTION the
//!   state stands for, and its closing clause names that direction and the minimum it needed,
//!   so both integers of the alarm's configuration are recoverable from the sentence.
//!
//! The evaluation is a pure function of the stored datapoints and the clock, and it writes
//! only when the state it computes differs from the state the alarm already reports — so
//! whichever call materialises it, every later caller sees the same thing. The first
//! evaluation of an alarm becomes due [`EVALUATION_DELAY_SECS`] after the write that
//! configured it ("When this operation creates an alarm, the alarm state is immediately set
//! to INSUFFICIENT_DATA. The alarm is then evaluated and its state is set appropriately." —
//! `PutMetricAlarm`), and a state `SetAlarmState` forced stands until that alarm's next
//! evaluation, one of its own periods away ([`forced_state_span_secs`]) or sooner if a
//! datapoint of the series it watches arrives before then.
//!
//! What is NOT evaluated here keeps the state it has, rather than being answered with a
//! state nothing computed: an alarm with `EvaluationCriteria` (a PromQL query), one whose
//! configuration names no stored metric at all, and one over a `Metrics` query array this
//! engine cannot resolve to a single time series.
//!
//! An alarm configured with a `Metrics` query array IS evaluated: "Use the `Metrics` parameter
//! to create an alarm based on a metric math expression" (`PutMetricAlarmInput$Metrics`), and
//! the series it grades is the value of the query that array returns — the EXPRESSION's output,
//! computed over the same stored datapoints `GetMetricData` answers the same array from
//! ([`super::metric_query`]) — against the same threshold, the same m-of-n predicate and the
//! same explanation as an alarm over one metric.
//!
//! An alarm whose threshold is a MODEL rather than a number is evaluated by the same predicate
//! over a threshold that moves with the series: "In an alarm based on an anomaly detection
//! model, this is the ID of the ANOMALY_DETECTION_BAND function used as the threshold for the
//! alarm" (`MetricAlarm$ThresholdMetricId`), so the designated query of the array is resolved
//! to the pair of bounds that function yields and each graded window is compared against the
//! bound its own period carries ([`Predicate`]) — the upper bound, the lower bound, or both,
//! as the alarm's comparison names. A window the model says nothing about yet is not graded at
//! all, the same way an under-sampled percentile window is not: the alarm keeps the state it
//! has rather than being moved by a threshold nothing computed.

use ::serde_json::{Map, Value};

use super::alarm_actions::AlarmNotification;
use super::metric_math::MathValue;
use super::{
    dimension_set, to_timestamp, weighted_percentile, AlarmStateSnapshot, BasicStatistics,
    ComAmazonawsCloudwatchGraniteserviceversion20100801State as State, MetricKey, StoredDatum,
    ALARM_STATE_ALARM, ALARM_STATE_INSUFFICIENT_DATA, ALARM_STATE_OK, ALARM_TYPE_METRIC,
    HISTORY_DATA_VERSION, MAX_STATE_REASON_DATA_LEN,
};
use crate::ops::types::{
    AlarmHistoryItem, MetricAlarm, MetricDataQuery, STATISTIC_VALUES,
};

/// The grid of period windows an alarm's evaluation grades its series over: where the
/// boundaries between those periods fall, and whether the period the clock is IN may be graded
/// before it closes — the whole of what an alarm's `EvaluationWindow` decides.
pub mod evaluation_window;

pub use evaluation_window::{AlignmentZone, EvaluationGrid};

/// The WARM-UP window an alarm is created or updated with: when it ends, and what it withholds
/// while it is in force.
pub mod warm_up;

/// How long after the write that configured an alarm its FIRST evaluation is owed, and how
/// long after an evaluation the NEXT one is owed.
///
/// `PutMetricAlarm` creates an alarm in `INSUFFICIENT_DATA` and documents that "the alarm is
/// then evaluated and its state is set appropriately", so the evaluated state is not due at
/// the instant of the write: a caller reading straight after the write still sees the state
/// the write left behind. CloudWatch publishes no duration for that gap, so this span is the
/// emulator's own choice inside the framework's 250-750 ms window for a transition the clock
/// owes, and it is never slept on — the due instant is stored and the evaluation is
/// materialised by the first call at or after it. It is the same span, for the same reason,
/// as [`super::COMPOSITE_EVALUATION_DELAY_SECS`].
///
/// An evaluation RE-ARMS the alarm by the same span, so an alarm just evaluated is not folded
/// again by every following request: a state its datapoints already imply becomes visible to
/// the next caller a span later — the same window a clock-owed transition takes — and the fold
/// one alarm costs is charged at most once per span rather than once per request.
pub const EVALUATION_DELAY_SECS: f64 = 0.5;

/// The most period windows ONE alarm's LOOK-BACK covers.
///
/// A capacity bound, not an AWS quota: `EvaluationPeriods` is modelled with a minimum only,
/// so without this one alarm could ask one call to fold arbitrarily many windows. CloudWatch's
/// own published ceiling on an alarm's look-back is that `EvaluationPeriods` x `Period` may
/// not exceed one day, which at the shortest regular period (60 s) is exactly this many
/// windows. An alarm asking for more is evaluated over its most recent windows rather than
/// refused, because the windows past the bound are older than any datapoint CloudWatch would
/// evaluate.
const MAX_EVALUATED_PERIODS: usize = 1_440;

/// How much wider than the look-back the EVALUATION RANGE is: the span of periods an
/// evaluation searches for the datapoints it evaluates.
///
/// CloudWatch "attempts to retrieve a higher number of data points than the number specified
/// as Evaluation Periods" and calls that span the evaluation range, so a datapoint just older
/// than the look-back is still evaluated rather than counted as missing — which is why a
/// series published a little slower than the alarm's period still decides its state. How much
/// wider the range is depends on the period and the resolution and is not published as a
/// number, so this factor is the emulator's own bounded choice: wide enough that a sparse
/// series is evaluated on what it really published, and a fixed multiple of the look-back so
/// the periods one evaluation folds stay bounded by it. Where the periods are finer than the
/// granularity the series is STORED at, or finer than the shortest regular period, the width
/// this factor gives is not enough on its own and the range covers one storage span, or one
/// [`MINIMUM_EVALUATION_RANGE_SPAN_SECS`], instead ([`evaluation_range_periods`]) — each
/// bounded too.
///
/// The periods are anchored on the grid the alarm's window lays and the newest of them is the one
/// the end of the rolling range falls in ([`EvaluationGrid::newest_graded_period`]), so the range
/// reaches further back than a span measured from the evaluation instant would, which is why this
/// factor leaves room for more periods than the look-back plus the sparse-series margin.
const EVALUATION_RANGE_FACTOR: usize = 3;

/// The shortest `Period` an alarm over a REGULAR-resolution metric is evaluated on: "Valid values
/// are 10, 20, 30, and any multiple of 60" (`PutMetricAlarmInput$Period`), of which 60 is the
/// shortest that is not a high-resolution period — "if you specify a period of 10, 20, or 30 for an
/// alarm on a high-resolution metric, you create a high-resolution alarm".
const SHORTEST_REGULAR_PERIOD_SECS: f64 = 60.0;

/// The span of clock the EVALUATION RANGE reaches back over at the least, whatever the alarm's
/// period, in seconds.
///
/// How many datapoints CloudWatch attempts to retrieve "depends on the length of the alarm period
/// and whether it is based on a metric with standard resolution or high resolution", so the range
/// is not a multiple of the period alone: a FINER period does not give an alarm a shorter memory
/// of the series it watches. This is the width in TIME that an alarm at the shortest regular
/// period gets from the factor above ([`EVALUATION_RANGE_FACTOR`] x
/// [`SHORTEST_REGULAR_PERIOD_SECS`]), and an alarm whose periods are finer than that period
/// reaches back over the same span in more, shorter windows ([`evaluation_range_periods`]).
///
/// What it decides is what happens when a client STOPS publishing: the most recent real datapoint
/// of the series keeps deciding the alarm for as long as it is inside the range ("all real data
/// points in the evaluation range are included in the evaluation"), so an alarm whose newest
/// non-missing datapoint still breaches holds `ALARM` until data that contradicts it arrives,
/// instead of being moved by the ABSENCE of new data one look-back after the last publish. The
/// range stays bounded, so a series nothing publishes into for long enough is eventually decided
/// by the missing-data treatment, as CloudWatch decides it.
const MINIMUM_EVALUATION_RANGE_SPAN_SECS: f64 =
    EVALUATION_RANGE_FACTOR as f64 * SHORTEST_REGULAR_PERIOD_SECS;

/// The most action notifications the store holds waiting for delivery.
///
/// A capacity bound: the notifications live in this process's memory, so their number has to
/// be finite. Reaching it drops the OLDEST — a notification nothing has delivered yet is the
/// stale one — and says so in the log, which is the honest answer for a delivery that did
/// not happen.
pub const MAX_PENDING_NOTIFICATIONS: usize = 256;

/// The `TreatMissingData` value that counts a missing datapoint as breaching.
const MISSING_BREACHING: &str = "breaching";

/// The `TreatMissingData` value that counts a missing datapoint as not breaching.
const MISSING_NOT_BREACHING: &str = "notBreaching";

/// The `TreatMissingData` value under which an evaluation that cannot decide the alarm leaves
/// it in the state it has: "the current alarm state is maintained". A window the datapoints
/// present DO decide still decides it.
const MISSING_IGNORE: &str = "ignore";

/// The `TreatMissingData` default: "If `TreatMissingData` is omitted, the default behavior of
/// `missing` is used" (`PutMetricAlarmInput$TreatMissingData`) — a missing datapoint is
/// neither breaching nor not breaching, so the alarm is evaluated on the datapoints it has and
/// reports insufficient data where they are too few to reach either direction's minimum.
const MISSING_MISSING: &str = "missing";

/// The most alarms ONE request may evaluate.
///
/// A per-request bound, not an AWS quota: the store may hold [`super::MAX_ALARMS`] alarms and
/// an evaluation pass visits the ones that are due, so without this one small request's work
/// would grow with everything the account has ever created. It sits far above what a settled
/// store asks of one request — an alarm is re-armed for [`EVALUATION_DELAY_SECS`] once
/// evaluated, so a request pays only for the alarms that came due in that span — and an
/// evaluation this bound stops is not answered wrongly, only settled later: it stays DUE and
/// the next call carries it, because the evaluation is a pure function of the stored
/// datapoints and the clock.
const MAX_ALARMS_EVALUATED_PER_REQUEST: usize = 256;

/// The most stored datapoints the folds of ONE request may visit.
///
/// The other factor of the same product: each alarm's evaluation folds the series it watches,
/// which holds up to [`super::MAX_DATAPOINTS_PER_SERIES`] datapoints, so bounding the number
/// of alarms alone would leave `alarms x datapoints` unbounded. The charge is made before a
/// series is folded and saturates, so one request folds at most this many datapoints plus the
/// one series that exhausted the budget.
const MAX_DATAPOINTS_FOLDED_PER_REQUEST: usize = 200_000;

/// How long ONE request may spend materialising the evaluations the clock has made due.
///
/// The wall-clock half of the budget, so the work is bounded in TIME as well as in counts,
/// however cheap or expensive each charged unit turns out to be. Short enough to stay a small
/// part of any request's own latency; what it does not reach stays due for the next call. It
/// bounds what a pass does AFTER its first evaluation, never whether that first one runs
/// ([`EvaluationBudget::spent`]), because a span this short can be gone before a pass has
/// begun — the wait for the store's lock, and the scheduler, are not under the pass's control.
const EVALUATION_DEADLINE: std::time::Duration = std::time::Duration::from_millis(50);

/// The work ONE request may spend materialising the alarm evaluations the clock has already
/// made due: alarms visited, datapoints folded, and wall clock.
///
/// ONE budget covers a whole evaluation STAGE — the metric-alarm pass and the composite-alarm
/// pass that follows it — so the two cannot each spend a full one, and every charge is made
/// BEFORE the work it pays for. A handler that must evaluate before its own write charges one
/// for that stage and the settle that follows the write charges one more, so a request's
/// evaluation work is bounded by a small fixed number of these budgets rather than by what
/// the store holds. An evaluation a budget stops is left due for the next call rather than
/// answered from nothing — which is only true while every pass the budget bounds settles one
/// of them, so each pass is owed its first evaluation before the bounds answer at all
/// ([`EvaluationBudget::start_pass`], [`EvaluationBudget::spent`]).
#[derive(Debug)]
pub struct EvaluationBudget {
    /// How many more alarms this request may evaluate.
    alarms: usize,
    /// How many more stored datapoints this request's folds may visit.
    datapoints: usize,
    /// When this request stops evaluating, whatever is left of the counts above.
    deadline: std::time::Instant,
    /// Whether the pass this budget is bounding has settled an evaluation yet.
    settled_in_pass: bool,
}

impl Default for EvaluationBudget {
    fn default() -> Self {
        Self::for_one_request()
    }
}

impl EvaluationBudget {
    /// The budget ONE request starts with.
    #[must_use]
    pub fn for_one_request() -> Self {
        Self {
            alarms: MAX_ALARMS_EVALUATED_PER_REQUEST,
            datapoints: MAX_DATAPOINTS_FOLDED_PER_REQUEST,
            deadline: std::time::Instant::now() + EVALUATION_DEADLINE,
            settled_in_pass: false,
        }
    }

    /// Begin one PASS over the alarms this budget bounds. The pass is owed its first
    /// evaluation: until that one is settled, this budget is not spent ([`Self::spent`]).
    pub(super) fn start_pass(&mut self) {
        self.settled_in_pass = false;
    }

    /// Whether this request has spent its budget and must stop evaluating.
    ///
    /// A bound on the work one request may do DEFERS the evaluations it stops: they stay due,
    /// and the next call carries them. That holds only while every pass makes progress — a
    /// pass stopped before its first evaluation defers nothing, it starves it, and a pass can
    /// be stopped that early by a clock it does not control, since the time the wait for the
    /// store's lock and the scheduler took before the pass began is time off this budget. Were
    /// one pass after another stopped that way, an evaluation the clock has made due would
    /// never be materialised at all and the alarm would keep the state its last write left
    /// behind for as long as the load lasted. So the bounds answer from a pass's SECOND
    /// evaluation onwards, and one request pays for at most one evaluation per pass beyond
    /// them — each still bounded on its own by what one alarm's fold may visit.
    #[must_use]
    pub fn spent(&self) -> bool {
        self.settled_in_pass
            && (self.alarms == 0
                || self.datapoints == 0
                || std::time::Instant::now() >= self.deadline)
    }

    /// Charge one alarm's evaluation.
    pub(super) fn charge_alarm(&mut self) {
        self.alarms = self.alarms.saturating_sub(1);
    }

    /// Record that this pass has settled one alarm's evaluation, whatever that evaluation
    /// computed — from here on the pass is bounded again. Recorded AFTER the evaluation rather
    /// than before it, so the one evaluation a pass is owed is a whole one.
    pub(super) fn settled_one(&mut self) {
        self.settled_in_pass = true;
    }

    /// Charge the datapoints one fold is about to visit. Saturating: the fold that exhausts
    /// the budget still runs — it is bounded on its own by
    /// [`super::MAX_DATAPOINTS_PER_SERIES`] — and no further alarm is evaluated by this
    /// request.
    fn charge_datapoints(&mut self, datapoints: usize) {
        self.datapoints = self.datapoints.saturating_sub(datapoints);
    }
}

/// How long a state `SetAlarmState` forces on a metric alarm STANDS before the alarm's own
/// evaluation takes it back.
///
/// "Metric alarms returns to their actual state quickly, often within seconds. Because the
/// metric alarm state change happens quickly, it is typically only visible in the alarm's
/// History tab […] or through DescribeAlarmHistory" (`SetAlarmState`) is the only thing
/// CloudWatch states about the latency, and it is SECONDS — which is also what the recorded
/// behaviour of a force pins it at from both sides: a caller's own reads right after the call
/// still report the state it forced, and the state is nevertheless taken back with no further
/// call.
///
/// What takes it back is the alarm's own next evaluation, which is one of that alarm's PERIODS
/// away ("the length, in seconds, used each time the metric specified in MetricName is
/// evaluated", `PutMetricAlarmInput$Period`) — so the span is the alarm's period, capped here.
/// The CAP is this emulator's own choice: one minute, the cadence a standard-resolution series
/// is evaluated at and the shortest period `Period` admits above its sub-minute values, so an
/// alarm of any longer period holds a forced state for that minute rather than for its own
/// hours. It is what a CLIENT DRIVING A SCENARIO through forced states needs: a forced state is
/// an input to every alarm whose rule READS this one ("The alarms specified in a composite
/// alarm's rule expression can include metric alarms", `PutCompositeAlarm`), so it has to
/// outlive the client's own round trips — the watch it keeps on the parent to see that the rule
/// does NOT hold yet, and the force it then puts on the next child — or no client could ever
/// observe the rule coming to hold. It is bounded all the same, so a client WAITING for the
/// alarm to come back to its actual state gets there inside one session.
const FORCED_STATE_SECS: f64 = 60.0;

/// How one evaluated window's datapoint stands against the alarm's threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Datapoint {
    /// The comparison between the datapoint and the threshold held.
    Breaching,
    /// The window had a datapoint and the comparison did not hold.
    NotBreaching,
    /// The window held no datapoint of this metric.
    Missing,
}

/// The series ONE metric alarm watches: namespace, metric name and normalised dimension set,
/// the identity its evaluation reads and the one a datapoint is stored under.
///
/// The one place an alarm's configuration is turned into a series identity, so the evaluation,
/// the forced-state span and [`watched_series_set`] can never resolve it three ways.
pub(super) fn watched_series(alarm: &MetricAlarm) -> MetricKey {
    MetricKey {
        namespace: alarm.namespace.clone().unwrap_or_default(),
        metric_name: alarm.metric_name.clone().unwrap_or_default(),
        dimensions: dimension_set(alarm.dimensions.as_deref()),
    }
}

/// EVERY series one alarm's evaluation reads, each beside the `Unit` it reads that series
/// under.
///
/// An alarm over one metric reads the series its own members name, under its own `Unit`; an
/// alarm configured with a query array reads the series each of that array's `MetricStat`
/// queries names, each under that query's own `Unit`. It is what [`State::make_alarms_due`]
/// matches an arriving datapoint against, resolved from the same members the evaluation itself
/// reads the store by.
pub(super) fn watched_series_set(alarm: &MetricAlarm) -> Vec<(MetricKey, Option<String>)> {
    let Some(queries) = alarm.metrics.as_deref() else {
        return vec![(watched_series(alarm), alarm.unit.clone())];
    };
    queries
        .iter()
        .filter_map(|query| query.metric_stat.as_ref())
        .filter_map(|stat| {
            super::metric_query::query_series_key(stat).map(|key| (key, stat.unit.clone()))
        })
        .collect()
}

/// The identity of the ONE time series an alarm's evaluation GRADES, or `None` for an alarm
/// whose configuration names no series of its own.
///
/// Both forms a configuration identifies that series in resolve here, and they resolve to the
/// same identity the evaluation reads the store by: the alarm's own metric members
/// ([`watched_series`]), and the `MetricStat` of the one query of a `Metrics` array the
/// evaluation watches ([`watched_query`], [`super::metric_query::query_series_key`]). An alarm
/// graded on an expression computed from several series, or on a query of another language,
/// names no single series: the thing it grades is computed rather than stored.
#[must_use]
pub(crate) fn graded_series_key(alarm: &MetricAlarm) -> Option<MetricKey> {
    match alarm.metrics.as_deref() {
        Some(queries) => super::metric_query::query_series_key(
            watched_query(queries, alarm.threshold_metric_id.as_deref())?
                .metric_stat
                .as_ref()?,
        ),
        None => {
            let key = watched_series(alarm);
            // "a metric is uniquely defined by a name, a namespace, and zero or more
            // dimensions", so members missing either half name no series at all.
            (!key.namespace.is_empty() && !key.metric_name.is_empty()).then_some(key)
        }
    }
}

/// The width of the windows an alarm's query ARRAY is graded in.
///
/// "The granularity, in seconds, of the returned data points" (`MetricDataQuery$Period`) when
/// the returning query names one, and the `MetricStat$Period` when that query IS a metric —
/// the granularity the shared resolution folds that query at
/// ([`super::metric_query::query_period`]). Otherwise the period of the metrics the expression
/// reads — the widest of them, the granularity every one of those series has a value at —
/// because the period of an expression is the one of the metrics it is computed from.
fn graded_period(queries: &[MetricDataQuery], graded: &MetricDataQuery) -> f64 {
    if graded.period.is_some() || graded.metric_stat.is_some() {
        return super::metric_query::query_period(graded);
    }
    let widest = queries
        .iter()
        .filter(|query| query.period.is_some() || query.metric_stat.is_some())
        .map(super::metric_query::query_period)
        .fold(0.0_f64, f64::max);
    if widest >= 1.0 {
        widest
    } else {
        // An array that reads no metric at all computes its series on the default expression
        // grid, which is the granularity it is then graded in.
        super::metric_query::DEFAULT_EXPRESSION_PERIOD
    }
}

/// The width of the windows ONE alarm's series is graded in, in seconds, or `None` for an
/// alarm whose configuration names none.
///
/// The same width the grading itself folds that series at, read from the same members: the
/// alarm's own `Period` for an alarm over one metric ([`State::graded_metric`]), and the period
/// of the query a `Metrics` array returns for one configured with such an array
/// ([`graded_period`]). It is what sizes the evaluation range
/// ([`evaluation_range_periods`]), which has to be decided before the series is folded.
fn graded_window_secs(alarm: &MetricAlarm) -> Option<f64> {
    match alarm.metrics.as_deref() {
        Some(queries) => {
            let graded = watched_query(queries, alarm.threshold_metric_id.as_deref())?;
            Some(graded_period(queries, graded))
        }
        None => alarm_period_secs(alarm),
    }
}

/// How long it takes this alarm to reach a VERDICT about data published now, in seconds, or
/// `None` for an alarm whose configuration names no period or no evaluation count — one whose
/// series nothing here folds at all.
///
/// Two spans of the alarm's own configuration, as the evaluation applies them: a datapoint is
/// graded only once the window it falls in has CLOSED, the window the clock is in still being
/// published into ([`EvaluationGrid::newest_graded_period`]), and the verdict is taken over the
/// alarm's `EvaluationPeriods` — "the number of periods over which data is compared to the
/// specified threshold" — windows ([`evaluated_periods`]). So the span from a value being
/// published to the alarm being able to report what that value implies is the look-back plus
/// the window that value lands in: `Period` x (`EvaluationPeriods` + 1).
///
/// Read by the components that have to WAIT for an alarm to speak about something they are
/// watching, rather than by the evaluation itself.
#[must_use]
pub fn verdict_span_secs(alarm: &MetricAlarm) -> Option<f64> {
    let window = graded_window_secs(alarm)?;
    #[allow(clippy::cast_precision_loss)]
    let periods = evaluated_periods(alarm)? as f64;
    Some(window * (periods + 1.0))
}

/// The width of the windows an alarm over ONE metric is evaluated on: its own `Period` in
/// seconds, or `None` for an alarm naming no period a series can be folded over at all.
///
/// One reader, so the fold and the range that sizes it cannot drift on what counts as a usable
/// `Period`.
fn alarm_period_secs(alarm: &MetricAlarm) -> Option<f64> {
    alarm.period.filter(|period| *period >= 1).map(f64::from)
}

/// The metrics the BAND of one alarm is drawn from: the metrics the query its
/// `ThresholdMetricId` designates reads, followed through the ids that query's expression
/// refers to ("A math expression can use the `Id` of the other metrics or queries to refer to
/// those metrics", `MetricDataQuery$Expression`). Empty for an alarm whose threshold is a
/// number.
///
/// The walk visits each id of the array at most once — the ids an expression may refer to are
/// the array's own, so a chain of references cannot be longer than the array and one that goes
/// in a circle is left after its first visit.
fn band_model_metrics(alarm: &MetricAlarm) -> Vec<super::metric_math::SeriesMetric> {
    let mut metrics: Vec<super::metric_math::SeriesMetric> = Vec::new();
    let (Some(threshold_id), Some(queries)) = (
        alarm.threshold_metric_id.as_deref(),
        alarm.metrics.as_deref(),
    ) else {
        return metrics;
    };
    let mut pending: Vec<String> = vec![threshold_id.to_string()];
    let mut visited: Vec<String> = Vec::with_capacity(queries.len());
    while let Some(id) = pending.pop() {
        if visited.contains(&id) {
            continue;
        }
        visited.push(id.clone());
        let Some(query) = queries.iter().find(|query| query.id == id) else {
            continue;
        };
        if let Some(stat) = query.metric_stat.as_ref() {
            // A model is the DETECTOR's, and a detector is created for one metric folded by
            // one statistic, which is the identity this query names
            // ([`super::metric_query::query_series_key`]).
            if let Some(key) = super::metric_query::query_series_key(stat) {
                metrics.push(super::metric_math::SeriesMetric {
                    namespace: key.namespace,
                    metric_name: key.metric_name,
                    dimensions: key.dimensions,
                    stat: stat.stat.clone(),
                });
            }
            continue;
        }
        if let Some(tree) = query
            .expression
            .as_deref()
            .and_then(|expression| super::metric_math::parse(expression).ok())
        {
            pending.extend(super::metric_math::references(&tree));
        }
    }
    metrics
}

/// Whether `alarm`'s evaluation grades a VECTOR of time series rather than one.
///
/// The model names the two configurations whose result is a vector. A PromQL criteria's "query
/// must return a result of vector type. Each entry in the vector result represents an alarm
/// contributor." (`AlarmPromQLCriteria$Query`), and a Metrics Insights query that groups its
/// result returns one series per group ("This field can contain either a Metrics Insights query,
/// or a metric math expression", `MetricDataQuery$Expression`,
/// [`super::metric_math::groups_a_metrics_insights_query`]) — so an alarm configured with either
/// is graded on several series at once.
///
/// Every other configuration grades ONE series, whatever identifies it: the single metric the
/// alarm's own members name, the one query of a `Metrics` array the evaluation watches
/// ([`watched_query`]), an expression folded from several series, a composite alarm's rule over
/// other alarms, or a log alarm's scheduled query.
#[must_use]
pub fn grades_a_vector_of_series(alarm: &MetricAlarm) -> bool {
    if alarm.evaluation_criteria.is_some() {
        return true;
    }
    // Only the query the evaluation WATCHES decides it: a grouped query feeding an expression
    // that folds its groups to one series is graded as that one series.
    watched_query(
        alarm.metrics.as_deref().unwrap_or_default(),
        alarm.threshold_metric_id.as_deref(),
    )
    .and_then(|watched| watched.expression.as_deref())
    .is_some_and(super::metric_math::groups_a_metrics_insights_query)
}

/// Which query of a `Metrics` array the alarm WATCHES: the one returning data that is not the
/// query its threshold comes from.
///
/// "When used in PutMetricAlarm, specify true for the one expression result to use as the
/// alarm." … "If you omit this, the default of true is used." (`MetricDataQuery$ReturnData`) —
/// the query that returns data is the one the alarm grades, whether it is an expression or a
/// metric. The query named by `ThresholdMetricId` is what the watched result is compared
/// AGAINST rather than the result itself, so it is never the watched one however its own
/// `ReturnData` reads.
pub(super) fn watched_query<'array>(
    queries: &'array [MetricDataQuery],
    threshold_metric_id: Option<&str>,
) -> Option<&'array MetricDataQuery> {
    queries.iter().find(|query| {
        query.return_data != Some(false)
            && threshold_metric_id.is_none_or(|designated| designated != query.id)
    })
}

/// The points of ONE resolved series: its `(time stamp, value)` pairs, oldest first.
type SeriesPoints<'resolved> = &'resolved [(f64, f64)];

/// The points of the ONE series the query `id` resolved to.
///
/// "Any expression used in a PutMetricAlarm operation must return a single time series."
/// (`MetricDataQuery`) — a number is no series at all, and an array of several is not the one
/// series an alarm watches.
fn single_series<'resolved>(
    resolution: &'resolved super::metric_query::Resolution,
    id: &str,
) -> Option<SeriesPoints<'resolved>> {
    match resolution.value(id)? {
        MathValue::Series(series) => Some(&series.points),
        MathValue::Array(series) if series.len() == 1 => Some(&series.first()?.points),
        _ => None,
    }
}

/// The LOWER and UPPER bound series of the band the query `id` resolved to, in that order.
///
/// A band is the pair of series `ANOMALY_DETECTION_BAND` yields, each reported under the
/// label the function suffixes ([`super::metric_math::LOWER_BAND_SUFFIX`] /
/// [`super::metric_math::UPPER_BAND_SUFFIX`]), so the bound a value is compared against is
/// read from the label the same reader of the band sees rather than from the position of a
/// series in the array. `None` for a value that is not such a pair: a query the alarm
/// designates as its threshold and that is no band yields no threshold at all.
fn band_bounds<'resolved>(
    resolution: &'resolved super::metric_query::Resolution,
    id: &str,
) -> Option<(SeriesPoints<'resolved>, SeriesPoints<'resolved>)> {
    let MathValue::Array(series) = resolution.value(id)? else {
        return None;
    };
    let bound = |suffix: &str| {
        series
            .iter()
            .find(|series| series.label.ends_with(suffix))
            .map(|series| series.points.as_slice())
    };
    Some((
        bound(super::metric_math::LOWER_BAND_SUFFIX)?,
        bound(super::metric_math::UPPER_BAND_SUFFIX)?,
    ))
}

/// One resolved series folded into the windows an evaluation grades, newest first — index 0
/// being the window the end of the evaluation's range fell in
/// ([`EvaluationGrid::newest_graded_period`]).
///
/// Which window a point belongs to is decided by its own time stamp against the alarm's own
/// grid, the same anchoring an alarm over one metric folds its datapoints by, and a point past
/// the end of this evaluation's range is left to a later evaluation; where a series carries
/// several points inside one window — a series spaced finer than the period it is graded in —
/// the window takes the NEWEST of them.
fn graded_windows(
    points: &[(f64, f64)],
    grid: EvaluationGrid,
    period: f64,
    range_periods: usize,
    now_secs: f64,
) -> Vec<Option<GradedDatapoint>> {
    let mut windows: Vec<Option<(f64, GradedDatapoint)>> = vec![None; range_periods];
    for (timestamp, value) in points {
        // A point the arithmetic produced no finite number for is not a value the alarm can
        // grade, and it is reported as dropped rather than graded as a zero.
        if !value.is_finite() {
            continue;
        }
        let Some(index) = grid.periods_back(now_secs, *timestamp, period) else {
            continue;
        };
        let Some(slot) = windows.get_mut(index) else {
            continue;
        };
        if slot.is_none_or(|(held, _)| *timestamp >= held) {
            *slot = Some((
                *timestamp,
                GradedDatapoint {
                    value: *value,
                    // Nothing measured a sample count for a value an expression computed: the
                    // series is not one metric's observations.
                    sample_count: None,
                },
            ));
        }
    }
    windows
        .into_iter()
        .map(|slot| slot.map(|(_, datapoint)| datapoint))
        .collect()
}

/// How many of the alarm's periods ONE evaluation grades — its `EvaluationPeriods`, within the
/// capacity bound — or `None` for an alarm that names none.
fn evaluated_periods(alarm: &MetricAlarm) -> Option<usize> {
    usize::try_from(alarm.evaluation_periods?)
        .ok()
        .filter(|periods| *periods >= 1)
        .map(|periods| periods.min(MAX_EVALUATED_PERIODS))
}

/// How many period windows the EVALUATION RANGE spans for a look-back of `evaluation_periods`
/// over windows `period` seconds wide, grading a series whose datapoints are stored at a
/// granularity of `stored_span` seconds ([`EVALUATION_RANGE_FACTOR`]).
///
/// The range is deliberately wider than the look-back so that a series published more slowly
/// than the alarm's period is still evaluated on the datapoints it really holds, and how much
/// more slowly a series can be published is not a free choice: a datapoint is aggregated at the
/// granularity its own `StorageResolution` stores it at
/// ([`super::StoredDatum::aggregated_instant`]), so a series stored at regular resolution holds
/// ONE datapoint per minute however often a client publishes into it, and an alarm whose periods
/// are finer than that granularity has at most one real datapoint per storage span of range. A
/// range reaching back LESS far than that span lets a datapoint published late in its span fall
/// out of the range before any evaluation has graded it — nothing would ever grade a breach a
/// client really published — so the range reaches back at least one whole storage span plus the
/// lag the end of the range runs behind this instant ([`EvaluationGrid::lag_secs`]). That is the
/// width at which every stored datapoint is graded by the evaluations running while it is the
/// newest one the series holds.
///
/// The range also reaches back over at least one minimum SPAN of clock
/// ([`MINIMUM_EVALUATION_RANGE_SPAN_SECS`]), in as many of this alarm's windows as that span
/// takes: how far back the retrieval reaches depends on the length of the alarm's period, so an
/// alarm whose periods are finer than the shortest regular one searches more of them rather than a
/// shorter stretch of its series.
///
/// Where the look-back's own multiple is already that wide the width is unchanged, which is
/// every alarm whose period is no finer than the granularity its series is stored at and no finer
/// than the shortest regular period. Each covering term carries the same capacity bound the
/// look-back does ([`MAX_EVALUATED_PERIODS`]), so no term can size one evaluation's fold by more
/// windows than this store ever grades.
fn evaluation_range_periods(
    evaluation_periods: usize,
    period: Option<f64>,
    stored_span: f64,
    grid: EvaluationGrid,
    now_secs: f64,
) -> usize {
    let covering = |span: f64| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let windows = span.ceil() as usize;
        windows.min(MAX_EVALUATED_PERIODS)
    };
    let covering_one_storage_span = period
        .filter(|period| *period >= 1.0 && stored_span > 0.0)
        .map_or(0, |period| {
            covering((stored_span + grid.lag_secs(now_secs, period)) / period)
        });
    let covering_the_minimum_span = period
        .filter(|period| *period >= 1.0)
        .map_or(0, |period| {
            covering(MINIMUM_EVALUATION_RANGE_SPAN_SECS / period)
        });
    evaluation_periods
        .saturating_mul(EVALUATION_RANGE_FACTOR)
        .max(covering_one_storage_span)
        .max(covering_the_minimum_span)
}

/// The names of `alarms` in the order their evaluations came DUE: oldest first, and by name
/// between two owed one at the same instant.
///
/// The one definition of that order, so the metric-alarm pass
/// ([`State::evaluate_metric_alarms`]) and the composite-alarm pass
/// ([`super::ComAmazonawsCloudwatchGraniteserviceversion20100801State::evaluate_composite_alarms`])
/// cannot settle the alarms a request pays for in two different orders. Oldest first is what
/// makes the alarm that has waited longest the one a request whose budget stops it early still
/// settles; the name tie-break is what keeps the pass deterministic.
pub(super) fn due_order<'name>(
    alarms: impl Iterator<Item = (&'name str, f64)>,
) -> Vec<String> {
    let mut ordered: Vec<(&str, f64)> = alarms.collect();
    ordered.sort_by(|(left_name, left_due), (right_name, right_due)| {
        left_due
            .total_cmp(right_due)
            .then_with(|| left_name.cmp(right_name))
    });
    ordered
        .into_iter()
        .map(|(name, _due)| name.to_string())
        .collect()
}

/// Whether `datum` is one the alarm's `Unit` selects ([`alarm_grades_unit`]).
fn carries_alarm_unit(alarm: &MetricAlarm, datum: &StoredDatum) -> bool {
    alarm_grades_unit(alarm, datum.unit())
}

/// Whether a datapoint carrying `unit` is one this alarm GRADES: "If you specify a unit, the
/// operation returns only data that was collected with that unit specified." An alarm naming no
/// unit grades every datapoint of the series it watches.
fn alarm_grades_unit(alarm: &MetricAlarm, unit: &str) -> bool {
    grades_unit(alarm.unit.as_deref(), unit)
}

/// Whether a datapoint carrying `unit` is one a reader selecting `selector` GRADES: "If you
/// specify a unit, the operation returns only data that was collected with that unit
/// specified." A reader naming no unit grades every datapoint of the series.
///
/// `unit` is the unit the datapoint is STORED under ([`StoredDatum::unit`]), which every
/// datapoint has: one whose publisher named none carries the value the enumerated type declares
/// for a measurement with no unit, so an alarm selecting THAT value grades it and an alarm
/// selecting any other does not.
///
/// The one definition of that selector, so the evaluation fold, the forced-state span and
/// [`State::make_alarms_due`] cannot answer "is this data for this alarm?" three ways —
/// whether the unit is the alarm's own or the one its query names.
pub(super) fn grades_unit(selector: Option<&str>, unit: &str) -> bool {
    selector.is_none_or(|wanted| unit == wanted)
}

/// What the series ONE alarm READS hold, as a single pass over them found it.
///
/// Both halves come out of that one pass, over the same series ([`watched_series_set`]), so an
/// evaluation that has to know how far back its range must reach AND whether those series have
/// moved on past the end of it does not walk them twice.
#[derive(Debug, Clone, Copy, Default)]
struct WatchedData {
    /// The COARSEST granularity any datapoint of those series is stored at, in seconds, and
    /// `0.0` for an alarm over series this store holds no datapoint of.
    storage_span: f64,
    /// The newest instant a datapoint this alarm GRADES has been aggregated into, among the
    /// ones already in the past; `None` where those series hold no such datapoint.
    newest_instant: Option<f64>,
}

/// One window of the series an alarm GRADES: the value it took, and how many observations it
/// aggregated where the series is one metric's.
#[derive(Debug, Clone, Copy)]
struct GradedDatapoint {
    /// The value the graded series took over the window.
    value: f64,
    /// How many observations that window aggregated, for a window of one metric's series;
    /// `None` where the value was computed rather than measured.
    sample_count: Option<f64>,
}

/// The series ONE alarm grades, window by window.
struct GradedSeries<'alarm> {
    /// The statistic the series was folded by, for an alarm that names one; `None` for a
    /// series a query array computed, which no single statistic describes.
    statistic: Option<AlarmStatistic<'alarm>>,
    /// The grid those windows lie on: where their boundaries fall, and whether the period the
    /// clock is in was graded at all ([`EvaluationGrid`]). Carried beside them so the
    /// explanation names the instants the SAME grid put them at.
    grid: EvaluationGrid,
    /// How wide one window is, in seconds.
    period: f64,
    /// What each window of the evaluation range took, newest first — index 0 being the window
    /// the end of the evaluation's range fell in ([`EvaluationGrid::newest_graded_period`]) —
    /// and `None` for a window the series has no value for.
    windows: Vec<Option<GradedDatapoint>>,
}

/// What ONE evaluation grades its series against: the alarm's threshold and comparison, how
/// many of its periods the verdict covers, and what it does with a window that has no value.
#[derive(Debug, Clone, Copy)]
struct GradedAgainst<'alarm> {
    /// The threshold and comparison the alarm judges each datapoint by — a number
    /// ("The value to compare with the specified statistic.", `MetricAlarm$Threshold`) or the
    /// band a model draws.
    predicate: &'alarm Predicate,
    /// How many of those periods held a datapoint the alarm WITHHELD for holding too few
    /// observations for its percentile ([`low_sample_floor`]): those slots are filled by
    /// nothing, because the period was published into, so no arm of the missing-data treatment
    /// decided anything for it.
    withheld: usize,
    /// How many of the alarm's periods the verdict is made over — its `EvaluationPeriods`.
    evaluation_periods: usize,
    /// "Sets how this alarm is to handle missing data points."
    /// (`MetricAlarm$TreatMissingData`)
    treat_missing: &'alarm str,
}

/// One datapoint ONE evaluation evaluated.
#[derive(Debug, Clone, Copy)]
struct Evaluated {
    /// Which window of the evaluation range this slot stands for, counting back from the window
    /// the end of the evaluation's range fell in ([`EvaluationGrid::newest_graded_period`]) —
    /// the period a real datapoint came from, and the period a missing-data arm stood in for
    /// where no real datapoint filled it. Every slot stands for ONE period of the look-back, so
    /// every slot can be named by that period's own instant.
    period: usize,
    /// The statistic over that window, for a real datapoint — and `None` for a slot no real
    /// datapoint filled, which is what tells the two apart.
    value: Option<f64>,
    /// How many observations that window aggregated, for a real datapoint.
    sample_count: Option<f64>,
    /// The threshold this datapoint was compared AGAINST, for an alarm whose threshold moves
    /// with the series — the bound of the band its own period carries. `None` where the
    /// threshold is the one number the alarm is configured with, which a record reports once
    /// rather than per datapoint.
    threshold: Option<f64>,
    /// How it stands against the threshold.
    datapoint: Datapoint,
}

impl Evaluated {
    /// Whether a real datapoint of the series filled this slot, as against a missing-data arm
    /// standing in for a period the series held nothing over.
    fn measured(&self) -> bool {
        self.value.is_some()
    }
}

/// What the datapoints a verdict RESTS ON are: how a datapoint that satisfied the direction the
/// state was decided in reads against the threshold, and how many of them that direction needed.
#[derive(Debug, Clone, Copy)]
struct Verdict {
    /// The classification of a datapoint that satisfied the reported direction: breaching for
    /// `ALARM`, non-breaching for `OK`.
    satisfying: Datapoint,
    /// How many of them the direction needed — the alarm's M for `ALARM`, and `N - M + 1` for
    /// `OK`.
    minimum: usize,
}

/// The state one evaluation computed for one alarm, with the explanation it reports.
#[derive(Debug, Clone)]
struct Evaluation {
    /// The computed `StateValue`.
    state: &'static str,
    /// `StateReason`: "The reason that this alarm is set to this specific state, in text
    /// format." (`MetricAlarm$StateReason`)
    reason: String,
    /// `StateReasonData`: "An explanation for the alarm state, in JSON format."
    /// (`MetricAlarm$StateReasonData`)
    reason_data: String,
}

/// The Metrics Insights query one alarm's evaluation reads the store by: the query the alarm
/// WATCHES, when that query is one, and `None` for every other configuration.
///
/// Read through the ONE parser of that language ([`super::metrics_insights::parse`]), so which
/// series an alarm reads, when its evaluation is due and what its contributors are all follow
/// from the same reading of the statement.
pub(super) fn watched_insights_query(
    alarm: &MetricAlarm,
) -> Option<super::metrics_insights::Query> {
    let expression = watched_query(
        alarm.metrics.as_deref()?,
        alarm.threshold_metric_id.as_deref(),
    )?
    .expression
    .as_deref()?;
    if !super::metric_math::is_metrics_insights_query(expression) {
        return None;
    }
    super::metrics_insights::parse(expression).ok()
}

/// How many characters a contributor's identifier carries: the whole of `ContributorId`'s
/// modelled 1..16 length.
const CONTRIBUTOR_ID_LEN: usize = 16;

/// The identifier of the contributor that is ONE series of the alarm `alarm_name`.
///
/// "Represents an individual contributor to a multi-timeseries alarm, containing information
/// about a specific time series" (`AlarmContributor`) — so the identifier names a series OF an
/// alarm: it is a digest of the alarm and the series identity (the metric, its namespace and its
/// normalised label set), cut to the modelled length. Derived, so it is stable across reads and
/// the same contributor keeps the same id for the life of the alarm, and distinct, so two series
/// are two contributors.
///
/// Each part is digested under its own LENGTH rather than under a separator, because a label name
/// or value may carry any character a separator could be: two series whose parts differ can never
/// be digested from the same bytes.
#[must_use]
pub fn contributor_id(alarm_name: &str, series: Option<&MetricKey>) -> String {
    let mut identity = String::new();
    let mut part = |text: &str| {
        identity.push_str(&text.len().to_string());
        identity.push(':');
        identity.push_str(text);
    };
    part(alarm_name);
    if let Some(series) = series {
        part(&series.namespace);
        part(&series.metric_name);
        for (name, value) in &series.dimensions {
            part(name);
            part(value);
        }
    }
    let digest = ::hex::encode(<::sha2::Sha256 as ::sha2::Digest>::digest(
        identity.as_bytes(),
    ));
    digest
        .get(..CONTRIBUTOR_ID_LEN)
        .unwrap_or(digest.as_str())
        .to_string()
}

/// One contributor of a multi-time-series alarm, as one evaluation graded it.
///
/// "Each entry in the vector result represents an alarm contributor" — so a contributor is one
/// entry of the vector the alarm's query returns, graded by the alarm's own predicate exactly as
/// a single series would be.
#[derive(Debug, Clone)]
struct GradedContributor {
    /// The labels that identify the entry: the keys the query grouped by and this entry's values
    /// for them.
    labels: ::std::collections::BTreeMap<String, String>,
    /// The identity of the series this entry is, which its identifier is digested from.
    series: MetricKey,
    /// The state this entry's own grading put it in, with the explanation it reports.
    evaluation: Evaluation,
}

/// The state the CONTRIBUTORS of a multi-time-series alarm put the alarm itself in, with the
/// explanation that entry reports.
///
/// "Returns the information of the current alarm contributors that are in ALARM state."
/// (`DescribeAlarmContributors`) — an alarm whose query returns a vector of series is in `ALARM`
/// while any entry of that vector is, so the alarm's state is the state of the entry that
/// decides it: the first entry in `ALARM` in the order the query ranked them, else the first
/// entry with a decided state, and the explanation is that entry's own.
///
/// `None` for a query whose result has no entry at all — nothing graded anything, so the alarm
/// keeps the state it has.
fn contributor_verdict(graded: &[GradedContributor]) -> Option<Evaluation> {
    let deciding = graded
        .iter()
        .find(|contributor| contributor.evaluation.state == ALARM_STATE_ALARM)
        .or_else(|| {
            graded
                .iter()
                .find(|contributor| contributor.evaluation.state == ALARM_STATE_OK)
        })
        .or_else(|| graded.first())?;
    Some(deciding.evaluation.clone())
}

impl State {
    /// Arm the span the state `SetAlarmState` has just forced on the metric alarm `name` stands
    /// for: its next evaluation is owed at the end of it
    /// ([`Self::forced_state_span_secs`]). A name no metric alarm carries arms nothing.
    pub fn arm_forced_state(&mut self, name: &str, now_secs: f64) {
        // The alarm as it now stands, read before the write: the span is a function of its own
        // configuration.
        let Some(alarm) = self.alarms.get(name).map(|stored| stored.alarm.clone()) else {
            return;
        };
        let due = now_secs + self.forced_state_span_secs(&alarm, now_secs);
        if let Some(stored) = self.alarms.get_mut(name) {
            stored.evaluation_due_secs = due;
            // The instant the force stands until, so nothing brings the evaluation that takes
            // it back forward of it ([`State::make_alarms_due`]).
            stored.forced_state_until_secs = due;
        }
    }

    /// How long a state `SetAlarmState` forces on `alarm` stands before the evaluation takes
    /// the alarm back over: until that alarm's next SCHEDULED evaluation
    /// ([`FORCED_STATE_SECS`]), bounded by the alarm's own period.
    ///
    /// "Temporarily sets the state of an alarm for testing purposes. […] Metric alarms returns
    /// to their actual state quickly, often within seconds." (`SetAlarmState`) — what takes the
    /// alarm back is its next evaluation, and this is the span that evaluation is owed from the
    /// force. The span is never zero, so a caller reading straight after the call still sees the
    /// state it forced; and it does not depend on what the series holds, so it is never shorter
    /// than the span the alarms whose rule READS this one are owed their own evaluation in —
    /// a composite alarm's rule is evaluated over the state the caller forced rather than over
    /// the one a reclaim has already written.
    ///
    /// An alarm whose configuration names no period at all — one evaluated over a `Metrics`
    /// query array or an `EvaluationCriteria` query, where the period belongs to each query —
    /// has no scheduled pass of its own, so the force stands for that same span.
    ///
    /// A composite alarm is not armed by a span but by its children's states, which
    /// `SetAlarmState` snapshots — this is the span of a METRIC alarm's force.
    #[must_use]
    pub fn forced_state_span_secs(&self, alarm: &MetricAlarm, now_secs: f64) -> f64 {
        // The span is a property of the force, not of the series: an alarm over a metric
        // nothing writes and one over a metric being published into keep a forced state for
        // the same time, because what takes either of them back is the evaluation the clock
        // owes rather than the arrival of a datapoint.
        let _ = now_secs;
        alarm
            .period
            .filter(|period| *period >= 1)
            .map_or(FORCED_STATE_SECS, |period| {
                f64::from(period).clamp(EVALUATION_DELAY_SECS, FORCED_STATE_SECS)
            })
    }

    /// Materialise every metric-alarm evaluation the clock has already made due, within what
    /// `budget` lets this request pay for.
    ///
    /// Alarms are visited in the order their evaluations came due ([`due_order`]). Each
    /// alarm's evaluation reads only the metric store and the clock, so the order never
    /// changes the outcome, and an alarm the budget does not reach stays due for the next
    /// call — while the pass settles the first due alarm it finds whatever the budget already
    /// holds ([`EvaluationBudget::start_pass`]), so what it defers is never everything.
    pub fn evaluate_metric_alarms(&mut self, now_secs: f64, budget: &mut EvaluationBudget) {
        budget.start_pass();
        let ordered = due_order(
            self.alarms
                .iter()
                .map(|(name, stored)| (name.as_str(), stored.evaluation_due_secs)),
        );
        for name in ordered {
            if budget.spent() {
                break;
            }
            self.evaluate_metric_alarm(&name, now_secs, budget);
        }
    }

    /// Evaluate ONE metric alarm, writing its new state only when the computed state differs
    /// from the one the alarm reports.
    fn evaluate_metric_alarm(
        &mut self,
        name: &str,
        now_secs: f64,
        budget: &mut EvaluationBudget,
    ) {
        let Some(stored) = self.alarms.get(name) else {
            return;
        };
        if now_secs < stored.evaluation_due_secs {
            return;
        }
        // Everything the evaluation reads is read before anything is written.
        let alarm = stored.alarm.clone();
        budget.charge_alarm();
        // This alarm is being evaluated now, so its NEXT evaluation is owed a span from here
        // rather than being due again at once: it is re-armed before the fold runs, so the
        // fold is charged to this request only and a following request pays for the alarms
        // that came due since.
        if let Some(stored) = self.alarms.get_mut(name) {
            stored.evaluation_due_secs = now_secs + EVALUATION_DELAY_SECS;
        }
        // The anomaly detection model this alarm draws its threshold from is part of what the
        // alarm needs to be evaluated at all, for as long as it is configured that way, so
        // every evaluation ensures it is there: a model deleted while this alarm still draws a
        // band from it is created again here, and a model no alarm's threshold is drawn from is
        // never put back ([`Self::create_band_models`]). An alarm whose threshold is a number
        // names no band, so nothing is created for it.
        self.create_band_models(&alarm, now_secs);
        // An alarm inside its WARM-UP window is not graded at all: "During the warm-up period,
        // the alarm stays in INSUFFICIENT_DATA and does not perform alarm actions."
        // (`WarmUpConfiguration`) — so this evaluation is withheld, whichever form the alarm's
        // configuration is graded in, and the one that finds the window over hands the alarm
        // back to its own grading ([`warm_up`]).
        if self.warm_up_withholds_evaluation(name, now_secs, budget) {
            budget.settled_one();
            return;
        }
        // An alarm whose query returns a VECTOR of series is decided by its CONTRIBUTORS: each
        // entry of that vector is graded on its own ([`Self::graded_contributors`]) and the
        // alarm's own state is the state those entries put it in
        // ([`contributor_verdict`]). `None` for every other configuration, which is graded as
        // the one series it watches.
        let contributors = self.graded_contributors(&alarm, now_secs, budget);
        let evaluation = match contributors.as_deref() {
            Some(graded) => contributor_verdict(graded),
            None => self.metric_alarm_evaluation(&alarm, now_secs, budget),
        };
        // What the contributors stand at NOW is what a read of them reports, whether or not the
        // alarm itself moved: the entries of the vector are the alarm's own current reading of
        // its query.
        if let Some(graded) = contributors.as_deref() {
            self.record_contributors(name, graded, now_secs);
        }
        let Some(evaluation) = evaluation else {
            // This pass has paid for one alarm's evaluation, so what remains of the budget
            // bounds it again — whether the evaluation computed a state or this is an alarm
            // this engine computes nothing for.
            budget.settled_one();
            return;
        };
        budget.settled_one();
        // An evaluation that computes the state the alarm already reports moves it nowhere, so
        // NOTHING about the alarm changes: no history item, no action, no new instant on
        // `StateUpdatedTimestamp` or `StateTransitionedTimestamp` — and not the EXPLANATION
        // either. "An explanation for the alarm state" (`MetricAlarm$StateReason` /
        // `$StateReasonData`, the decision record naming the datapoints that were graded) is the
        // record of the evaluation that PUT the alarm in the state it is in, which is the one
        // the state change announced for it: an evaluation that moved it nowhere decided nothing
        // about that state, so the alarm goes on reporting the record of the evaluation that
        // did, however many times it is graded again meanwhile.
        if alarm.state_value.as_deref() == Some(evaluation.state) {
            return;
        }

        let previous = AlarmStateSnapshot::of_metric_alarm(&alarm);
        let summary = format!(
            "Alarm updated from {} to {}",
            previous
                .state_value
                .as_deref()
                .unwrap_or(ALARM_STATE_INSUFFICIENT_DATA),
            evaluation.state
        );
        let Some(stored) = self.alarms.get_mut(name) else {
            return;
        };
        stored.alarm.state_value = Some(evaluation.state.to_string());
        stored.alarm.state_reason = Some(evaluation.reason);
        stored.alarm.state_reason_data = Some(evaluation.reason_data);
        stored.alarm.state_updated_timestamp = Some(to_timestamp(now_secs));
        // "The timestamp of the last change to the alarm's StateValue."
        // (`MetricAlarm$StateTransitionedTimestamp`) — this evaluation changed it.
        stored.alarm.state_transitioned_timestamp = Some(to_timestamp(now_secs));
        let updated = AlarmStateSnapshot::of_metric_alarm(&stored.alarm);
        let notification = AlarmNotification::of_transition(
            &stored.alarm,
            &previous,
            now_secs,
        );

        self.record_history(
            name,
            now_secs,
            AlarmHistoryItem {
                alarm_name: Some(name.to_string()),
                alarm_contributor_id: None,
                alarm_type: Some(ALARM_TYPE_METRIC.to_string()),
                timestamp: Some(to_timestamp(now_secs)),
                history_item_type: Some("StateUpdate".to_string()),
                history_summary: Some(summary),
                history_data: Some(super::state_transition_history_data(&previous, &updated)),
                alarm_contributor_attributes: None,
            },
        );
        if let Some(notification) = notification {
            self.enqueue_notification(notification, now_secs);
        }
        // This alarm's new state is an input to every composite alarm whose rule reads it, so
        // their evaluations are owed from this instant too — and the composite pass that
        // follows this one in the same settle takes them.
        self.make_parents_due(name, now_secs);
    }

    /// Every CONTRIBUTOR of an alarm whose query returns a vector of series, graded by the
    /// alarm's own predicate — and `None` for an alarm that is graded on ONE series, whatever
    /// identifies it.
    ///
    /// "The query must return a result of vector type. Each entry in the vector result represents
    /// an alarm contributor." (`AlarmPromQLCriteria$Query`) — so the entries of the vector are
    /// what such an alarm is decided from: each one is a time series, graded window by window
    /// against the alarm's threshold by exactly the predicate a single-series alarm is graded by
    /// ([`Self::verdict_over`]), and the order they come back in is the order the query ranked
    /// them ([`super::metrics_insights`]).
    ///
    /// The vector comes from the Metrics Insights query the alarm watches, which is the one query
    /// language of this model whose result this engine resolves; an alarm whose
    /// `EvaluationCriteria` states a query in another language has contributors this engine
    /// resolves no entries for, so it is left to the state the write that configured it left.
    fn graded_contributors(
        &self,
        alarm: &MetricAlarm,
        now_secs: f64,
        budget: &mut EvaluationBudget,
    ) -> Option<Vec<GradedContributor>> {
        let queries = alarm.metrics.as_deref()?;
        let watched = watched_query(queries, alarm.threshold_metric_id.as_deref())?;
        let query = watched_insights_query(alarm).filter(super::metrics_insights::Query::groups_its_result)?;
        // Every entry of the vector is graded against the alarm's own threshold and comparison,
        // over the alarm's own periods: a contributor is a series of this alarm, not an alarm of
        // its own.
        let operator = alarm.comparison_operator.as_deref()?;
        let comparison = comparison_of(operator)?;
        let threshold = alarm.threshold?;
        let predicate = Predicate::Static {
            threshold,
            comparison,
        };
        let evaluation_periods = evaluated_periods(alarm)?;
        let grid = EvaluationGrid::of(alarm, now_secs);
        let period = graded_period(queries, watched);
        let read = self.watched_data(alarm, now_secs, budget);
        let range_periods = evaluation_range_periods(
            evaluation_periods,
            graded_window_secs(alarm),
            read.storage_span,
            grid,
            now_secs,
        );
        if budget.spent() {
            return None;
        }
        // The range the query is evaluated over is exactly the windows the evaluation grades,
        // laid on the alarm's own grid — the same range a `Metrics` array of this alarm would be
        // resolved over ([`Self::resolved_query_array`]).
        let start = grid.window_start(now_secs, range_periods.saturating_sub(1), period);
        let end = grid.range_end(now_secs, period);
        #[allow(clippy::cast_possible_truncation)]
        let period_secs = period.trunc() as i32;
        let answered = query.evaluate(self, start, end, period_secs, now_secs);
        budget.charge_datapoints(answered.datapoints);
        let mut graded: Vec<GradedContributor> = Vec::with_capacity(answered.groups.len());
        for group in answered.groups {
            let series = GradedSeries {
                // No single statistic describes a series a query folded, so its decision record
                // names none.
                statistic: None,
                grid,
                period,
                windows: graded_windows(
                    &group.series.points,
                    grid,
                    period,
                    range_periods,
                    now_secs,
                ),
            };
            let Some(evaluation) = self.verdict_over(
                alarm,
                &series,
                &predicate,
                evaluation_periods,
                &read,
                now_secs,
            ) else {
                continue;
            };
            graded.push(GradedContributor {
                series: MetricKey {
                    namespace: group
                        .series
                        .metric
                        .as_ref()
                        .map(|metric| metric.namespace.clone())
                        .unwrap_or_default(),
                    metric_name: group
                        .series
                        .metric
                        .as_ref()
                        .map(|metric| metric.metric_name.clone())
                        .unwrap_or_default(),
                    dimensions: group.labels.clone(),
                },
                labels: group.labels,
                evaluation,
            });
        }
        Some(graded)
    }

    /// Keep what this evaluation graded each of the alarm's contributors at, so a read of them
    /// reports the entries of the vector as they stand now.
    ///
    /// An entry that is in the state it was already in keeps the instant it reached that state:
    /// "The timestamp when the contributor last transitioned to its current state."
    /// (`AlarmContributor$StateTransitionedTimestamp`) — so the instant moves only where the
    /// entry's own state did.
    fn record_contributors(
        &mut self,
        name: &str,
        graded: &[GradedContributor],
        now_secs: f64,
    ) {
        let Some(stored) = self.alarms.get_mut(name) else {
            return;
        };
        let held = ::std::mem::take(&mut stored.contributors);
        stored.contributors = graded
            .iter()
            .map(|contributor| {
                let id = contributor_id(name, Some(&contributor.series));
                let since = held
                    .iter()
                    .find(|previous| {
                        previous.contributor_id == id
                            && previous.state_value == contributor.evaluation.state
                    })
                    .map_or(now_secs, |previous| previous.state_transitioned_secs);
                super::StoredAlarmContributor {
                    contributor_id: id,
                    attributes: contributor.labels.clone(),
                    state_value: contributor.evaluation.state.to_string(),
                    state_reason: contributor.evaluation.reason.clone(),
                    state_transitioned_secs: since,
                }
            })
            .collect();
    }

    /// Create the anomaly detection model `alarm`'s threshold is drawn from, when the store
    /// holds none for the metric that band is drawn from.
    ///
    /// An alarm whose threshold is a model names a band as that threshold ("the ID of the
    /// ANOMALY_DETECTION_BAND function used as the threshold for the alarm",
    /// `MetricAlarm$ThresholdMetricId`) and a band is a model's ("You can use the model to
    /// display a band of expected normal values when the metric is graphed.",
    /// `PutAnomalyDetector`), so the write that configures such an alarm creates the model for
    /// each metric its band is drawn from where the account holds none — pending training,
    /// exactly as a model a caller created directly is.
    ///
    /// The model is the alarm's own standing requirement rather than something its write
    /// creates once: an alarm needs a model for every metric its band is drawn from for as long
    /// as it is configured that way, so each EVALUATION of such an alarm ensures the model is
    /// there ([`Self::evaluate_metric_alarm`]) and a model deleted while an alarm still draws a
    /// band from it is back by that alarm's next evaluation. `DeleteAnomalyDetector` "Deletes
    /// the specified anomaly detection model from your account" (`DeleteAnomalyDetector`) and
    /// deletes it here too — nothing but an alarm's own evaluation puts one back, so a model no
    /// alarm's threshold is drawn from stays deleted.
    ///
    /// The model this creates carries no training configuration and no metric characteristics:
    /// the alarm names only the metric and statistic its band is drawn from. Its identifier is
    /// minted for it like any other model's, so a client can tell the model that came back from
    /// the one it deleted.
    pub fn create_band_models(&mut self, alarm: &MetricAlarm, now_secs: f64) {
        for metric in band_model_metrics(alarm) {
            let form = super::AnomalyDetectorForm::of_series_metric(&metric);
            let detector_key = form.detector_key();
            if self.anomaly_detectors.contains_key(&detector_key)
                || self.anomaly_detectors.len() >= super::MAX_ANOMALY_DETECTORS
            {
                continue;
            }
            self.anomaly_detectors.insert(
                detector_key,
                super::StoredAnomalyDetector::created(
                    &form,
                    None,
                    None,
                    super::minted_anomaly_detector_id(),
                    now_secs,
                ),
            );
        }
    }

    /// The state the alarm's own comparison puts it in at `now_secs`, or `None` for an alarm
    /// this engine does not evaluate (see the module docs) and for one whose computed state
    /// is the state it already has. The datapoints the fold visits are charged to `budget`.
    fn metric_alarm_evaluation(
        &self,
        alarm: &MetricAlarm,
        now_secs: f64,
        budget: &mut EvaluationBudget,
    ) -> Option<Evaluation> {
        // An alarm over a PromQL query is not graded against a threshold of this store at all,
        // so nothing is computed for it: the entries of its query's result vector are its
        // contributors ("Each entry in the vector result represents an alarm contributor",
        // `AlarmPromQLCriteria$Query`), this engine resolves no query of that language, and the
        // state such an alarm reports is therefore the one the write that configured it left —
        // `OK`, with the explanation of `OK` that write seeded
        // ([`NO_CONTRIBUTOR_IN_ALARM_REASON`]).
        if alarm.evaluation_criteria.is_some() {
            return None;
        }
        let operator = alarm.comparison_operator.as_deref()?;
        let evaluation_periods = evaluated_periods(alarm)?;
        // The grid this alarm's periods lie on, and whether the period the clock is IN is one
        // this evaluation may grade at all: the whole of what its `EvaluationWindow` decides
        // ([`EvaluationGrid`]).
        let grid = EvaluationGrid::of(alarm, now_secs);
        // The windows of the EVALUATION RANGE: window 0 is the newest period this evaluation
        // grades ([`EvaluationGrid::newest_graded_period`]), window i the period before it, and
        // the range reaches further back than the look-back ([`evaluation_range_periods`]) — how
        // much further depends on how wide this alarm's windows are and on the granularity the
        // series it grades is STORED at, so both are resolved from the same places the grading
        // itself reads them from ([`graded_window_secs`], [`Self::watched_data`]).
        let watched = self.watched_data(alarm, now_secs, budget);
        let range_periods = evaluation_range_periods(
            evaluation_periods,
            graded_window_secs(alarm),
            watched.storage_span,
            grid,
            now_secs,
        );
        // The series this alarm GRADES, as the value each of those windows took, and what it
        // compares each of them against: the one number the alarm is configured with, or the
        // band the query its `ThresholdMetricId` designates draws.
        let (graded, predicate) = match alarm.threshold_metric_id.as_deref() {
            Some(threshold_id) => {
                let bound = band_bound_of(operator)?;
                let queries = alarm.metrics.as_deref()?;
                let (graded, bands) = self.graded_against_band(
                    queries,
                    threshold_id,
                    range_periods,
                    grid,
                    now_secs,
                    budget,
                )?;
                (graded, Predicate::Band { bound, bands })
            }
            None => {
                let comparison = comparison_of(operator)?;
                let threshold = alarm.threshold?;
                let graded = self.graded_series(alarm, range_periods, grid, now_secs, budget)?;
                (
                    graded,
                    Predicate::Static {
                        threshold,
                        comparison,
                    },
                )
            }
        };

        self.verdict_over(
            alarm,
            &graded,
            &predicate,
            evaluation_periods,
            &watched,
            now_secs,
        )
    }

    /// The state ONE graded series puts an alarm in, with the explanation it reports — the
    /// verdict every metric alarm is decided by, whatever identified the series it grades.
    ///
    /// `None` where this evaluation decided nothing: an evaluation whose every real datapoint was
    /// withheld, and one that graded no datapoint at all while the series it reads hold one its
    /// range has not reached yet. Such an alarm keeps the state it has rather than being given
    /// one nothing computed.
    fn verdict_over(
        &self,
        alarm: &MetricAlarm,
        graded: &GradedSeries<'_>,
        predicate: &Predicate,
        evaluation_periods: usize,
        watched: &WatchedData,
        now_secs: f64,
    ) -> Option<Evaluation> {
        let grid = graded.grid;
        let treat_missing = alarm.treat_missing_data.as_deref().unwrap_or(MISSING_MISSING);
        // How many observations a window has to hold before this alarm grades the percentile
        // over it at all, for an alarm configured to withhold its state while a period is too
        // thinly sampled for that percentile ([`low_sample_floor`]); `None` for every alarm
        // that is graded on whatever its periods hold.
        let low_samples_withheld_below = graded
            .statistic
            .as_ref()
            .and_then(|statistic| low_sample_floor(alarm, statistic));
        // The REAL datapoints of the range, newest first and at most `EvaluationPeriods` of
        // them: "all real data points in the evaluation range are included in the
        // evaluation", and where the range holds more than the alarm evaluates, it is the
        // most recent ones it evaluates.
        let mut evaluated: Vec<Evaluated> = Vec::with_capacity(evaluation_periods);
        // How many windows held a datapoint this alarm did not grade: one holding too few
        // observations for its percentile, and one the alarm's threshold says nothing about.
        let mut withheld = 0_usize;
        for (index, window) in graded.windows.iter().enumerate() {
            if evaluated.len() == evaluation_periods {
                break;
            }
            let Some(datapoint) = window else {
                continue;
            };
            // A window holding fewer observations than the percentile needs is not one this
            // alarm grades: the value over it is withheld, so nothing it would have decided
            // reaches the predicate below.
            if low_samples_withheld_below.is_some_and(|floor| {
                datapoint.sample_count.is_some_and(|count| count < floor)
            }) {
                withheld += 1;
                continue;
            }
            // A window whose threshold the alarm's configuration does not yield — a period a
            // still-training model says nothing about — is not graded either: there is no
            // comparison to make for it, and the period WAS published into, so no arm of the
            // missing-data treatment stands in for it.
            let Some((breaches, threshold)) = predicate.judge(index, datapoint.value) else {
                withheld += 1;
                continue;
            };
            evaluated.push(Evaluated {
                period: index,
                value: Some(datapoint.value),
                // How many observations the window stood for, which is what a decision record
                // reports beside the value the statistic took over them.
                sample_count: datapoint.sample_count,
                // The threshold this window's datapoint was compared against, which a record
                // reports per datapoint where it moves with the series.
                threshold: predicate.moving_threshold().then_some(threshold),
                datapoint: if breaches {
                    Datapoint::Breaching
                } else {
                    Datapoint::NotBreaching
                },
            });
        }
        // "the alarm state does not change during periods with too few data points to be
        // statistically significant": an evaluation whose every real datapoint was withheld
        // has nothing left to decide on, so it computes no state and the alarm keeps the one
        // it has — rather than being moved by a missing-data arm that stood in for a window
        // the metric really was published into.
        if withheld > 0 && evaluated.is_empty() {
            return None;
        }
        // An evaluation that graded NO datapoint at all, while the series this alarm reads hold
        // one its range has not reached yet, has not graded this alarm's series: the periods it
        // covered are not periods the series has no datapoint for — the datapoint belongs to a
        // period NEWER than the end of this evaluation's range
        // ([`EvaluationGrid::newest_graded_period`]) — so no arm of the missing-data treatment
        // stands in for anything, and the alarm keeps the state it has until the evaluation
        // whose range reaches that datapoint grades it. Without this an alarm would reach a
        // decided state from the treatment alone, over an EMPTY window, while a client was
        // publishing into the very series it watches.
        //
        // The wait is until the end of the period the datapoint was published into has passed,
        // on either grid: no verdict is reported over a period that has not closed
        // ([`EvaluationGrid::newest_graded_period`]), so a period the series really holds no
        // datapoint for is decided by the treatment as soon as the period a newer datapoint
        // landed in has itself closed.
        let range_end = grid.range_end(now_secs, graded.period);
        if evaluated.is_empty() && watched.newest_instant.is_some_and(|at| at >= range_end) {
            return None;
        }
        self.graded_evaluation(
            alarm,
            graded,
            GradedAgainst {
                predicate,
                // How many of the alarm's periods held a datapoint it withheld: those slots are
                // not ones the missing-data treatment may stand in for either, because
                // substituting a datapoint for a period the metric really was published into
                // would move the state on the strength of exactly the under-sampled period the
                // alarm asked to ignore.
                withheld,
                evaluation_periods,
                treat_missing,
            },
            evaluated,
            now_secs,
        )
    }

    /// What the series this alarm READS hold, in ONE pass over them ([`WatchedData`]). The
    /// datapoints it visits are charged to `budget`, as every fold over a series is.
    ///
    /// The series are every one the alarm reads — the one its own members name, or each metric a
    /// `Metrics` array's queries read ([`watched_series_set`]), plus every series the Metrics
    /// Insights query it watches SELECTS ([`watched_insights_query`]) — because the value a query
    /// yields is only as dense, and only as fresh, as the series it is computed from.
    ///
    /// What a datapoint's granularity is, is the datapoint's own: "Setting this to 1 specifies
    /// this metric as a high-resolution metric […] Setting this to 60 specifies this metric as
    /// regular resolution" (`MetricDatum$StorageResolution`), which is the span it is aggregated
    /// into ([`super::StoredDatum::aggregated_span`]) and so the shortest interval two of its
    /// datapoints can be apart. The coarsest of them is what the evaluation range has to reach
    /// back over ([`evaluation_range_periods`]).
    ///
    /// The newest instant is read over the datapoints this alarm GRADES — a datapoint carrying
    /// another unit than the one the alarm or its query selects is not one it reads at all
    /// ([`grades_unit`]) — and only over the ones already in the past, since a datapoint stamped
    /// ahead of this instant ("as much as 2 hours after the current day and time",
    /// `PutMetricData`) belongs to a period the clock has not reached and no evaluation running
    /// now is behind it.
    fn watched_data(
        &self,
        alarm: &MetricAlarm,
        now_secs: f64,
        budget: &mut EvaluationBudget,
    ) -> WatchedData {
        let mut found = WatchedData::default();
        let read = |found: &mut WatchedData, series: &[StoredDatum], unit: Option<&str>| {
            for datum in series {
                found.storage_span = found.storage_span.max(datum.aggregated_span());
                if !grades_unit(unit, datum.unit()) {
                    continue;
                }
                let at = datum.aggregated_instant();
                if at > now_secs {
                    continue;
                }
                if found.newest_instant.is_none_or(|newest| at > newest) {
                    found.newest_instant = Some(at);
                }
            }
        };
        for (key, unit) in watched_series_set(alarm) {
            let Some(series) = self.metrics.get(&key) else {
                continue;
            };
            budget.charge_datapoints(series.len());
            read(&mut found, series, unit.as_deref());
        }
        // A Metrics Insights query names no series of its own: it SELECTS them out of the store
        // by namespace, metric name and labels, so what the alarm reads is every series its
        // query selects — and it names no unit, so every datapoint of each of them.
        if let Some(query) = watched_insights_query(alarm) {
            for (key, series) in &self.metrics {
                if !query.selects(key) {
                    continue;
                }
                budget.charge_datapoints(series.len());
                read(&mut found, series, None);
            }
        }
        found
    }

    /// The series ONE alarm grades, as the value each window of its evaluation range took.
    ///
    /// Which series that is, is what the alarm's configuration says: an alarm over one metric
    /// grades that metric folded by its own `Statistic` over its own `Period`, and an alarm
    /// configured with a query array ("Use the `Metrics` parameter to create an alarm based on
    /// a metric math expression", `PutMetricAlarmInput$Metrics`) grades the value of the query
    /// that array RETURNS — the expression's output, not the metrics it reads. Either way the
    /// values come from the datapoints `PutMetricData` stored and from nothing else.
    fn graded_series<'alarm>(
        &self,
        alarm: &'alarm MetricAlarm,
        range_periods: usize,
        grid: EvaluationGrid,
        now_secs: f64,
        budget: &mut EvaluationBudget,
    ) -> Option<GradedSeries<'alarm>> {
        match alarm.metrics.as_deref() {
            Some(queries) => {
                self.graded_query_array(queries, range_periods, grid, now_secs, budget)
            }
            None => self.graded_metric(alarm, range_periods, grid, now_secs, budget),
        }
    }

    /// The windows of the ONE metric an alarm names, folded by the statistic it names.
    fn graded_metric<'alarm>(
        &self,
        alarm: &'alarm MetricAlarm,
        range_periods: usize,
        grid: EvaluationGrid,
        now_secs: f64,
        budget: &mut EvaluationBudget,
    ) -> Option<GradedSeries<'alarm>> {
        let statistic = AlarmStatistic::of(alarm)?;
        let period = alarm_period_secs(alarm)?;
        let key = watched_series(alarm);
        // An alarm this engine evaluates names the series it watches, which `watched_series`
        // resolves from those same members.
        if key.namespace.is_empty() || key.metric_name.is_empty() {
            return None;
        }

        // One pass over the series fills every window of the evaluation range.
        let mut windows: Vec<Vec<&StoredDatum>> = vec![Vec::new(); range_periods];
        let series = self.metrics.get(&key);
        // The datapoints this fold is about to visit, charged before it visits them: this is
        // the factor that multiplies with the number of alarms one request evaluates.
        budget.charge_datapoints(series.map_or(0, Vec::len));
        // Which window of the evaluation range a datapoint falls in, or `None` for one this
        // alarm does not grade at all.
        //
        // A datapoint belongs to the period of the alarm's own grid its OWN time stamp falls in,
        // so which window it is aggregated into is decided by the datapoint and by the grid
        // ([`EvaluationGrid`]) rather than by where in a period the evaluation happens to run —
        // the same fold this store's readers make over a series
        // ([`super::windows_by_unit`]), laid from this grid's boundaries instead of from a
        // requested start, and down to the granularity the datapoint's own `StorageResolution`
        // stores it at ([`StoredDatum::aggregated_instant`]). A period
        // FINER than that granularity still grades the datapoint, in the window its stored
        // instant falls in: an alarm "accepts" a sub-minute period over a regular-resolution
        // series and buckets it by the minute, which is what the recorded evaluation of such an
        // alarm shows.
        //
        // A datapoint of a period NEWER than the newest one this evaluation grades — one
        // stamped past the end of its range, or up to two hours ahead of this instant
        // ("as much as 2 hours after the current day and time", `PutMetricData`) — is graded by
        // a later evaluation, whose range has advanced over it, and not by this one.
        let graded_window = |datum: &StoredDatum| -> Option<usize> {
            grid.periods_back(now_secs, datum.aggregated_instant(), period)
                .filter(|index| *index < range_periods)
        };
        for datum in series.into_iter().flatten() {
            // "If you specify a unit, the operation returns only data that was collected with
            // that unit specified" — the alarm's own `Unit` selects which datapoints of the
            // series it grades, and an alarm naming none grades every one of them.
            if !carries_alarm_unit(alarm, datum) {
                continue;
            }
            if let Some(window) = graded_window(datum).and_then(|index| windows.get_mut(index)) {
                window.push(datum);
            }
        }

        Some(GradedSeries {
            // The statistic the alarm folded its windows by, which its decision record names.
            statistic: Some(statistic),
            grid,
            period,
            windows: windows
                .iter()
                .map(|window| {
                    statistic.over(window).map(|value| GradedDatapoint {
                        value,
                        // How many observations the window stood for, which is what a
                        // decision record reports beside the value over them.
                        sample_count: Some(window.iter().map(|datum| datum.sample_count).sum()),
                    })
                })
                .collect(),
        })
    }

    /// The windows of the series an alarm's `Metrics` query ARRAY grades: the value of the
    /// query that array returns, computed over the same stored datapoints `GetMetricData`
    /// answers the same array from ([`super::metric_query`]).
    ///
    /// `None` for an array this engine cannot resolve to ONE series — a malformed or
    /// unevaluable expression, a query naming no stored metric, a value that is a number or
    /// several series rather than one — so such an alarm keeps the state it has rather than
    /// being given one nothing computed.
    fn graded_query_array(
        &self,
        queries: &[MetricDataQuery],
        range_periods: usize,
        grid: EvaluationGrid,
        now_secs: f64,
        budget: &mut EvaluationBudget,
    ) -> Option<GradedSeries<'static>> {
        let graded = watched_query(queries, None)?;
        let (resolution, period) =
            self.resolved_query_array(queries, graded, range_periods, grid, now_secs, budget)?;
        Some(GradedSeries {
            // No single statistic describes a series a query array computed, so its decision
            // record names none.
            statistic: None,
            grid,
            period,
            windows: graded_windows(
                single_series(&resolution, &graded.id)?,
                grid,
                period,
                range_periods,
                now_secs,
            ),
        })
    }

    /// The series an anomaly alarm grades and the BAND it grades each of its windows against:
    /// the value of the query the array returns, and the pair of bounds the query
    /// `threshold_id` designates yields over the same windows.
    ///
    /// "In an alarm based on an anomaly detection model, this is the ID of the
    /// ANOMALY_DETECTION_BAND function used as the threshold for the alarm"
    /// (`MetricAlarm$ThresholdMetricId`) — so the two come out of ONE resolution of the array,
    /// the same resolution `GetMetricData` answers that array from: the bound the alarm cites
    /// for a period is the value the band's own series carries at it, and nothing recomputes
    /// either of them a second way.
    ///
    /// `None` when the array resolves to no single watched series, or when the designated query
    /// is not a band at all — a value that is not the Low/High pair that function yields is no
    /// threshold, so the alarm keeps the state it has.
    fn graded_against_band(
        &self,
        queries: &[MetricDataQuery],
        threshold_id: &str,
        range_periods: usize,
        grid: EvaluationGrid,
        now_secs: f64,
        budget: &mut EvaluationBudget,
    ) -> Option<(GradedSeries<'static>, Vec<Option<Band>>)> {
        let graded = watched_query(queries, Some(threshold_id))?;
        let (resolution, period) =
            self.resolved_query_array(queries, graded, range_periods, grid, now_secs, budget)?;
        let watched = graded_windows(
            single_series(&resolution, &graded.id)?,
            grid,
            period,
            range_periods,
            now_secs,
        );
        let (lower, upper) = band_bounds(&resolution, threshold_id)?;
        let lower = graded_windows(lower, grid, period, range_periods, now_secs);
        let upper = graded_windows(upper, grid, period, range_periods, now_secs);
        // A period is inside the band only where BOTH bounds have a value for it: a model
        // reports its band as the two series together, so half of one is no band.
        let bands = lower
            .iter()
            .zip(&upper)
            .map(|bounds| match bounds {
                (Some(low), Some(high)) => Some(Band {
                    lower: low.value,
                    upper: high.value,
                }),
                _ => None,
            })
            .collect();
        Some((
            GradedSeries {
                statistic: None,
                grid,
                period,
                windows: watched,
            },
            bands,
        ))
    }

    /// ONE resolution of an alarm's `Metrics` array over the windows its evaluation grades,
    /// with the granularity those windows are laid at: the value of every query of the array,
    /// computed exactly as a `GetMetricData` request over the same array is answered.
    ///
    /// `None` for an array this engine cannot resolve — a malformed or unevaluable expression,
    /// a query naming no stored metric, or one asking for more work than one resolution may do
    /// — so such an alarm keeps the state it has rather than being graded from a part of itself.
    fn resolved_query_array(
        &self,
        queries: &[MetricDataQuery],
        graded: &MetricDataQuery,
        range_periods: usize,
        grid: EvaluationGrid,
        now_secs: f64,
        budget: &mut EvaluationBudget,
    ) -> Option<(super::metric_query::Resolution, f64)> {
        let period = graded_period(queries, graded);
        // The range the array is resolved over is exactly the windows the evaluation grades:
        // laid on the alarm's own grid, the newest being the one the end of this evaluation's
        // range falls in ([`EvaluationGrid::newest_graded_period`]), so the range ends where
        // that window ends.
        let start = grid.window_start(now_secs, range_periods.saturating_sub(1), period);
        let end = grid.range_end(now_secs, period);

        // How much work this array asks for is bounded BEFORE anything is parsed or folded:
        // the queries of the array and the windows of the range are the factors of every walk
        // below, so an array past what one resolution may do leaves the alarm in the state it
        // has rather than being graded from a part of itself.
        if budget.spent()
            || super::metric_query::resolution_refusal(queries, start, end).is_some()
        {
            return None;
        }
        // Every expression is parsed, and the order they are evaluated in settled, before any
        // of them is evaluated, exactly as a request over the same array is answered.
        let parsed = super::metric_query::parse_queries(queries).ok()?;
        let order = super::metric_query::evaluation_order(queries, &parsed).ok()?;
        let mut resolution = super::metric_query::Resolution::default();
        for query in queries {
            // The queries of ONE array are the second factor of this pass's work, so the
            // budget is checked between them too: an array the budget does not reach leaves the
            // alarm due for the next call rather than being graded from a part of itself.
            if budget.spent() {
                return None;
            }
            // A Metrics Insights query SELECTS series out of the store rather than computing
            // over the values of other queries, so it is resolved in this pass, exactly as a
            // `GetMetricData` request over the same array resolves it.
            if query
                .expression
                .as_deref()
                .is_some_and(super::metric_math::is_metrics_insights_query)
            {
                let (value, datapoints) =
                    super::metric_query::insights_series(self, query, start, end, now_secs).ok()?;
                budget.charge_datapoints(datapoints);
                resolution.resolve_query(query, value, datapoints).ok()?;
                continue;
            }
            let Some(stat) = query.metric_stat.as_ref() else {
                continue;
            };
            // The datapoints this query's fold is about to visit, charged before it visits
            // them, as for an alarm over one metric.
            budget.charge_datapoints(
                super::metric_query::query_series_key(stat)
                    .and_then(|key| self.metrics.get(&key))
                    .map_or(0, Vec::len),
            );
            let series =
                super::metric_query::retrieved_series(self, query, stat, start, end, now_secs)
                    .ok()?;
            resolution.retrieve(query, series).ok()?;
        }
        for id in order {
            if budget.spent() {
                return None;
            }
            let tree = parsed.get(id)?;
            let evaluated = resolution
                .evaluate(
                    id,
                    tree,
                    &super::metric_query::StoreSeriesSource {
                        store: self,
                        requested_start: start,
                        end,
                        now: now_secs,
                    },
                    (start, end),
                    period,
                )
                .ok()?;
            resolution.record(id, evaluated);
        }
        Some((resolution, period))
    }

    /// The state the alarm's "M out of N" predicate puts it in over the datapoints `evaluated`,
    /// with the explanation it reports — the one verdict every metric alarm is decided by,
    /// whatever series it grades.
    fn graded_evaluation(
        &self,
        alarm: &MetricAlarm,
        graded: &GradedSeries<'_>,
        against: GradedAgainst<'_>,
        mut evaluated: Vec<Evaluated>,
        now_secs: f64,
    ) -> Option<Evaluation> {
        let GradedAgainst {
            predicate,
            withheld,
            evaluation_periods,
            treat_missing,
        } = against;
        let period = graded.period;
        // The grid the windows were laid on, so the instants the explanation names are the ones
        // the grading itself graded ([`EvaluationGrid`]).
        let grid = graded.grid;
        // What the missing-data treatment fills the slots no real datapoint filled with: a
        // substituted datapoint under `breaching` and `notBreaching`, and nothing at all
        // under `missing` and `ignore`, which leave the evaluation with fewer datapoints. A
        // slot a WITHHELD datapoint came from is filled by nothing at all whatever the
        // treatment says: the period was published into, so no arm of the treatment decided
        // anything for it.
        let substituted = match treat_missing {
            MISSING_BREACHING => Datapoint::Breaching,
            MISSING_NOT_BREACHING => Datapoint::NotBreaching,
            _ => Datapoint::Missing,
        };
        // WHICH period each of those slots stands for: the periods of the look-back no real
        // datapoint filled, newest first. A slot stands for the period the treatment decided
        // FOR, so a record can name it by that period's own instant exactly as it names a
        // period a datapoint really came from.
        let filled: Vec<usize> = evaluated.iter().map(|slot| slot.period).collect();
        let mut unfilled = (0..evaluation_periods).filter(|period| !filled.contains(period));
        for _ in evaluated.len()..evaluation_periods.saturating_sub(withheld) {
            let Some(period) = unfilled.next() else {
                break;
            };
            evaluated.push(Evaluated {
                period,
                value: None,
                sample_count: None,
                threshold: None,
                datapoint: substituted,
            });
        }

        let breaching = evaluated
            .iter()
            .filter(|slot| slot.datapoint == Datapoint::Breaching)
            .count();
        let not_breaching = evaluated
            .iter()
            .filter(|slot| slot.datapoint == Datapoint::NotBreaching)
            .count();
        let present = evaluated
            .iter()
            .filter(|slot| slot.datapoint != Datapoint::Missing)
            .count();
        // The slots no real datapoint filled that the treatment SUBSTITUTED a datapoint for:
        // these are the holes the evaluation graded without having seen any data for them, and
        // the reason has to report them as holes rather than as datapoints it counted.
        let substituted = evaluated
            .iter()
            .filter(|slot| !slot.measured() && slot.datapoint != Datapoint::Missing)
            .count();
        // "The number of data points that must be breaching to trigger the alarm. This is
        // used only if you are setting an "M out of N" alarm." — an alarm that names no M
        // breaches when every one of its N datapoints does.
        let to_alarm = alarm
            .datapoints_to_alarm
            .and_then(|count| usize::try_from(count).ok())
            .filter(|count| *count >= 1)
            .unwrap_or(evaluation_periods)
            .min(evaluation_periods);
        // The complement of M over the same N: the number of NON-breaching datapoints that
        // makes M breaching ones unreachable inside the window, which is the minimum an
        // evaluation needs to put the alarm in `OK`. Both minima are built from the alarm's
        // configuration, so neither depends on how many datapoints this one evaluation found.
        let to_ok = evaluation_periods
            .saturating_sub(to_alarm)
            .saturating_add(1);

        let state = if breaching >= to_alarm {
            ALARM_STATE_ALARM
        } else if not_breaching >= to_ok {
            ALARM_STATE_OK
        } else if treat_missing == MISSING_IGNORE {
            // Neither direction's minimum was reached, and the datapoints the treatment left
            // out are why: "ignore — the current alarm state is maintained", so this
            // evaluation produces no state at all and the alarm keeps the one it has.
            return None;
        } else {
            // The window the evaluation graded is short of the datapoints EITHER direction
            // needs — the treatments that substitute nothing are the only ones that can leave
            // it so — so there is no verdict to report: the alarm had insufficient data.
            ALARM_STATE_INSUFFICIENT_DATA
        };
        // Which datapoints the verdict RESTS ON: the direction the state was decided in, and
        // how many datapoints of it that direction needed. A state no datapoint decided —
        // insufficient data — rests on none, and its sentence names no minimum either.
        let verdict = if state == ALARM_STATE_ALARM {
            Some(Verdict {
                satisfying: Datapoint::Breaching,
                minimum: to_alarm,
            })
        } else if state == ALARM_STATE_OK {
            Some(Verdict {
                satisfying: Datapoint::NotBreaching,
                minimum: to_ok,
            })
        } else {
            None
        };
        let counts = EvaluationCounts {
            breaching,
            not_breaching,
            present,
            substituted,
            periods: evaluated.len(),
            minimum: verdict.map_or(0, |verdict| verdict.minimum),
        };
        // How the sentence reads the comparison the evaluation made, and the thresholds it
        // cites: the one number a static alarm names, or the band bound each counted datapoint
        // was compared against.
        let (phrase, thresholds) = predicate.phrase(&evaluated);
        Some(Evaluation {
            state,
            reason: state_reason(
                state,
                counts,
                missing_data_arm(treat_missing),
                phrase,
                &thresholds,
                &cited_datapoints(&evaluated, verdict, grid, now_secs, period),
            ),
            reason_data: state_reason_data(
                graded.statistic.as_ref(),
                GradedWindows {
                    grid,
                    period,
                    evaluation_periods,
                },
                predicate.fixed_threshold(),
                now_secs,
                &evaluated,
                verdict,
            ),
        })
    }

    /// Queue one transition's record, within [`MAX_PENDING_NOTIFICATIONS`] — with its ACTIONS
    /// dropped when an ACTIVE alarm mute rule MUTES them.
    ///
    /// "When a mute rule is active, the targeted alarms continue to evaluate metrics and
    /// transition between states, but their configured actions (such as Amazon SNS notifications
    /// or Auto Scaling actions) are muted." (`PutAlarmMuteRule`) — so this is where the mute
    /// applies, for every transition whichever call or clock materialised it: the alarm has
    /// already moved and been recorded as moving, and what the mute decides is only whether the
    /// actions configured for the state it moved to are executed. A muted transition executes
    /// NONE of them and leaves the account of that in the alarm's own history
    /// ([`super::muted_actions_history_item`]) instead of a record of an execution that never
    /// happened. What a mute does NOT decide is the event the state change publishes
    /// ([`super::alarm_actions::state_change_events`]): that is owed by the transition rather
    /// than configured on the alarm, so the record is queued either way.
    ///
    /// The question is asked at the instant of the TRANSITION rather than at delivery, because
    /// that is the instant whose mute windows decide whether these actions fire at all.
    pub(crate) fn enqueue_notification(
        &mut self,
        notification: AlarmNotification,
        now_secs: f64,
    ) {
        let mut notification = notification;
        // What the event this transition publishes reports as the state the alarm moved OUT OF:
        // the state the LAST event of this alarm announced, which is the state this transition
        // moved it out of. The alarm's own state members are the state as it reads now — an
        // evaluation that moved the alarm nowhere still rewrites the explanation of the state it
        // is in, and a write re-stating the same state refreshes its instant — so a consumer
        // following one alarm would otherwise be handed a previous state no event of that alarm
        // ever announced, and the chain of its transitions would not join up.
        if let Some(announced) = self.published_alarm_states.get(&notification.alarm_name) {
            notification
                .state_change
                .reports_previous_state(announced.clone());
        }
        self.published_alarm_states.insert(
            notification.alarm_name.clone(),
            notification.state_change.state_document(),
        );
        if let Some(rule_name) = self.muting_alarm_mute_rule(
            &notification.alarm_name,
            notification.alarm_arn.as_deref(),
            now_secs,
        ) {
            // A transition that fires no action at all has nothing for a mute to withhold, so
            // it leaves no record of a muting either.
            if !notification.actions.is_empty() {
                let item = super::muted_actions_history_item(
                    &notification.alarm_name,
                    &notification.alarm_type,
                    &rule_name,
                    &notification.new_state_value,
                    &notification.actions,
                    now_secs,
                );
                self.record_history(&notification.alarm_name, now_secs, item);
                notification.actions.clear();
            }
        }
        self.queue_notification(notification);
    }

    /// Put one record on the queue the settle delivers from, within
    /// [`MAX_PENDING_NOTIFICATIONS`].
    ///
    /// The one place a record is queued, so the capacity bound is applied to every record
    /// whatever made it due: a transition's ([`Self::enqueue_notification`]) and the actions a
    /// deleted mute rule released
    /// ([`super::ComAmazonawsCloudwatchGraniteserviceversion20100801State::delete_alarm_mute_rule`]).
    pub(crate) fn queue_notification(&mut self, notification: AlarmNotification) {
        while self.pending_notifications.len() >= MAX_PENDING_NOTIFICATIONS {
            let Some(dropped) = self.pending_notifications.pop_front() else {
                break;
            };
            ::tracing::warn!(
                alarm = %dropped.alarm_name,
                pending = MAX_PENDING_NOTIFICATIONS,
                "this store already holds the most undelivered alarm notifications it can; \
                 the oldest is dropped undelivered"
            );
        }
        self.pending_notifications.push_back(notification);
    }

    /// Take at most `limit` of the queued notifications, oldest first, leaving the rest for
    /// the next caller: the caller DELIVERS what it takes, so a taken notification must not be
    /// left for another caller to deliver twice, and a limit is what bounds the fan-out ONE
    /// request performs.
    pub fn take_pending_notifications(
        &mut self,
        limit: usize,
    ) -> std::collections::VecDeque<AlarmNotification> {
        let taken = limit.min(self.pending_notifications.len());
        self.pending_notifications.drain(..taken).collect()
    }
}

/// What one `ComparisonOperator` means to an evaluation: the test it makes between a datapoint
/// and the threshold, and the words an explanation states that test in.
///
/// The two live together so a reason can never word a comparison the evaluation did not make.
/// Reached by the LOG alarm fold as well ([`super::log_alarm_evaluation`]): an operator means the
/// same thing to every alarm that compares a value against a threshold, so both folds read it
/// here.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Comparison {
    /// Whether a datapoint BREACHES the threshold under this operator.
    pub(crate) breaches: fn(f64, f64) -> bool,
    /// How a `StateReason` reads the comparison: "… was <phrase> the threshold (<threshold>)",
    /// and negated ("was not <phrase> …") for the state in which it did not hold.
    pub(crate) phrase: &'static str,
}

/// The comparison one `ComparisonOperator` makes between a datapoint and a STATIC threshold, or
/// `None` for the anomaly-detection band operators, which compare against the pair of bounds a
/// model draws ([`band_bound_of`]) rather than against a number.
pub(crate) fn comparison_of(comparison: &str) -> Option<Comparison> {
    // The static-threshold members of the modelled `ComparisonOperator` enum
    // (`smithy_query(query="shape",shape="ComparisonOperator")`), each with the reading
    // CloudWatch's own alarm explanations give it.
    match comparison {
        "GreaterThanOrEqualToThreshold" => Some(Comparison {
            breaches: |value, threshold| value >= threshold,
            phrase: "greater than or equal to",
        }),
        "GreaterThanThreshold" => Some(Comparison {
            breaches: |value, threshold| value > threshold,
            phrase: "greater than",
        }),
        "LessThanThreshold" => Some(Comparison {
            breaches: |value, threshold| value < threshold,
            phrase: "less than",
        }),
        "LessThanOrEqualToThreshold" => Some(Comparison {
            breaches: |value, threshold| value <= threshold,
            phrase: "less than or equal to",
        }),
        // "LessThanLowerOrGreaterThanUpperThreshold", "LessThanLowerThreshold" and
        // "GreaterThanUpperThreshold" compare against an anomaly detection band.
        _ => None,
    }
}

/// The pair of bounds an anomaly-detection band holds ONE period's value between: the values
/// the model's two series carry at it.
#[derive(Debug, Clone, Copy)]
struct Band {
    /// What the band's "Low" series reports for the period.
    lower: f64,
    /// What its "High" series reports for the period.
    upper: f64,
}

/// Which bound of that band one `ComparisonOperator` tests a datapoint against.
#[derive(Debug, Clone, Copy)]
enum BandBound {
    /// `GreaterThanUpperThreshold`: a datapoint above the upper bound breaches.
    Upper,
    /// `LessThanLowerThreshold`: a datapoint below the lower bound breaches.
    Lower,
    /// `LessThanLowerOrGreaterThanUpperThreshold`: a datapoint outside the band breaches,
    /// whichever side of it the value left.
    Either,
}

/// The band bound one `ComparisonOperator` compares against, or `None` for the operators that
/// compare against a static threshold ([`comparison_of`]).
fn band_bound_of(comparison: &str) -> Option<BandBound> {
    // The band members of the modelled `ComparisonOperator` enum
    // (`smithy_query(query="shape",shape="ComparisonOperator")`), which are the operators an
    // alarm whose threshold is a model is written with.
    match comparison {
        "GreaterThanUpperThreshold" => Some(BandBound::Upper),
        "LessThanLowerThreshold" => Some(BandBound::Lower),
        "LessThanLowerOrGreaterThanUpperThreshold" => Some(BandBound::Either),
        _ => None,
    }
}

impl BandBound {
    /// Whether `value` breaches `band` under this operator, and the bound the explanation cites
    /// for that datapoint: the bound the comparison TESTED — the one a value outside the band
    /// crossed, and the upper bound for a two-sided comparison the value satisfied.
    fn judge(self, band: Band, value: f64) -> (bool, f64) {
        match self {
            Self::Upper => (value > band.upper, band.upper),
            Self::Lower => (value < band.lower, band.lower),
            Self::Either if value > band.upper => (true, band.upper),
            Self::Either if value < band.lower => (true, band.lower),
            Self::Either => (false, band.upper),
        }
    }

    /// How a `StateReason` reads this comparison, and what it calls the thresholds it cites:
    /// the same sentence a static-threshold alarm's reason is written in, with the band's own
    /// bounds in place of the one number ("… were greater than the upper thresholds [12.5]").
    fn phrase(self) -> (&'static str, &'static str) {
        match self {
            Self::Upper => ("greater than", "the upper thresholds"),
            Self::Lower => ("less than", "the lower thresholds"),
            Self::Either => ("outside", "the band"),
        }
    }
}

/// What ONE alarm compares each of its graded datapoints against.
#[derive(Debug)]
enum Predicate {
    /// "The value to compare with the specified statistic." (`MetricAlarm$Threshold`) — one
    /// number, for every window the evaluation grades.
    Static {
        /// The number itself.
        threshold: f64,
        /// The comparison the alarm makes between a datapoint and it.
        comparison: Comparison,
    },
    /// The band the query `ThresholdMetricId` designates draws: a threshold that moves with the
    /// series, so each window is compared against the bound its OWN period carries.
    Band {
        /// Which bound of the band the alarm's comparison tests against.
        bound: BandBound,
        /// The band of each window of the evaluation range, newest first, and `None` for a
        /// window the model says nothing about.
        bands: Vec<Option<Band>>,
    },
}

impl Predicate {
    /// Whether the datapoint `value` of the window `index` BREACHES, and the threshold it was
    /// compared against; `None` for a window this predicate has no threshold for, which is a
    /// window the evaluation does not grade at all.
    fn judge(&self, index: usize, value: f64) -> Option<(bool, f64)> {
        match self {
            Self::Static {
                threshold,
                comparison,
            } => Some(((comparison.breaches)(value, *threshold), *threshold)),
            Self::Band { bound, bands } => Some(bound.judge((*bands.get(index)?)?, value)),
        }
    }

    /// Whether the threshold MOVES from window to window, which is what makes a decision
    /// record report one threshold per datapoint rather than one for the whole evaluation.
    fn moving_threshold(&self) -> bool {
        matches!(self, Self::Band { .. })
    }

    /// The one threshold every window was compared against, for an alarm compared against a
    /// number; `None` for one compared against a band, whose datapoints each carry their own.
    fn fixed_threshold(&self) -> Option<f64> {
        match self {
            Self::Static { threshold, .. } => Some(*threshold),
            Self::Band { .. } => None,
        }
    }

    /// How a `StateReason` reads the comparison this predicate makes, and the thresholds that
    /// sentence cites: "greater than" and "the threshold (10.0)" for a number, and "greater
    /// than" and "the upper thresholds [12.5, 12.4]" for a band — the bound cited for each
    /// datapoint the reason counts, newest first.
    fn phrase(&self, cited: &[Evaluated]) -> (&'static str, String) {
        match self {
            Self::Static {
                threshold,
                comparison,
            } => (
                comparison.phrase,
                format!("the threshold ({})", decimal_text(*threshold)),
            ),
            Self::Band { bound, .. } => {
                let (phrase, name) = bound.phrase();
                let thresholds: Vec<String> = cited
                    .iter()
                    .filter_map(|slot| slot.threshold.map(decimal_text))
                    .collect();
                (phrase, format!("{name} [{}]", thresholds.join(", ")))
            }
        }
    }
}

/// The statistic one alarm aggregates each of its windows with.
#[derive(Debug, Clone, Copy)]
enum AlarmStatistic<'a> {
    /// "The statistic for the metric specified in MetricName" — one of the basic five
    /// (`MetricAlarm$Statistic`).
    Basic(&'a str),
    /// "The percentile statistic for the metric associated with the alarm"
    /// (`MetricAlarm$ExtendedStatistic`), as the name and the percentile it parses to.
    Percentile(&'a str, f64),
}

impl AlarmStatistic<'_> {
    /// The statistic the alarm names, or `None` when it names none this engine can compute:
    /// "For an alarm based on a metric, you must specify either Statistic or
    /// ExtendedStatistic but not both." (`PutMetricAlarmInput$Statistic`)
    fn of(alarm: &MetricAlarm) -> Option<AlarmStatistic<'_>> {
        if let Some(statistic) = alarm.statistic.as_deref() {
            // Only one of the modelled `Statistic` values is a statistic `BasicStatistics`
            // folds; anything else names a statistic nothing computes.
            return STATISTIC_VALUES
                .contains(&statistic)
                .then_some(AlarmStatistic::Basic(statistic));
        }
        let extended = alarm.extended_statistic.as_deref()?;
        super::parse_percentile(extended).map(|percentile| AlarmStatistic::Percentile(extended, percentile))
    }

    /// This statistic over one window's datapoints; `None` for a window with no datapoint of
    /// the metric at all, and for one whose individual values a percentile needs were never
    /// published.
    fn over(&self, window: &[&StoredDatum]) -> Option<f64> {
        if window.is_empty() {
            return None;
        }
        match self {
            Self::Basic(name) => BasicStatistics::of(window.iter().copied()).named(name),
            Self::Percentile(_, percentile) => weighted_percentile(window, *percentile),
        }
    }

    /// The name the alarm gave this statistic: one of the basic five, or the percentile name
    /// the alarm spelled.
    fn name(&self) -> &str {
        match self {
            Self::Basic(name) | Self::Percentile(name, _) => name,
        }
    }
}

/// The `EvaluateLowSampleCountPercentile` value under which a percentile alarm's state is NOT
/// moved by a period too thinly sampled for that percentile to mean anything: "If you specify
/// `ignore`, the alarm state does not change during periods with too few data points to be
/// statistically significant." The other documented value, `evaluate`, and the OMISSION of the
/// member both grade every period on whatever it holds — "If you specify `evaluate` or omit
/// this parameter, the alarm is always evaluated and possibly changes state no matter how many
/// data points are available" — so withholding is opt-in
/// (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="EvaluateLowSampleCountPercentile")`).
const LOW_SAMPLE_IGNORE: &str = "ignore";

/// The numerator of the sample-count floor a percentile has to clear to be statistically
/// significant ([`significance_floor`]).
const LOW_SAMPLE_FLOOR_NUMERATOR: f64 = 10.0;

/// The whole of the distribution, in percent: the span a percentile cuts a tail off each end of.
const WHOLE_DISTRIBUTION_PERCENT: f64 = 100.0;

/// How many observations ONE window has to hold before the `percentile`th percentile over it is
/// statistically significant: ten over the weight of the NEARER TAIL the percentile cuts off, so
/// a percentile closer to an edge of the distribution needs proportionally more observations — a
/// hundred at the ninetieth percentile (and at the tenth, which cuts off a tail of the same
/// weight at the other end), a thousand at the ninety-ninth — rather than one flat count for
/// every percentile.
///
/// `None` at the two degenerate ends of the range, where the nearer tail carries no weight at
/// all and the percentile is the window's largest or smallest observation: a window is graded on
/// what it holds there.
///
/// `percentile` is in PERCENT, the way an `ExtendedStatistic` name spells it
/// ([`super::parse_percentile`]), and the tail is measured in percent too: the quotient of two
/// whole percentages is exact for a percentile named in whole percent, so a window sitting
/// exactly ON its floor is not withheld by a rounding of the floor itself.
fn significance_floor(percentile: f64) -> Option<f64> {
    let tail = percentile.min(WHOLE_DISTRIBUTION_PERCENT - percentile);
    (tail > 0.0).then(|| LOW_SAMPLE_FLOOR_NUMERATOR * WHOLE_DISTRIBUTION_PERCENT / tail)
}

/// The sample count below which `alarm` WITHHOLDS the state a window would decide, or `None`
/// for an alarm whose every window is graded on what it holds: one not configured to ignore low
/// samples ([`LOW_SAMPLE_IGNORE`]), one whose statistic is not a percentile at all ("Used only
/// for alarms based on percentiles"), and one whose percentile no floor is published for
/// ([`significance_floor`]).
fn low_sample_floor(alarm: &MetricAlarm, statistic: &AlarmStatistic<'_>) -> Option<f64> {
    if alarm.evaluate_low_sample_count_percentile.as_deref() != Some(LOW_SAMPLE_IGNORE) {
        return None;
    }
    let AlarmStatistic::Percentile(_, percentile) = statistic else {
        return None;
    };
    significance_floor(*percentile)
}

/// What ONE evaluation counted over the slots it evaluated, which is what its `StateReason`
/// explains.
#[derive(Debug, Clone, Copy)]
struct EvaluationCounts {
    /// Datapoints whose comparison against the threshold held.
    breaching: usize,
    /// Datapoints the comparison did not hold for: the ones that satisfy the `OK` direction of
    /// the alarm's predicate.
    not_breaching: usize,
    /// Datapoints the evaluation really had to decide on: the real ones plus the ones the
    /// missing-data treatment substituted.
    present: usize,
    /// How many of those were SUBSTITUTED for a window that held no datapoint of the metric.
    substituted: usize,
    /// How many windows the look-back covered — the alarm's `EvaluationPeriods`.
    periods: usize,
    /// How many datapoints the DIRECTION this state was decided in needs: the alarm's M for
    /// `ALARM`, and its complement `N - M + 1` for `OK`. Zero for a state no direction decided,
    /// whose sentence names no minimum.
    minimum: usize,
}

/// The name the missing-data treatment is reported under in a `StateReason`, or `None` for a
/// treatment that substitutes no datapoint at all (`missing` and `ignore` leave a hole out of
/// the evaluation instead, so no arm decided anything for it).
///
/// The bracketed names are the recorded wording of the reason AWS reports for an evaluation
/// whose windows held no data; the arms themselves are the documented `TreatMissingData`
/// values (`smithy_query(query="doc",shape="MetricAlarm",member="TreatMissingData")`).
fn missing_data_arm(treat_missing: &str) -> Option<&'static str> {
    match treat_missing {
        MISSING_BREACHING => Some("Breaching"),
        MISSING_NOT_BREACHING => Some("NonBreaching"),
        _ => None,
    }
}

/// The noun suffix and verb a count takes: "1 datapoint was", "2 datapoints were".
fn plural(count: usize) -> (&'static str, &'static str) {
    if count == 1 {
        ("", "was")
    } else {
        ("s", "were")
    }
}

/// The `StateReason` a QUERY-class alarm reports while no contributor of its query is in the
/// alarm state: CloudWatch's own fixed sentence for that outcome.
///
/// Fixed text, not built from the request: what it states is that no entry of the query's
/// result vector was in `ALARM`, which names neither the query nor any series. It is the
/// explanation an alarm of this class is CREATED with, because it is created `OK`
/// (`PutMetricAlarm`'s "For PromQL alarms, the alarm state is instead immediately set to OK")
/// and this is what `OK` means for it. This engine resolves no query of that language, so no
/// evaluation of such an alarm reaches a verdict of its own
/// ([`State::metric_alarm_evaluation`]) and the explanation its write left stands.
pub const NO_CONTRIBUTOR_IN_ALARM_REASON: &str = "No time series evaluated to ALARM";

/// The `StateReason` an evaluated state reports: the DATAPOINTS the evaluation graded, how many
/// of them read the way the state says against the threshold, the minimum the transition needed
/// and the transition itself — and, where a window held no datapoint of the metric at all, that
/// the window was EMPTY and which missing-data arm was applied to it.
///
/// The sentence is the one CloudWatch publishes for an evaluated alarm:
/// `Threshold Crossed: <n> out of the last <m> datapoints [<value> (<instant>), …] was|were
/// [not] <comparison> <thresholds> (minimum <k> datapoint(s) for <from> -> <to>
/// transition).` for a graded state, and `Insufficient Data: <m> datapoint(s) was|were unknown.`
/// for a state decided with too few datapoints to grade. `<thresholds>` is what the alarm was
/// compared against as its own predicate words it — `the threshold (<threshold>)` for a number
/// and `the upper thresholds [<bound>, …]` for a band ([`Predicate::phrase`]). The state the
/// alarm moved to decides
/// whether the comparison is stated as it holds or negated, the count is of the datapoints that
/// read that way, and the closing clause names that direction and the minimum it needed
/// ([`transition_minimum`]).
///
/// The datapoints are NAMED, not just counted ([`cited_datapoints`]): the explanation of a
/// state says which values decided it and when each was measured, so a reader can tell the
/// evaluation that graded the series it published from one that graded something else.
///
/// A hole is reported as a hole: an evaluation every one of whose datapoints was substituted
/// by the missing-data treatment graded no data, so it says it received none and names the arm
/// that decided the state instead of reporting a count of datapoints it never had, and an
/// evaluation that mixed real datapoints with substituted ones names the arm beside its
/// counts.
///
/// Built from the evaluation rather than from any request: the text states the datapoints, the
/// counts, the comparison and the threshold the alarm is configured with. Inside the modelled
/// `@length 0..1023` of `StateReason` for every alarm, because the only variable parts are
/// four counts, two state names, a comparison reading of at most 24 characters, one number, and
/// a datapoint list bounded by [`MAX_CITED_DATAPOINT_CHARS`].
fn state_reason(
    state: &str,
    counts: EvaluationCounts,
    arm: Option<&str>,
    comparison: &str,
    thresholds: &str,
    cited: &str,
) -> String {
    let EvaluationCounts {
        breaching,
        not_breaching,
        present,
        substituted,
        periods,
        minimum,
    } = counts;
    if state == ALARM_STATE_INSUFFICIENT_DATA {
        // The window was short of the datapoints either direction of the predicate needs, so
        // the evaluation reports the ones it did not have: the windows of its look-back that
        // no datapoint, real or substituted, stood for.
        let unknown = periods.saturating_sub(present);
        let (plural_suffix, verb) = plural(unknown);
        return format!("Insufficient Data: {unknown} datapoint{plural_suffix} {verb} unknown.");
    }
    if let Some(arm) = arm.filter(|_| substituted > 0 && substituted == present) {
        // EVERY slot was substituted: the state was decided by the missing-data arm alone, so
        // that — and not a graded verdict over datapoints the evaluation never received — is
        // what the reason reports.
        let (periods_suffix, _) = plural(periods);
        let (datapoints_suffix, verb) = plural(substituted);
        return format!(
            "Threshold Crossed: no datapoints were received for {periods} \
             period{periods_suffix} and {substituted} missing datapoint{datapoints_suffix} \
             {verb} treated as [{arm}]."
        );
    }
    // Some real datapoint was graded. Where the treatment also substituted datapoints for
    // windows that held none, the reason names those holes and the arm applied to them beside
    // the counts.
    let substituted_clause = match arm.filter(|_| substituted > 0) {
        Some(arm) => {
            let (datapoints_suffix, verb) = plural(substituted);
            format!(
                " {substituted} missing datapoint{datapoints_suffix} {verb} treated as [{arm}]."
            )
        }
        None => String::new(),
    };
    if state == ALARM_STATE_ALARM {
        // The comparison HELD often enough, so the sentence states it as it reads and counts
        // the datapoints it held for.
        return format!(
            "Threshold Crossed: {breaching} out of the last {present} datapoints{cited} \
             {} {comparison} {thresholds} ({}).{substituted_clause}",
            plural(breaching).1,
            transition_minimum(minimum, state)
        );
    }
    // The alarm is in OK, so the comparison did not hold: the sentence states its NEGATION and
    // counts the datapoints that read that way — the ones that satisfied the direction being
    // reported, not the ones that breached.
    format!(
        "Threshold Crossed: {not_breaching} out of the last {present} datapoints{cited} {} not \
         {comparison} {thresholds} ({}).{substituted_clause}",
        plural(not_breaching).1,
        transition_minimum(minimum, state)
    )
}

/// The clause a graded reason closes with: how many datapoints the direction the state was
/// decided in needs, and that direction — `minimum 1 datapoint for OK -> ALARM transition` for a
/// state the breaching datapoints decided, and `minimum <N - M + 1> datapoints for ALARM -> OK
/// transition` for one the non-breaching ones decided.
///
/// The clause names the DIRECTION of the predicate the state stands for, not the state the alarm
/// happened to report before: the minimum belongs to the transition into the state being
/// explained, so both integers of an "M out of N" configuration are recoverable from the
/// sentence whichever state the evaluation moved out of.
fn transition_minimum(minimum: usize, state: &str) -> String {
    let (datapoints_suffix, _) = plural(minimum);
    let transition = if state == ALARM_STATE_ALARM {
        format!("{ALARM_STATE_OK} -> {ALARM_STATE_ALARM}")
    } else {
        format!("{ALARM_STATE_ALARM} -> {ALARM_STATE_OK}")
    };
    format!("minimum {minimum} datapoint{datapoints_suffix} for {transition} transition")
}

/// The most characters the datapoint list a `StateReason` cites may take.
///
/// `StateReason` is modelled `@length 0..1023` and the sentence around the list needs room
/// inside that, so the list is the part that is bounded: an alarm may evaluate as many windows
/// as [`MAX_EVALUATED_PERIODS`] and each datapoint renders a value of its own width. The
/// datapoints past the bound are left out rather than the sentence truncated, because the
/// counts and the threshold are what the state was decided on and `StateReasonData` carries
/// every evaluated datapoint anyway.
const MAX_CITED_DATAPOINT_CHARS: usize = 700;

/// The datapoints an evaluation's verdict RESTS ON, newest first, each as the value the
/// statistic took over its window and the instant that window ends: ` [137.0 (25/10/26
/// 02:02:00), …]`.
///
/// Only the datapoints on the side of the comparison the state stands for are named, because the
/// sentence around the list states the comparison the way THAT direction reads it — "`<n>` out of
/// the last `<m>` datapoints `[…]` were not greater than the threshold" for `OK`, and "were
/// greater than the threshold" for `ALARM` — so a datapoint on the other side would be cited as
/// having read the way it did not. The count the sentence opens with is of those same datapoints
/// ([`state_reason`]), and `StateReasonData` carries every datapoint the window held
/// ([`state_reason_data`]), so nothing the evaluation graded is lost by naming only these.
///
/// Empty when the verdict rests on no measured datapoint at all — a slot the missing-data
/// treatment substituted stands for a window that held nothing, so there is no measured value to
/// name for it, and a state no direction decided (insufficient data) rests on none — which is
/// also why the list is prefixed with its own space and can be dropped from the sentence without
/// leaving a gap.
fn cited_datapoints(
    evaluated: &[Evaluated],
    verdict: Option<Verdict>,
    grid: EvaluationGrid,
    now_secs: f64,
    period: f64,
) -> String {
    let mut cited = String::new();
    let Some(verdict) = verdict else {
        return cited;
    };
    for slot in evaluated {
        if slot.datapoint != verdict.satisfying {
            continue;
        }
        let Some(value) = slot.value else {
            continue;
        };
        let entry = match reason_instant(grid.window_start(now_secs, slot.period, period)) {
            Some(at) => format!("{} ({at})", decimal_text(value)),
            None => decimal_text(value),
        };
        if cited.len() + entry.len() + 2 > MAX_CITED_DATAPOINT_CHARS {
            break;
        }
        if !cited.is_empty() {
            cited.push_str(", ");
        }
        cited.push_str(&entry);
    }
    if cited.is_empty() {
        return cited;
    }
    format!(" [{cited}]")
}

/// The instant a graded datapoint is named by, in the form an alarm explanation writes one:
/// the day, the month and the last two digits of the year, then the time of day in UTC to the
/// second (`25/10/26 17:40:25`).
///
/// The calendar fields come from the modelled timestamp type's own rendering
/// ([`super::rfc3339`]) and are only re-ordered here, so no second date implementation lives
/// in this crate and an instant a sentence names is the instant a document reports. `None` for
/// an instant that type cannot render.
fn reason_instant(secs: f64) -> Option<String> {
    /// How many digits of the year the form writes.
    const YEAR_DIGITS: usize = 2;

    let rendered = super::rfc3339(secs)?;
    let (date, time) = rendered.split_once('T')?;
    let mut fields = date.split('-');
    let year = fields.next()?;
    let month = fields.next()?;
    let day = fields.next()?;
    // Whole seconds: neither the fraction a timestamp may carry nor the zone suffix is part of
    // the form.
    let time = time.split('.').next()?.trim_end_matches('Z');
    let year = year.get(year.len().saturating_sub(YEAR_DIGITS)..)?;
    Some(format!("{day}/{month}/{year} {time}"))
}

/// One measured quantity as an explanation names it: a decimal that always carries a fraction,
/// so a value and a threshold read as the measurements they are rather than as counts.
pub(crate) fn decimal_text(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        return format!("{value:.1}");
    }
    format!("{value}")
}

/// The windows ONE evaluation graded, as its decision record reports them: the grid their
/// boundaries fell on ([`EvaluationGrid`]), how wide one of them is, and how many of them the
/// alarm's look-back covered.
///
/// The three together are what every instant a record names is resolved from, so a record cannot
/// report an instant of one grid beside a span of another.
#[derive(Debug, Clone, Copy)]
struct GradedWindows {
    /// Where the boundaries between the periods fall, and whether the period the clock is in was
    /// graded at all.
    grid: EvaluationGrid,
    /// How wide one of those periods is, in seconds.
    period: f64,
    /// How many of them the verdict was made over — the alarm's `EvaluationPeriods`.
    evaluation_periods: usize,
}

impl GradedWindows {
    /// The instant the span ONE evaluation read BEGINS at, as its decision record reports it.
    ///
    /// The start of the range the alarm's evaluation window selected
    /// ([`EvaluationGrid::look_back_start`]) — a clock boundary for an aligned window, and the
    /// look-back's own width behind the query instant for a rolling one — reaching back FURTHER
    /// where a period the record names is older than that: the range an evaluation searches is
    /// deliberately wider than the look-back ([`evaluation_range_periods`]), so a verdict may
    /// rest on a datapoint of a period before it, and the span a record opens at covers every
    /// period it names.
    fn span_start(self, now_secs: f64, evaluated: &[Evaluated]) -> f64 {
        let look_back = self
            .grid
            .look_back_start(now_secs, self.evaluation_periods, self.period);
        evaluated
            .iter()
            .map(|slot| self.grid.window_start(now_secs, slot.period, self.period))
            .reduce(f64::min)
            .map_or(look_back, |oldest| oldest.min(look_back))
    }
}

/// A span of seconds as one of this service's JSON documents states it: the whole number of
/// seconds it is, and the number itself for a span that is not whole.
///
/// A modelled span of this service is an integer number of seconds (`MetricAlarm$Period`,
/// `MetricAlarm$EvaluationInterval`), so a document states it as an integer; the evaluation
/// carries it as a real number because its arithmetic over the series is real.
fn document_seconds(secs: f64) -> Value {
    /// The largest whole number of seconds the integer form holds.
    const MAX_WHOLE: f64 = 9.0e15;

    if secs.is_finite() && secs.fract() == 0.0 && secs.abs() < MAX_WHOLE {
        #[allow(clippy::cast_possible_truncation)]
        return Value::from(secs as i64);
    }
    Value::from(secs)
}

/// The `StateReasonData` an evaluated state reports: the query the evaluation made, the values
/// of the window it graded, and the periods the verdict RESTS ON — the ones that satisfied the
/// direction the state was decided in, as many as that direction needed, each named by the
/// instant of the period it stands for, with the value the statistic took over it and how many
/// observations that period held.
///
/// A period the missing-data treatment substituted a datapoint for is named by its own instant
/// too, and carries NO value and no sample count: the entry stands for a period of the
/// look-back the series held nothing over, and nothing was measured over it to report.
/// A state NO datapoint decided — insufficient data — names the periods it had no value for
/// instead, each entry carrying that period's instant and nothing else.
/// "An explanation for the alarm state, in JSON format." (`MetricAlarm$StateReasonData`),
/// bounded by [`MAX_STATE_REASON_DATA_LEN`] — a document is JSON, so it can never be
/// truncated to fit: the datapoint list is dropped instead when an alarm evaluates more
/// windows than the bound holds. Every instant inside it is written in the form a document of
/// this service stamps an instant with ([`super::document_instant`]).
///
/// The statistic is the one the graded series was FOLDED by, reported under the single
/// `statistic` member whichever of the two members the alarm configured it on, so a series a
/// query array computed — which no single statistic describes — contributes no statistic
/// member at all.
///
/// Every instant it names is the start of the period it belongs to on the grid the evaluation
/// graded ([`EvaluationGrid::window_start`]), so an alarm with an aligned window reports the
/// clock boundaries its range was aligned to, while an alarm with a rolling one opens its span
/// wherever that range has rolled to ([`GradedWindows::span_start`]).
fn state_reason_data(
    statistic: Option<&AlarmStatistic<'_>>,
    windows: GradedWindows,
    threshold: Option<f64>,
    now_secs: f64,
    evaluated: &[Evaluated],
    verdict: Option<Verdict>,
) -> String {
    let GradedWindows { grid, period, .. } = windows;
    let mut document = Map::new();
    document.insert("version".to_string(), Value::from(HISTORY_DATA_VERSION));
    // The instant the evaluation queried the series at, and the start of the span it read the
    // series over: together they say which span of the series the state was decided over. Both
    // are written in the form this service's JSON documents stamp an instant with
    // ([`super::document_instant`]). The query instant is at or after the END of the newest
    // period the document reports for a high-resolution alarm, whose rolling range lags the
    // clock by its own period, and for every alarm with an ALIGNED window, which grades only
    // periods that have closed ([`EvaluationGrid::newest_graded_period`]).
    if let Some(at) = super::document_instant(now_secs) {
        document.insert("queryDate".to_string(), Value::from(at));
    }
    if let Some(at) = super::document_instant(windows.span_start(now_secs, evaluated)) {
        document.insert("startDate".to_string(), Value::from(at));
    }
    // ONE member carries the statistic however the alarm spelled it: the name the windows were
    // folded by, whether that is one of the basic five (`MetricAlarm$Statistic`) or the
    // percentile `MetricAlarm$ExtendedStatistic` names, so a reader recovers the statistic from
    // the same place for either form.
    if let Some(statistic) = statistic {
        document.insert("statistic".to_string(), Value::from(statistic.name()));
    }
    // How wide one of the periods it graded is, as the WHOLE NUMBER of seconds a period is:
    // "The period, in seconds, over which the statistic is applied." (`MetricAlarm$Period`) is
    // modelled as an integer, so a record states it as one rather than as the real number the
    // evaluation carries its arithmetic in.
    document.insert("period".to_string(), document_seconds(period));
    // The threshold the evaluation compared every window against, for an alarm compared
    // against a number. An alarm whose threshold is a model has no one threshold to report
    // here: the bound each window was compared against moves with the series, so it is
    // reported on the datapoint it decided instead.
    if let Some(threshold) = threshold {
        document.insert("threshold".to_string(), Value::from(threshold));
    }
    // The values the evaluation really had, oldest first — the order they were observed in.
    // `evaluated` is newest first and holds a slot for every period the alarm evaluates, so
    // the slots a missing-data treatment filled contribute no value here: this list reports
    // the window the state was decided over, and `evaluatedDatapoints` below reports the
    // datapoints the verdict rests on.
    let recent: Vec<Value> = evaluated
        .iter()
        .rev()
        .filter_map(|slot| slot.value.map(Value::from))
        .collect();
    document.insert("recentDatapoints".to_string(), Value::Array(recent));

    let mut datapoints: Vec<Value> = Vec::new();
    if let Some(verdict) = verdict {
        for slot in evaluated
            .iter()
            .filter(|slot| slot.datapoint == verdict.satisfying)
            .take(verdict.minimum)
        {
            let mut entry = Map::new();
            // WHICH period the entry stands for, named by that period's own start on the grid
            // the evaluation graded: the one thing every entry carries, whether a datapoint was
            // measured over the period or a missing-data arm stood in for it.
            if let Some(at) =
                super::document_instant(grid.window_start(now_secs, slot.period, period))
            {
                entry.insert("timestamp".to_string(), Value::from(at));
            }
            // What was MEASURED over that period, for a period that held a datapoint: nothing
            // was measured over a period the treatment substituted a datapoint for, so its
            // entry names the period and reports no measurement of it.
            if let Some(count) = slot.sample_count {
                entry.insert("sampleCount".to_string(), Value::from(count));
            }
            if let Some(value) = slot.value {
                entry.insert("value".to_string(), Value::from(value));
            }
            // The threshold THIS datapoint was compared against, for an alarm whose threshold
            // moves with the series: a band bound belongs to the period it was drawn for, so a
            // reader recovers the comparison the verdict rests on from the datapoint itself.
            if let Some(threshold) = slot.threshold {
                entry.insert("threshold".to_string(), Value::from(threshold));
            }
            datapoints.push(Value::Object(entry));
        }
    } else {
        // A state decided with too few datapoints to grade rests on NO datapoint: what its
        // record names is the periods the evaluation had no value for — the ones its sentence
        // counts as unknown — one entry per unknown period, carrying that period's own instant
        // on the grid the evaluation graded and NOTHING else, because nothing was measured over
        // it. A period that really held a datapoint is named by `recentDatapoints` above and is
        // not one of these.
        let graded: Vec<usize> = evaluated
            .iter()
            .filter(|slot| slot.measured())
            .map(|slot| slot.period)
            .collect();
        let unknown = evaluated
            .len()
            .saturating_sub(evaluated.iter().filter(|slot| slot.measured()).count());
        for index in 0..evaluated.len() {
            if datapoints.len() >= unknown {
                break;
            }
            if graded.contains(&index) {
                continue;
            }
            let mut entry = Map::new();
            if let Some(at) = super::document_instant(grid.window_start(now_secs, index, period)) {
                entry.insert("timestamp".to_string(), Value::from(at));
            }
            datapoints.push(Value::Object(entry));
        }
    }
    let mut full = document.clone();
    full.insert("evaluatedDatapoints".to_string(), Value::Array(datapoints));
    let rendered = Value::Object(full).to_string();
    if rendered.len() <= MAX_STATE_REASON_DATA_LEN {
        return rendered;
    }
    Value::Object(document).to_string()
}
