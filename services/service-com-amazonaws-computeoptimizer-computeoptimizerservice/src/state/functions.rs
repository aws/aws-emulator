// SPDX-License-Identifier: CC0-1.0

//! The AWS Lambda function versions Compute Optimizer observes, and the memory-size
//! recommendation a reader derives from that observation.
//!
//! Part of [`super`]'s one state module, in its own file so one concern lives in one place.

use super::*;

/// The function version Lambda gives the unpublished version of a function, and the one
/// Compute Optimizer analyses for an unqualified ARN: "if you specify an unqualified ARN
/// without a function version suffix, Compute Optimizer will return recommendations for
/// the latest ($LATEST) version of the function"
/// (`GetLambdaFunctionRecommendations$functionArns`).
pub const LATEST_FUNCTION_VERSION: &str = "$LATEST";

/// The `LambdaFunctionRecommendationFilterName` values
/// ([`LAMBDA_FUNCTION_RECOMMENDATION_FILTER_NAME_VALUES`]), named so the filter rules below
/// read as the model's own vocabulary.
pub const LAMBDA_FILTER_FINDING: &str = "Finding";
/// See [`LAMBDA_FILTER_FINDING`].
pub const LAMBDA_FILTER_FINDING_REASON_CODE: &str = "FindingReasonCode";

/// The `LambdaFunctionRecommendationFinding` value outside the service-wide `Finding`
/// vocabulary: "Compute Optimizer was unable to generate a recommendation for the
/// function". Its other two values are [`FINDING_OPTIMIZED`] and [`FINDING_NOT_OPTIMIZED`].
pub const LAMBDA_FINDING_UNAVAILABLE: &str = "Unavailable";

/// The `LambdaFunctionRecommendationFindingReasonCode` values of the `Unavailable` finding:
/// `InsufficientData` — "the function does not have sufficient metric data for Compute
/// Optimizer to generate a recommendation" — and `Inconclusive` — "the function does not
/// qualify for a recommendation because Compute Optimizer cannot generate a recommendation
/// with a high degree of confidence". The two codes of the `NotOptimized` finding are
/// [`METRIC_REASON_MEMORY_OVERPROVISIONED`] and [`METRIC_REASON_MEMORY_UNDERPROVISIONED`].
pub const LAMBDA_REASON_INSUFFICIENT_DATA: &str = "InsufficientData";
/// See [`LAMBDA_REASON_INSUFFICIENT_DATA`].
pub const LAMBDA_REASON_INCONCLUSIVE: &str = "Inconclusive";

/// The `LambdaFunctionMetricName` values
/// (`crate::ops::types::LAMBDA_FUNCTION_METRIC_NAME_VALUES`): the duration a function's
/// code spends processing an event, and the memory it uses doing so.
pub const LAMBDA_METRIC_DURATION: &str = "Duration";
/// See [`LAMBDA_METRIC_DURATION`].
pub const LAMBDA_METRIC_MEMORY: &str = "Memory";

/// The statistic value beside [`METRIC_STATISTIC_MAXIMUM`] that this API's metric-statistic
/// vocabularies declare (`crate::ops::types::METRIC_STATISTIC_VALUES` and
/// `crate::ops::types::LAMBDA_FUNCTION_METRIC_STATISTIC_VALUES` are both `Average` |
/// `Maximum`): the other statistic an analysis records an observation under, and so the other
/// one a reader of projected metrics can ask for.
pub const METRIC_STATISTIC_AVERAGE: &str = "Average";

/// The `LambdaFunctionMemoryMetricStatistic` values
/// (`crate::ops::types::LAMBDA_FUNCTION_MEMORY_METRIC_STATISTIC_VALUES`): the range a
/// recommendation option projects the function's duration into, and the value expected
/// inside it.
pub const PROJECTED_STATISTIC_LOWER_BOUND: &str = "LowerBound";
/// See [`PROJECTED_STATISTIC_LOWER_BOUND`].
pub const PROJECTED_STATISTIC_UPPER_BOUND: &str = "UpperBound";
/// See [`PROJECTED_STATISTIC_LOWER_BOUND`].
pub const PROJECTED_STATISTIC_EXPECTED: &str = "Expected";

/// The memory a Lambda function can be configured with, from the AWS Lambda quotas: a
/// function's memory is "128 MB to 10,240 MB, in 1-MB increments", so no memory size this
/// service recommends falls outside that range or carries a fraction of a MB.
pub const LAMBDA_MIN_MEMORY_MB: i32 = 128;
/// See [`LAMBDA_MIN_MEMORY_MB`].
pub const LAMBDA_MAX_MEMORY_MB: i32 = 10_240;

/// How many invocations the analysed period has to hold for Compute Optimizer to reach a
/// finding for a function, from the "Supported resources and requirements" page the
/// `InsufficientData` reason code links: a Lambda function qualifies for a recommendation
/// once it has been invoked at least 50 times over the look-back period. A function below
/// that is reported `Unavailable` / `InsufficientData` rather than sized from the little
/// that was observed.
pub const LAMBDA_MIN_ANALYSED_INVOCATIONS: i64 = 50;

/// AWS Lambda public pricing in the `us-east-1` Region for the x86_64 architecture, as the
/// Lambda pricing page lists it: $0.0000166667 per GB-second of billed duration plus $0.20
/// per 1 million requests. Every cost and saving a function recommendation reports is
/// computed from these two prices, so its figures agree with each other.
pub const LAMBDA_PRICE_PER_GB_SECOND: f64 = 0.000_016_666_7;
/// See [`LAMBDA_PRICE_PER_GB_SECOND`].
pub const LAMBDA_PRICE_PER_REQUEST: f64 = 0.000_000_2;

/// MB in a GB: a function's memory is configured in MB and Lambda bills GB-seconds.
pub const MB_PER_GB: f64 = 1_024.0;

/// Milliseconds in a second: Lambda bills duration in GB-SECONDS and the `Duration` metric
/// is milliseconds.
pub const MILLISECONDS_PER_SECOND: f64 = 1_000.0;

/// Days in a month, so a rate observed per day turns into the monthly figures Compute
/// Optimizer reports. Derived from [`HOURS_PER_MONTH`] rather than stated again, so one
/// month means the same length everywhere in this service.
pub fn days_per_month() -> f64 {
    HOURS_PER_MONTH / 24.0
}

/// A Lambda function ARN — `arn:<partition>:lambda:<region>:<account>:function:<name>` with
/// an optional `:<version>` suffix — split into the parts the reader needs, or `None` for
/// anything that is not one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedFunctionArn<'a> {
    /// The ARN's account-id field: the account that owns the function.
    pub account_id: &'a str,
    /// The ARN's region field: the Region the function is in, which a regional service
    /// answers for only when the request was signed for it.
    pub region: &'a str,
    /// The function's name.
    pub function_name: &'a str,
    /// The version the ARN names, or [`LATEST_FUNCTION_VERSION`] for an UNQUALIFIED ARN:
    /// "if you specify an unqualified ARN without a function version suffix, Compute
    /// Optimizer will return recommendations for the latest ($LATEST) version of the
    /// function. If you specify a qualified ARN with a version suffix, Compute Optimizer
    /// will return recommendations for the specified function version."
    pub function_version: &'a str,
}

/// Parse a Lambda function ARN, or `None` for anything that is not one. See
/// [`ParsedFunctionArn`].
pub fn parse_function_arn(arn: &str) -> Option<ParsedFunctionArn<'_>> {
    let parsed = parse_arn(arn)?;
    if parsed.service != "lambda" {
        return None;
    }
    let mut resource = parsed.resource.splitn(3, ':');
    if resource.next() != Some("function") {
        return None;
    }
    let function_name = resource.next().filter(|name| !name.is_empty())?;
    let function_version = resource
        .next()
        .filter(|version| !version.is_empty())
        .unwrap_or(LATEST_FUNCTION_VERSION);
    Some(ParsedFunctionArn {
        account_id: parsed.account_id,
        region: parsed.region,
        function_name,
        function_version,
    })
}

/// One interval of the period Compute Optimizer analyses a function over, as CloudWatch
/// aggregates the `AWS/Lambda` series AWS Lambda publishes for the function version: how
/// often it ran in that interval, and how long an invocation took (shortest, mean and
/// longest).
///
/// Read out of the metric store by [`super::lambda_inventory`], which is the only thing that
/// builds one: every figure here is an observation AWS Lambda published, never a value this
/// service chose.
#[derive(Debug, Clone, Copy, Default)]
pub struct FunctionSample {
    /// How many days before the recommendation's last refresh this interval was observed,
    /// so the look-back period in force decides whether it is analysed at all.
    pub days_ago: f64,
    /// How many times the function was invoked in the interval, which is what
    /// `LambdaFunctionRecommendation$numberOfInvocations` totals.
    pub invocations: i64,
    /// The shortest invocation of the interval, in milliseconds.
    pub duration_ms_minimum: f64,
    /// The mean invocation of the interval, in milliseconds.
    pub duration_ms_average: f64,
    /// The longest invocation of the interval, in milliseconds.
    pub duration_ms_maximum: f64,
}

/// What ONE invocation's run report states it used: the memory the run peaked at, in MB.
///
/// AWS Lambda publishes no memory metric for a function — the memory an invocation really
/// used is stated by the report that closes the run, in the function's own log group — so the
/// memory side of a function's analysis is a list of these, one per run observed, read out of
/// the log store by [`super::lambda_inventory`].
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryReport {
    /// How many days before the recommendation's last refresh the run was reported, so the
    /// look-back period in force decides whether it is analysed at all.
    pub days_ago: f64,
    /// The memory the run used at its peak, in MB — the report's own `Max Memory Used`.
    pub used_mb: f64,
}

/// One Lambda function version Compute Optimizer observes: how it is configured, its tags,
/// and the observations a recommendation for it is computed from. This is the OBSERVATION;
/// everything a `LambdaFunctionRecommendation` reports is derived from it, so no two readers
/// of the same function can disagree.
///
/// It is built from the account's OWN inventory — the function AWS Lambda holds, the
/// `AWS/Lambda` series published for it and the run reports in its log group
/// ([`super::lambda_inventory::analysed_functions`], the one selection every Lambda function
/// surface of this service reads) — so a function this service reports is a function the
/// account really has.
#[derive(Debug, Clone)]
pub struct ObservedFunction {
    /// The function's name, which with [`Self::function_version`] is its key in the
    /// inventory. The ARN a reader reports is built from them and the region the request was
    /// signed for ([`Self::function_arn`]), because Compute Optimizer is a REGIONAL service:
    /// the functions it reports are the ones in the Region the caller addressed.
    pub function_name: String,
    /// "The version number of the current function"
    /// (`LambdaFunctionRecommendation$functionVersion`) — a published version number, or
    /// [`LATEST_FUNCTION_VERSION`] for the unpublished one an unqualified ARN names.
    pub function_version: String,
    /// The account that owns the function.
    pub account_id: String,
    /// "The amount of memory, in MB, that's allocated to the current function"
    /// (`LambdaFunctionRecommendation$currentMemorySize`) — the memory AWS Lambda holds this
    /// version configured with.
    pub configured_memory_mb: i32,
    /// How long an invocation of this version may run before AWS Lambda stops it, in seconds —
    /// the `Timeout` that service holds the version configured with. No member of a
    /// `LambdaFunctionRecommendation` carries it, and the `CurrentConfigurationTimeout`
    /// exportable field is a column of the function's own configuration, so it is observed with
    /// the memory size beside it.
    pub configured_timeout_seconds: i32,
    /// The function's tags, as key/value pairs, in the order Lambda reports them.
    pub tags: Vec<(String, String)>,
    /// The `AWS/Lambda` samples observed for this version, oldest first.
    pub samples: Vec<FunctionSample>,
    /// The memory the version's observed runs reported using, one entry per run.
    pub memory_reports: Vec<MemoryReport>,
}

impl ObservedFunction {
    /// This function's identity WITHOUT the region — the resource part of its ARN, which is
    /// also its key in the inventory. A published version is a QUALIFIED ARN
    /// (`function:<name>:<version>`); the unpublished `$LATEST` version is the unqualified
    /// one (`function:<name>`), the form a request may name it by.
    pub fn arn_resource(&self) -> String {
        if self.function_version == LATEST_FUNCTION_VERSION {
            format!("function:{}", self.function_name)
        } else {
            format!("function:{}:{}", self.function_name, self.function_version)
        }
    }

    /// This function's ARN in `region`:
    /// `arn:aws:lambda:<region>:<account>:function:<name>[:<version>]`.
    pub fn function_arn(&self, region: &str) -> String {
        if self.account_id == ::engine::emulator_account_id() {
            ::engine::emulator_arn("lambda", region, &self.arn_resource())
        } else {
            format!(
                "arn:aws:lambda:{region}:{}:{}",
                self.account_id,
                self.arn_resource()
            )
        }
    }

    /// The samples the look-back period in force analyses: those observed within
    /// `look_back_days` of the last refresh.
    pub(super) fn analysed_samples(&self, look_back_days: f64) -> impl Iterator<Item = &FunctionSample> {
        self.samples
            .iter()
            .filter(move |sample| sample.days_ago <= look_back_days)
    }

    /// "The number of days for which utilization metrics were analyzed for the function":
    /// the look-back period in force for it ([`look_back_period_in_days`]), which is the
    /// window the samples and run reports analysed are taken from.
    pub fn analysed_days(&self, look_back_period: &str) -> f64 {
        look_back_period_in_days(look_back_period)
    }

    /// The run reports the look-back period in force analyses: those reported within
    /// `look_back_days` of the last refresh.
    pub(super) fn analysed_memory_reports(
        &self,
        look_back_days: f64,
    ) -> impl Iterator<Item = &MemoryReport> {
        self.memory_reports
            .iter()
            .filter(move |report| report.days_ago <= look_back_days)
    }

    /// "The number of times your function code was applied during the look-back period"
    /// (`LambdaFunctionRecommendation$numberOfInvocations`).
    pub fn invocations(&self, look_back_days: f64) -> i64 {
        self.analysed_samples(look_back_days)
            .map(|sample| sample.invocations.max(0))
            .sum()
    }

    /// The highest of one sample measurement over the analysed period, or `0.0` when the
    /// period holds no sample.
    pub(super) fn peak(&self, look_back_days: f64, of: fn(&FunctionSample) -> f64) -> f64 {
        self.analysed_samples(look_back_days)
            .map(of)
            .fold(0.0_f64, f64::max)
    }

    /// The lowest of one sample measurement over the analysed period, or `0.0` when the
    /// period holds no sample.
    pub(super) fn trough(&self, look_back_days: f64, of: fn(&FunctionSample) -> f64) -> f64 {
        let lowest = self
            .analysed_samples(look_back_days)
            .map(of)
            .fold(f64::INFINITY, f64::min);
        if lowest.is_finite() {
            lowest.max(0.0)
        } else {
            0.0
        }
    }

    /// The mean of one sample measurement over the analysed period, each sample weighted by
    /// the invocations it covers — the mean OVER INVOCATIONS, which is what a per-invocation
    /// average means. `0.0` for a period that holds no invocation.
    pub(super) fn mean_per_invocation(&self, look_back_days: f64, of: fn(&FunctionSample) -> f64) -> f64 {
        let mut invocations = 0.0_f64;
        let mut total = 0.0_f64;
        for sample in self.analysed_samples(look_back_days) {
            let weight = sample.invocations.max(0) as f64;
            invocations += weight;
            total += weight * of(sample);
        }
        if invocations > 0.0 {
            total / invocations
        } else {
            0.0
        }
    }

    /// The memory an invocation used at its peak over the analysed period, in MB — the
    /// demand the function's configured memory has to carry. `0.0` for a period whose runs
    /// reported none, which is the `Inconclusive` case [`Self::finding`] reports.
    pub fn peak_memory_mb(&self, look_back_days: f64) -> f64 {
        self.analysed_memory_reports(look_back_days)
            .map(|report| report.used_mb)
            .fold(0.0_f64, f64::max)
    }

    /// The memory an invocation used on average over the analysed period, in MB. Each report
    /// is ONE run, so the mean over the reports is the mean per invocation.
    pub fn average_memory_mb(&self, look_back_days: f64) -> f64 {
        let mut runs = 0.0_f64;
        let mut total = 0.0_f64;
        for report in self.analysed_memory_reports(look_back_days) {
            runs += 1.0;
            total += report.used_mb;
        }
        if runs > 0.0 {
            total / runs
        } else {
            0.0
        }
    }

    /// The mean duration of an invocation over the analysed period, in milliseconds.
    pub fn average_duration_ms(&self, look_back_days: f64) -> f64 {
        self.mean_per_invocation(look_back_days, |sample| sample.duration_ms_average)
    }

    /// The metrics `LambdaFunctionRecommendation$utilizationMetrics` reports: each
    /// `LambdaFunctionMetricName` at each `LambdaFunctionMetricStatistic`, in the order the
    /// model's own `ExportableLambdaFunctionField` names them (`DurationAverage`,
    /// `DurationMaximum`, `MemoryAverage`, `MemoryMaximum`). Every value is computed from
    /// the analysed samples, so the metrics a recommendation carries are the observation the
    /// rest of it was derived from.
    pub fn utilization_metrics(&self, look_back_days: f64) -> Vec<LambdaFunctionUtilizationMetric> {
        let metric = |name: &str, statistic: &str, value: f64| LambdaFunctionUtilizationMetric {
            name: Some(name.to_string()),
            statistic: Some(statistic.to_string()),
            value: round_hundredths(value),
        };
        vec![
            metric(
                LAMBDA_METRIC_DURATION,
                METRIC_STATISTIC_AVERAGE,
                self.average_duration_ms(look_back_days),
            ),
            metric(
                LAMBDA_METRIC_DURATION,
                METRIC_STATISTIC_MAXIMUM,
                self.peak(look_back_days, |sample| sample.duration_ms_maximum),
            ),
            metric(
                LAMBDA_METRIC_MEMORY,
                METRIC_STATISTIC_AVERAGE,
                self.average_memory_mb(look_back_days),
            ),
            metric(
                LAMBDA_METRIC_MEMORY,
                METRIC_STATISTIC_MAXIMUM,
                self.peak_memory_mb(look_back_days),
            ),
        ]
    }

    /// The memory size this service recommends for the function, in MB: enough to carry the
    /// peak an invocation used over the analysed period with [`RECOMMENDATION_HEADROOM`] to
    /// spare, inside the range Lambda allows ([`LAMBDA_MIN_MEMORY_MB`]) and in whole MB.
    pub fn recommended_memory_mb(&self, look_back_days: f64) -> i32 {
        let required = self.peak_memory_mb(look_back_days) * (1.0 + RECOMMENDATION_HEADROOM);
        (required.ceil() as i32).clamp(LAMBDA_MIN_MEMORY_MB, LAMBDA_MAX_MEMORY_MB)
    }

    /// The function's `LambdaFunctionRecommendationFinding` over the analysed period.
    ///
    /// `Unavailable` while the analysis has nothing to size the function from — fewer than
    /// [`LAMBDA_MIN_ANALYSED_INVOCATIONS`] invocations, or no memory utilization observed at
    /// all. Otherwise `NotOptimized` exactly when the memory the workload asks for
    /// ([`Self::recommended_memory_mb`]) is not the memory the function is configured with,
    /// because then "its memory configuration can be sized down while still meeting the
    /// performance requirements" or "doesn't meet the performance requirements of the
    /// workload"; `Optimized` when it is — "the function is correctly provisioned to run
    /// your workload based on its current configuration and its utilization history".
    pub fn finding(&self, look_back_days: f64) -> &'static str {
        if self.invocations(look_back_days) < LAMBDA_MIN_ANALYSED_INVOCATIONS
            || self.peak_memory_mb(look_back_days) <= 0.0
        {
            return LAMBDA_FINDING_UNAVAILABLE;
        }
        if self.recommended_memory_mb(look_back_days) == self.configured_memory_mb {
            FINDING_OPTIMIZED
        } else {
            FINDING_NOT_OPTIMIZED
        }
    }

    /// "The reason for the finding classification of the function": the code of the finding
    /// [`Self::finding`] reached, and NOTHING for an optimized function — "functions that
    /// have a finding classification of `Optimized` don't have a finding reason code".
    pub fn finding_reason_codes(&self, look_back_days: f64) -> Vec<String> {
        if self.invocations(look_back_days) < LAMBDA_MIN_ANALYSED_INVOCATIONS {
            return vec![LAMBDA_REASON_INSUFFICIENT_DATA.to_string()];
        }
        if self.peak_memory_mb(look_back_days) <= 0.0 {
            return vec![LAMBDA_REASON_INCONCLUSIVE.to_string()];
        }
        match self
            .recommended_memory_mb(look_back_days)
            .cmp(&self.configured_memory_mb)
        {
            ::std::cmp::Ordering::Less => vec![METRIC_REASON_MEMORY_OVERPROVISIONED.to_string()],
            ::std::cmp::Ordering::Greater => vec![METRIC_REASON_MEMORY_UNDERPROVISIONED.to_string()],
            ::std::cmp::Ordering::Equal => Vec::new(),
        }
    }

    /// The `CurrentPerformanceRisk` the function's configured memory runs: how close the
    /// peak an invocation used came to the memory it is allowed, on the service's one risk
    /// scale ([`performance_risk_score`]).
    pub fn current_performance_risk(&self, look_back_days: f64) -> &'static str {
        performance_risk_band(performance_risk_score(
            self.peak_memory_mb(look_back_days),
            f64::from(self.configured_memory_mb.max(0)),
        ))
    }

    /// The duration an invocation that took `observed_ms` would take at the memory size a
    /// recommendation option names.
    ///
    /// The analysis observes this function at ONE memory size — the one it is configured with
    /// — so a duration at another size is not something it has observed: the projection is
    /// the duration observed, rather than a speed-up or a slow-down computed from no
    /// observation of the function at that size.
    pub fn projected_duration_ms(&self, observed_ms: f64) -> f64 {
        round_hundredths(observed_ms.max(0.0))
    }

    /// The projected metrics a recommendation option carries: the duration range the
    /// function runs in, from the shortest, the mean and the longest invocation the analysed
    /// period observed. `LambdaFunctionMemoryMetricName` declares one metric — `Duration` —
    /// so each statistic is reported for it.
    pub fn projected_utilization_metrics(
        &self,
        look_back_days: f64,
    ) -> Vec<LambdaFunctionMemoryProjectedMetric> {
        let metric = |statistic: &str, observed_ms: f64| LambdaFunctionMemoryProjectedMetric {
            name: Some(LAMBDA_METRIC_DURATION.to_string()),
            statistic: Some(statistic.to_string()),
            value: self.projected_duration_ms(observed_ms),
        };
        vec![
            metric(
                PROJECTED_STATISTIC_LOWER_BOUND,
                self.trough(look_back_days, |sample| sample.duration_ms_minimum),
            ),
            metric(
                PROJECTED_STATISTIC_EXPECTED,
                self.average_duration_ms(look_back_days),
            ),
            metric(
                PROJECTED_STATISTIC_UPPER_BOUND,
                self.peak(look_back_days, |sample| sample.duration_ms_maximum),
            ),
        ]
    }

    /// How many times the function runs in a month at the rate the analysed period observed.
    pub fn monthly_invocations(&self, look_back_days: f64) -> f64 {
        if look_back_days <= 0.0 {
            return 0.0;
        }
        self.invocations(look_back_days) as f64 / look_back_days * days_per_month()
    }

    /// What `invocations` invocations of `duration_ms` at `memory_mb` bill in USD: the
    /// GB-seconds that many invocations of that length at that memory bill, plus the
    /// per-request charge, at [`LAMBDA_PRICE_PER_GB_SECOND`] and
    /// [`LAMBDA_PRICE_PER_REQUEST`].
    ///
    /// The ONE costing of this analysis, so the monthly figures a recommendation's savings are
    /// computed from and the analysed period's own cost cannot price the same run differently.
    fn cost_of(invocations: f64, memory_mb: i32, duration_ms: f64) -> f64 {
        let invocations = invocations.max(0.0);
        let gb_seconds = invocations
            * (duration_ms.max(0.0) / MILLISECONDS_PER_SECOND)
            * (f64::from(memory_mb.max(0)) / MB_PER_GB);
        round_hundredths(
            gb_seconds * LAMBDA_PRICE_PER_GB_SECOND + invocations * LAMBDA_PRICE_PER_REQUEST,
        )
    }

    /// What a month of this function costs in USD at `memory_mb` with a mean invocation of
    /// `duration_ms`: the invocations a month holds at the observed rate, costed by
    /// [`Self::cost_of`].
    pub fn monthly_cost(&self, memory_mb: i32, duration_ms: f64, look_back_days: f64) -> f64 {
        Self::cost_of(
            self.monthly_invocations(look_back_days),
            memory_mb,
            duration_ms,
        )
    }

    /// What the function cost over the WHOLE analysed period as it is configured today, in
    /// USD: the invocations that period observed, at the memory the version is configured with
    /// and the mean duration they took.
    ///
    /// The period's total rather than a month's, because it is the cost of what was actually
    /// observed; the savings a recommendation option reports stay the monthly figures the
    /// modelled `EstimatedMonthlySavings` is.
    pub fn analysed_cost(&self, look_back_days: f64) -> f64 {
        Self::cost_of(
            self.invocations(look_back_days) as f64,
            self.configured_memory_mb,
            self.average_duration_ms(look_back_days),
        )
    }

    /// The mean cost of ONE DAY of the analysed period, in USD: the period's own cost
    /// ([`Self::analysed_cost`]) spread over the days it covers, and nothing for a period of
    /// no days at all.
    pub fn average_daily_cost(&self, look_back_days: f64) -> f64 {
        if look_back_days <= 0.0 {
            return 0.0;
        }
        round_hundredths(self.analysed_cost(look_back_days) / look_back_days)
    }

    /// What the analysed period's invocations would have cost at the memory size this service
    /// recommends, at the SHORTEST and at the LONGEST invocation that period observed: the
    /// range the option's cost falls in, bracketing the cost its savings are computed from.
    ///
    /// The bounds are the durations the option's own projected metrics report
    /// ([`Self::projected_utilization_metrics`]), so the range and those metrics are two
    /// readings of one observation.
    pub fn recommended_cost_bounds(&self, look_back_days: f64) -> (f64, f64) {
        let memory_mb = self.recommended_memory_mb(look_back_days);
        let invocations = self.invocations(look_back_days) as f64;
        let cost = |observed_ms: f64| {
            Self::cost_of(
                invocations,
                memory_mb,
                self.projected_duration_ms(observed_ms),
            )
        };
        (
            cost(self.trough(look_back_days, |sample| sample.duration_ms_minimum)),
            cost(self.peak(look_back_days, |sample| sample.duration_ms_maximum)),
        )
    }

    /// What the function costs per month as it is configured today.
    pub fn current_monthly_cost(&self, look_back_days: f64) -> f64 {
        self.monthly_cost(
            self.configured_memory_mb,
            self.average_duration_ms(look_back_days),
            look_back_days,
        )
    }

    /// What the function would cost per month at the recommended memory size: the same
    /// invocations, at the duration the analysis projects for them
    /// ([`Self::projected_duration_ms`]), billed over that memory size.
    pub fn recommended_monthly_cost(&self, look_back_days: f64) -> f64 {
        let memory_mb = self.recommended_memory_mb(look_back_days);
        self.monthly_cost(
            memory_mb,
            self.projected_duration_ms(self.average_duration_ms(look_back_days)),
            look_back_days,
        )
    }

    /// What adopting the recommended memory size saves per month, and that saving "as a
    /// percentage of monthly cost" (`SavingsOpportunity$savingsOpportunityPercentage`). A
    /// recommendation that costs the same or more saves NOTHING rather than a negative
    /// amount — an under-provisioned function is recommended MORE memory.
    pub fn savings(&self, look_back_days: f64) -> (f64, f64) {
        let current = self.current_monthly_cost(look_back_days);
        let saved = round_hundredths((current - self.recommended_monthly_cost(look_back_days)).max(0.0));
        (percentage_of(saved, current), saved)
    }

    /// The function recommendation this observation yields in `region` under
    /// `look_back_period`, as generated at `last_refresh_timestamp`.
    ///
    /// Every figure is computed here from the stored configuration and samples. A memory
    /// size recommendation option is reported exactly when the analysis reached a finding
    /// for the function: an `Unavailable` function has no memory size to move to, because
    /// "Compute Optimizer was unable to generate a recommendation" for it.
    /// `after_discounts` is whether the account activated the savings-estimation-mode
    /// preference for the function ([`EffectivePreferences::after_discounts`]), which is what
    /// decides whether the savings member net of discounts is reported at all.
    pub fn to_recommendation(
        &self,
        region: &str,
        look_back_period: &str,
        after_discounts: bool,
        last_refresh_timestamp: Option<DateTime>,
    ) -> LambdaFunctionRecommendation {
        let look_back_days = self.analysed_days(look_back_period);
        let finding = self.finding(look_back_days);
        let reason_codes = self.finding_reason_codes(look_back_days);
        let memory_mb = self.recommended_memory_mb(look_back_days);
        let (percentage, value) = self.savings(look_back_days);

        LambdaFunctionRecommendation {
            function_arn: Some(self.function_arn(region)),
            function_version: Some(self.function_version.clone()),
            account_id: Some(self.account_id.clone()),
            current_memory_size: self.configured_memory_mb,
            number_of_invocations: self.invocations(look_back_days),
            utilization_metrics: Some(self.utilization_metrics(look_back_days)),
            lookback_period_in_days: look_back_days,
            last_refresh_timestamp,
            finding: Some(finding.to_string()),
            finding_reason_codes: (!reason_codes.is_empty()).then_some(reason_codes),
            memory_size_recommendation_options: (finding != LAMBDA_FINDING_UNAVAILABLE).then(|| {
                vec![LambdaFunctionMemoryRecommendationOption {
                    // One option, so it is the first-ranked one: "rank 1 is considered the
                    // best" wherever this service ranks a recommendation option.
                    rank: 1,
                    memory_size: memory_mb,
                    projected_utilization_metrics: Some(
                        self.projected_utilization_metrics(look_back_days),
                    ),
                    savings_opportunity: Some(SavingsOpportunity {
                        savings_opportunity_percentage: percentage,
                        estimated_monthly_savings: Some(EstimatedMonthlySavings {
                            currency: Some(CURRENCY_USD.to_string()),
                            value,
                        }),
                    }),
                    // The savings net of Savings Plans and Reserved Instances discounts are
                    // reported only once the account activates the preference that "activates"
                    // them, the one gate every recommendation surface of this service applies.
                    // This emulator prices a function from the public price table in
                    // [`LAMBDA_PRICE_PER_GB_SECOND`] and records no negotiated discount for
                    // Lambda, so where the member is reported its figures are the
                    // before-discount ones — which is what the reported
                    // [`SAVINGS_ESTIMATION_SOURCE`] says.
                    savings_opportunity_after_discounts: after_discounts.then(|| {
                        LambdaSavingsOpportunityAfterDiscounts {
                            savings_opportunity_percentage: percentage,
                            estimated_monthly_savings: Some(LambdaEstimatedMonthlySavings {
                                currency: Some(CURRENCY_USD.to_string()),
                                value,
                            }),
                        }
                    }),
                }]
            }),
            current_performance_risk: Some(self.current_performance_risk(look_back_days).to_string()),
            // `LambdaEffectiveRecommendationPreferences` carries one preference: where the
            // money figures came from ([`SAVINGS_ESTIMATION_SOURCE`]).
            effective_recommendation_preferences: Some(LambdaEffectiveRecommendationPreferences {
                savings_estimation_mode: Some(LambdaSavingsEstimationMode {
                    source: Some(SAVINGS_ESTIMATION_SOURCE.to_string()),
                }),
            }),
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| Tag {
                        key: Some(key.clone()),
                        value: Some(value.clone()),
                    })
                    .collect()
            }),
        }
    }

    /// This function as a row of the findings catalog, i.e. as `GetRecommendationSummaries`
    /// aggregates it — or `None` for a function no summary can count.
    ///
    /// The projection is what keeps one function's analysis in ONE place: the summary reader
    /// and `GetLambdaFunctionRecommendations` both answer from this observation, so they
    /// cannot disagree about a function's finding, its performance risk or what adopting its
    /// recommendation saves. A function whose finding is `Unavailable` is NOT a row: the
    /// findings a summary of Lambda functions reports are the number of them "that are
    /// NotOptimized, or Optimized" and `Summary$name` cannot even carry `Unavailable`, so
    /// such a function is counted at no finding — matching the per-function reader, which
    /// withholds it unless a request asks for it by name.
    pub fn to_analysed_resource(&self, look_back_period: &str) -> Option<AnalysedResource> {
        let look_back_days = self.analysed_days(look_back_period);
        let finding = self.finding(look_back_days);
        if !findings_reported_for(SOURCE_TYPE_LAMBDA_FUNCTION)
            .iter()
            .any(|(stored, _)| *stored == finding)
        {
            return None;
        }
        Some(AnalysedResource {
            account_id: self.account_id.clone(),
            resource_type: SOURCE_TYPE_LAMBDA_FUNCTION.to_string(),
            arn_service: "lambda".to_string(),
            arn_resource: self.arn_resource(),
            finding: finding.to_string(),
            finding_reason_codes: self.finding_reason_codes(look_back_days),
            current_performance_risk: self.current_performance_risk(look_back_days).to_string(),
            current_monthly_cost: self.current_monthly_cost(look_back_days),
            recommended_monthly_cost: self.recommended_monthly_cost(look_back_days),
            // Compute Optimizer infers workload types for EC2 instances, not for functions,
            // and a function recommendation declares no such member.
            inferred_workload_types: Vec::new(),
            // A function is not a database, so there is no database configuration to record.
            rds_database: None,
        })
    }
}

/// Check a `LambdaFunctionRecommendationFilters` list against the vocabulary the model
/// declares for it, returning the reason it is not valid.
///
/// `name` is a `LambdaFunctionRecommendationFilterName` and the values a filter carries are
/// the value set of the member that name selects on: "specify `Optimized`, `NotOptimized`,
/// or `Unavailable` if you specify the `name` parameter as `Finding`"; "specify
/// `MemoryOverprovisioned`, `MemoryUnderprovisioned`, `InsufficientData`, or `Inconclusive`
/// if you specify the `name` parameter as `FindingReasonCode`". A name or a value outside
/// those sets is refused rather than silently matching nothing — the same rule
/// [`rds_filter_rejection`] applies to the other findings readers' filters.
pub fn lambda_filter_rejection(filters: &[LambdaFunctionRecommendationFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                LAMBDA_FUNCTION_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let Some(vocabulary) = lambda_filter_vocabulary(name) else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {name} is not a filter \
                 name, which must be one of [{}].",
                LAMBDA_FUNCTION_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// The value set a `LambdaFunctionRecommendationFilterName` selects on, or `None` for a name
/// the model does not declare.
pub(super) fn lambda_filter_vocabulary(name: &str) -> Option<&'static [&'static str]> {
    match name {
        LAMBDA_FILTER_FINDING => Some(LAMBDA_FUNCTION_RECOMMENDATION_FINDING_VALUES),
        LAMBDA_FILTER_FINDING_REASON_CODE => {
            Some(LAMBDA_FUNCTION_RECOMMENDATION_FINDING_REASON_CODE_VALUES)
        }
        _ => None,
    }
}

/// Does `recommendation` satisfy every `GetLambdaFunctionRecommendations` filter?
///
/// A filter matches when the member its name selects on equals any of the filter's values —
/// for the reason codes, which are a LIST, when any reported code is among them. Several
/// filters are ANDed, and a filter carrying no values constrains nothing. A member the
/// recommendation does not carry matches nothing. An unrecognised name never reaches here:
/// [`lambda_filter_rejection`] refuses the request first.
pub fn lambda_recommendation_matches_filters(
    recommendation: &LambdaFunctionRecommendation,
    filters: &[LambdaFunctionRecommendationFilter],
) -> bool {
    filters.iter().all(|filter| {
        let Some(values) = filter.values.as_ref().filter(|values| !values.is_empty()) else {
            return true;
        };
        match filter.name.as_deref() {
            Some(LAMBDA_FILTER_FINDING) => recommendation
                .finding
                .as_deref()
                .is_some_and(|finding| values.iter().any(|value| value == finding)),
            Some(LAMBDA_FILTER_FINDING_REASON_CODE) => {
                recommendation
                    .finding_reason_codes
                    .as_ref()
                    .is_some_and(|codes| {
                        codes
                            .iter()
                            .any(|code| values.iter().any(|value| value == code))
                    })
            }
            _ => false,
        }
    })
}

/// Whether a request's filters ask for the functions the analysis reached no finding for.
///
/// "Functions with a finding of unavailable are not returned unless you specify the filter
/// parameter with a value of `Unavailable` in your `GetLambdaFunctionRecommendations`
/// request" (`LambdaFunctionRecommendation$finding`), so the reader withholds them from
/// every other request rather than reporting a recommendation that does not exist.
pub fn lambda_filters_ask_for_unavailable(filters: &[LambdaFunctionRecommendationFilter]) -> bool {
    lambda_filters_name_unavailable(filters.iter().map(|filter| {
        (
            filter.name.as_deref().unwrap_or_default(),
            filter.values.as_deref().unwrap_or_default(),
        )
    }))
}

/// [`lambda_filters_ask_for_unavailable`] over the (name, values) pairs a filter list reduces
/// to, so the EXPORT of these recommendations — which carries the filters of any surface as
/// such pairs ([`super::export::ExportPlan::filters`]) — withholds the functions the analysis
/// reached no finding for by the very rule the reader withholds them by.
pub(super) fn lambda_filters_name_unavailable<'a>(
    filters: impl IntoIterator<Item = (&'a str, &'a [String])>,
) -> bool {
    filters.into_iter().any(|(name, values)| {
        name == LAMBDA_FILTER_FINDING
            && values
                .iter()
                .any(|value| value == LAMBDA_FINDING_UNAVAILABLE)
    })
}
