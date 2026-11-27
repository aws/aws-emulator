// SPDX-License-Identifier: CC0-1.0

//! The forecast a predictive scaling policy publishes: what `GetPredictiveScalingForecast`
//! answers, and what it refuses.
//!
//! The rules under test:
//!
//! 1. The operation reports the forecast OF a predictive scaling policy, so a `PolicyName`
//!    that resolves to a policy of any OTHER `PolicyType` is refused with HTTP 400
//!    `ValidationError` and the sentence "Policy <policy-name> is not a predictive scaling
//!    policy." — the table spans every other type the model admits. A name no policy of the
//!    group answers to is refused as an absent policy, and a group the account does not hold
//!    for the group.
//! 2. A forecast is COMPUTED from the observations the account's own CloudWatch store holds
//!    for the metric the policy's specification names: the hourly load of each hour of the
//!    requested range is the mean load observed at that hour of the day, and the capacity
//!    forecast is the smallest capacity that keeps the load at the specification's
//!    `TargetValue`, held inside the group's own bounds. The table spans the two forms a
//!    specification names its load metric in — a predefined metric pair and a customized load
//!    metric — and the values are derived from the observations the case published rather than
//!    written out.
//! 3. "A minimum of 24 hours of data is required to create the initial forecasts"
//!    (`GetPredictiveScalingForecast`): what the data decides is each hour's VALUE, so a
//!    policy whose metric the store holds fewer hours of is still reported over every hour of
//!    the range asked for, carrying NO VALUE for them rather than an invented load.
//! 4. The forecast is held to the group's `MaxSize` while the policy honours it
//!    (`PredictiveScalingConfiguration$MaxCapacityBreachBehavior`: "HonorMaxCapacity"), and
//!    is not when the policy may raise it ("IncreaseMaxCapacity").
//! 5. A specification naming a form no load can be read for — a metric math expression (alone
//!    or over the named sub-metrics it is computed from), queries that settle on no one
//!    result, or an extended (percentile) statistic, which needs the individual published
//!    values a stored aggregate does not carry — is answered as the unimplemented form it is
//!    rather than with a load aggregated some other way.
//! 6. "This is an array, but we currently only support a single metric specification"
//!    (`PredictiveScalingConfiguration$MetricSpecifications`): a put stating more is refused
//!    for the member and stores no policy — which is also what bounds the forecast a read
//!    computes, one series per specification.
//! 7. The span a request may ask a forecast over is the one its own members state: "The
//!    maximum time duration between the start and end time is 30 days"
//!    (`GetPredictiveScalingForecastType$EndTime`) and "At most, the date and time can be one
//!    year before the current date and time." (`$StartTime`), and a window that does not open
//!    bounds no forecast — each bound tested at the published value and one period past it.
//!
//! Every assertion names its case and the value actually reported.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch_service;
use service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

use crate::support::{
    borrowed, json_rpc, list_members, new_engine, p, query, seed_launch_templates, tag_value,
    tag_values, xml_unescape,
};

/// The launch template every group here is created from.
const LAUNCH_TEMPLATE: &str = "lt-forecast";

/// Where every group here places the instances it would launch.
const ZONE: (&str, &str) = ("AvailabilityZones.member.1", "us-west-2a");

/// The signing name a client scopes a CloudWatch request to.
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The `x-amz-target` prefix CloudWatch's awsJson protocol names its operations under.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// The awsJson version a CloudWatch client sends.
const CLOUDWATCH_JSON_VERSION: &str = "1.0";

/// The namespace Amazon EC2 publishes an instance metric under, and the dimension a metric of
/// a whole Auto Scaling group carries: the series a predefined metric pair names.
const EC2_NAMESPACE: &str = "AWS/EC2";
const GROUP_DIMENSION: &str = "AutoScalingGroupName";

/// The namespace and metric a CUSTOMIZED load metric names here — a client's own, which is
/// why it can be published over `PutMetricData`.
const CUSTOM_NAMESPACE: &str = "Emulator/Forecast";
const CUSTOM_METRIC: &str = "RequestsPerHour";

/// How wide one forecast period is: the hour the operation forecasts in.
const PERIOD: Duration = Duration::from_secs(60 * 60);

/// How many periods one day holds.
const PERIODS_PER_DAY: i64 = 24;

/// The fewest hours of data a forecast can be made from: "A minimum of 24 hours of data is
/// required to create the initial forecasts" (`GetPredictiveScalingForecast`). Written out
/// here rather than taken from the crate.
const MIN_HISTORY_PERIODS: i64 = 24;

/// The `TargetValue` every policy here states, which is what divides a forecast load into the
/// capacity it needs.
const TARGET_VALUE: f64 = 50.0;

/// The bounds every group here is created with: a ceiling the forecast of rule 2 stays under,
/// so only rule 4's own case is clamped by it.
const GROUP_MIN_SIZE: i32 = 0;
const GROUP_MAX_SIZE: i32 = 20;

/// One emulator with Amazon EC2 Auto Scaling and CloudWatch on ONE engine: the observations a
/// forecast is computed from are the account's own CloudWatch metrics, so one engine is what
/// makes the two views one account.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = new_engine();
    seed_launch_templates(&engine, &[LAUNCH_TEMPLATE]);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(cloudwatch_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Create one empty group with the bounds above, so nothing settles on the clock while the
/// forecast is read.
async fn create_group(router: &MultiServiceRouter, name: &str, max_size: i32) {
    let (status, body) = query(
        router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", name),
            ("LaunchTemplate.LaunchTemplateName", LAUNCH_TEMPLATE),
            ("MinSize", &GROUP_MIN_SIZE.to_string()),
            ("MaxSize", &max_size.to_string()),
            ("DesiredCapacity", "0"),
            ZONE,
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "creating {name:?} must succeed, got {status} with {body:?}"
    );
}

/// Put one scaling policy of `policy_type` on `group`, carrying `extra` beyond its type, and
/// answer its status, wire code and message.
async fn put_policy_answer(
    router: &MultiServiceRouter,
    group: &str,
    policy: &str,
    policy_type: &str,
    extra: &[(String, String)],
) -> (u16, String, String) {
    let mut params = vec![
        p("AutoScalingGroupName", group),
        p("PolicyName", policy),
        p("PolicyType", policy_type),
    ];
    params.extend_from_slice(extra);
    let (status, body) = query(router, "PutScalingPolicy", &borrowed(&params)).await;
    (
        status,
        tag_value(&body, "Code"),
        xml_unescape(&tag_value(&body, "Message")),
    )
}

/// Put one scaling policy of `policy_type` on `group`, carrying `extra` beyond its type.
async fn put_policy(
    router: &MultiServiceRouter,
    group: &str,
    policy: &str,
    policy_type: &str,
    extra: &[(String, String)],
) {
    let (status, code, message) =
        put_policy_answer(router, group, policy, policy_type, extra).await;
    assert_eq!(
        status, 200,
        "putting the {policy_type} policy {policy:?} must succeed, got {status} with {code:?} \
         {message:?}"
    );
}

/// The members a predictive-scaling put states beyond its type: the target value, the metric
/// form `metric` names, and the breach behaviour when the case states one.
fn predictive_members(metric: &[(String, String)], breach: Option<&str>) -> Vec<(String, String)> {
    let mut params = vec![p(
        "PredictiveScalingConfiguration.MetricSpecifications.member.1.TargetValue",
        TARGET_VALUE.to_string(),
    )];
    params.extend_from_slice(metric);
    if let Some(breach) = breach {
        params.push(p(
            "PredictiveScalingConfiguration.MaxCapacityBreachBehavior",
            breach,
        ));
    }
    params
}

/// The predefined-metric-pair form of a load metric: the group's own total CPU.
fn predefined_metric_pair() -> Vec<(String, String)> {
    vec![p(
        "PredictiveScalingConfiguration.MetricSpecifications.member.1\
         .PredefinedMetricPairSpecification.PredefinedMetricType",
        "ASGCPUUtilization",
    )]
}

/// The customized-load-metric form: a metric data query naming a client's own series,
/// aggregated by `stat` over the hour.
fn customized_load_metric_at(group: &str, stat: &str) -> Vec<(String, String)> {
    let prefix = "PredictiveScalingConfiguration.MetricSpecifications.member.1\
                  .CustomizedLoadMetricSpecification.MetricDataQueries.member.1";
    vec![
        p(&format!("{prefix}.Id"), "load"),
        p(&format!("{prefix}.MetricStat.Metric.Namespace"), CUSTOM_NAMESPACE),
        p(&format!("{prefix}.MetricStat.Metric.MetricName"), CUSTOM_METRIC),
        p(
            &format!("{prefix}.MetricStat.Metric.Dimensions.member.1.Name"),
            GROUP_DIMENSION,
        ),
        p(
            &format!("{prefix}.MetricStat.Metric.Dimensions.member.1.Value"),
            group,
        ),
        p(&format!("{prefix}.MetricStat.Stat"), stat),
    ]
}

/// The same form at the statistic a load is totalled by.
fn customized_load_metric(group: &str) -> Vec<(String, String)> {
    customized_load_metric_at(group, "Sum")
}

/// A customized load metric that is a metric MATH EXPRESSION, which names no single series.
fn customized_load_expression() -> Vec<(String, String)> {
    let prefix = "PredictiveScalingConfiguration.MetricSpecifications.member.1\
                  .CustomizedLoadMetricSpecification.MetricDataQueries.member.1";
    vec![
        p(&format!("{prefix}.Id"), "load"),
        p(&format!("{prefix}.Expression"), "SUM(METRICS())"),
    ]
}

/// The standard metric-math shape: the expression query that IS the result, beside the named
/// sub-metrics it is computed from — none of which is the load the policy is driven by.
fn customized_load_expression_over_sub_metrics(group: &str) -> Vec<(String, String)> {
    let queries = "PredictiveScalingConfiguration.MetricSpecifications.member.1\
                   .CustomizedLoadMetricSpecification.MetricDataQueries";
    let mut params = vec![
        p(&format!("{queries}.member.1.Id"), "e1"),
        p(&format!("{queries}.member.1.Expression"), "m1+m2"),
        p(&format!("{queries}.member.1.ReturnData"), "true"),
    ];
    for (index, id) in [(2, "m1"), (3, "m2")] {
        let query = format!("{queries}.member.{index}");
        params.extend([
            p(&format!("{query}.Id"), id),
            p(&format!("{query}.MetricStat.Metric.Namespace"), CUSTOM_NAMESPACE),
            p(&format!("{query}.MetricStat.Metric.MetricName"), CUSTOM_METRIC),
            p(
                &format!("{query}.MetricStat.Metric.Dimensions.member.1.Name"),
                GROUP_DIMENSION,
            ),
            p(
                &format!("{query}.MetricStat.Metric.Dimensions.member.1.Value"),
                group,
            ),
            p(&format!("{query}.MetricStat.Stat"), "Sum"),
            p(&format!("{query}.ReturnData"), "false"),
        ]);
    }
    params
}

/// Two named queries that both state they are the result, so they settle on no ONE series.
fn customized_load_two_results(group: &str) -> Vec<(String, String)> {
    let queries = "PredictiveScalingConfiguration.MetricSpecifications.member.1\
                   .CustomizedLoadMetricSpecification.MetricDataQueries";
    let mut params = Vec::new();
    for (index, id) in [(1, "m1"), (2, "m2")] {
        let query = format!("{queries}.member.{index}");
        params.extend([
            p(&format!("{query}.Id"), id),
            p(&format!("{query}.MetricStat.Metric.Namespace"), CUSTOM_NAMESPACE),
            p(&format!("{query}.MetricStat.Metric.MetricName"), CUSTOM_METRIC),
            p(
                &format!("{query}.MetricStat.Metric.Dimensions.member.1.Name"),
                GROUP_DIMENSION,
            ),
            p(
                &format!("{query}.MetricStat.Metric.Dimensions.member.1.Value"),
                group,
            ),
            p(&format!("{query}.MetricStat.Stat"), "Sum"),
            p(&format!("{query}.ReturnData"), "true"),
        ]);
    }
    params
}

/// Post one `GetPredictiveScalingForecast` and answer its status, wire code, message and
/// body.
async fn get_forecast(
    router: &MultiServiceRouter,
    group: &str,
    policy: &str,
    start: SystemTime,
    end: SystemTime,
) -> (u16, String, String, String) {
    let (status, body) = query(
        router,
        "GetPredictiveScalingForecast",
        &[
            ("AutoScalingGroupName", group),
            ("PolicyName", policy),
            ("StartTime", &rfc3339(start)),
            ("EndTime", &rfc3339(end)),
        ],
    )
    .await;
    (
        status,
        tag_value(&body, "Code"),
        xml_unescape(&tag_value(&body, "Message")),
        body,
    )
}

/// `at` as the RFC 3339 instant a request states a timestamp in.
fn rfc3339(at: SystemTime) -> String {
    let secs = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    ::aws_smithy_types::DateTime::from_secs(i64::try_from(secs).unwrap_or_default())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("the instant renders")
}

/// One series the answer reports: its time stamps, as epoch seconds, beside its values.
#[derive(Debug, Clone, PartialEq)]
struct Series {
    timestamps: Vec<i64>,
    values: Vec<f64>,
}

/// The `<Timestamps>` and `<Values>` of one block of the answer.
fn series_of(block: &str) -> Series {
    Series {
        timestamps: list_members(block, "Timestamps")
            .into_iter()
            .map(|member| {
                ::aws_smithy_types::DateTime::from_str(
                    member,
                    ::aws_smithy_types::date_time::Format::DateTime,
                )
                .map(|at| at.secs())
                .unwrap_or_default()
            })
            .collect(),
        values: list_members(block, "Values")
            .into_iter()
            .map(|member| member.parse::<f64>().unwrap_or_default())
            .collect(),
    }
}

/// The load forecast and the capacity forecast one answer reports.
fn forecasts(body: &str) -> (Series, Series) {
    let load = tag_value(body, "LoadForecast");
    (series_of(&load), series_of(&tag_value(body, "CapacityForecast")))
}

/// `at` rounded DOWN to the whole hour it falls in.
fn whole_hour(at: SystemTime) -> SystemTime {
    let secs = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    UNIX_EPOCH + Duration::from_secs(secs - secs % PERIOD.as_secs())
}

/// Which hour of the day `at` falls in, counted off the epoch exactly as a forecast period
/// is.
fn period_of_day(at: SystemTime) -> i64 {
    let hours = i64::try_from(
        at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() / PERIOD.as_secs(),
    )
    .unwrap_or_default();
    hours.rem_euclid(PERIODS_PER_DAY)
}

/// `at` as the epoch seconds a stored observation and a request state an instant in.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The load the history of these suites states for the hour of the day `slot`: a pattern that
/// differs by hour, so a forecast that reported one hour's load for another would be caught.
fn load_of_slot(slot: i64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let slot = slot as f64;
    100.0 + 10.0 * slot
}

/// The whole hours `periods` hours back from the hour `now` falls in, oldest first.
fn history_hours(now: SystemTime, periods: i64) -> Vec<SystemTime> {
    let latest = whole_hour(now);
    (1..=periods)
        .rev()
        .map(|back| {
            latest
                - Duration::from_secs(
                    PERIOD.as_secs() * u64::try_from(back).unwrap_or_default(),
                )
        })
        .collect()
}

/// Seed the series Amazon EC2 publishes for a whole group with one observation per hour of
/// `hours`, each carrying the load its own hour of the day states.
///
/// Written straight into the account's CloudWatch store because this is a reserved `AWS/`
/// namespace no client may publish into — the same seam the service's own group metrics are
/// appended through.
fn seed_group_cpu_history(
    engine: &Arc<engine::InMemoryEngine>,
    group: &str,
    hours: &[SystemTime],
) {
    let state =
        cloudwatch::service_state(&**engine).expect("the CloudWatch store is mounted on this engine");
    let mut store = state.lock();
    let key = cloudwatch::MetricKey {
        namespace: EC2_NAMESPACE.to_string(),
        metric_name: "CPUUtilization".to_string(),
        dimensions: BTreeMap::from([(GROUP_DIMENSION.to_string(), group.to_string())]),
    };
    let data: Vec<cloudwatch::StoredDatum> = hours
        .iter()
        .map(|at| {
            let value = load_of_slot(period_of_day(*at));
            cloudwatch::StoredDatum {
                timestamp_secs: epoch_secs(*at),
                sample_count: 1.0,
                sum: value,
                minimum: value,
                maximum: value,
                unit: None,
                storage_resolution: 60,
            }
        })
        .collect();
    store
        .append_datapoints(key, data)
        .expect("the metric store holds the series");
}

/// Publish one observation per hour of `hours` into the client's OWN series, over
/// `PutMetricData`.
async fn publish_custom_history(router: &MultiServiceRouter, group: &str, hours: &[SystemTime]) {
    let data: Vec<Value> = hours
        .iter()
        .map(|at| {
            json!({
                "MetricName": CUSTOM_METRIC,
                "Dimensions": [{"Name": GROUP_DIMENSION, "Value": group}],
                "Timestamp": epoch_secs(*at),
                "Value": load_of_slot(period_of_day(*at)),
            })
        })
        .collect();
    let (status, answer) = json_rpc(
        router,
        CLOUDWATCH_SIGNING_NAME,
        CLOUDWATCH_TARGET_PREFIX,
        CLOUDWATCH_JSON_VERSION,
        "PutMetricData",
        json!({"Namespace": CUSTOM_NAMESPACE, "MetricData": data}),
    )
    .await;
    assert_eq!(
        status, 200,
        "publishing the history must succeed, got {status} with {answer}"
    );
}

/// One scaling policy a row of rule 1 puts: its name, its `PolicyType`, and the members that
/// type requires.
type PolicyRow<'a> = (&'a str, &'a str, Vec<(String, String)>);

/// RULE 1: a policy name that resolves to a policy of any other type is refused, and so are a
/// name no policy answers to and a group the account does not hold.
#[tokio::test]
async fn a_policy_of_another_type_has_no_forecast() {
    let group = "forecast-types";
    let (router, _engine) = emulator();
    create_group(&router, group, GROUP_MAX_SIZE).await;

    // Every other `PolicyType` the model admits
    // (`smithy_query(query="doc",shape="PutScalingPolicyType",member="PolicyType")`:
    // "TargetTrackingScaling StepScaling SimpleScaling (default) PredictiveScaling"), each
    // with the members its own type requires.
    let rows: Vec<PolicyRow<'_>> = vec![
        (
            "simple-policy",
            "SimpleScaling",
            vec![
                p("AdjustmentType", "ChangeInCapacity"),
                p("ScalingAdjustment", "1"),
            ],
        ),
        (
            "step-policy",
            "StepScaling",
            vec![
                p("AdjustmentType", "ChangeInCapacity"),
                p(
                    "StepAdjustments.member.1.MetricIntervalLowerBound",
                    "0",
                ),
                p("StepAdjustments.member.1.ScalingAdjustment", "1"),
            ],
        ),
        (
            "target-policy",
            "TargetTrackingScaling",
            vec![
                p(
                    "TargetTrackingConfiguration.PredefinedMetricSpecification\
                     .PredefinedMetricType",
                    "ASGAverageCPUUtilization",
                ),
                p("TargetTrackingConfiguration.TargetValue", "40"),
            ],
        ),
    ];

    let start = whole_hour(SystemTime::now()) + PERIOD;
    let end = start + Duration::from_secs(24 * PERIOD.as_secs());
    for (policy, policy_type, members) in &rows {
        put_policy(&router, group, policy, policy_type, members).await;
        let (status, code, message, body) = get_forecast(&router, group, policy, start, end).await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (
                400,
                "ValidationError",
                format!("Policy {policy} is not a predictive scaling policy.").as_str()
            ),
            "case {policy_type:?}: expected the wrong-type refusal, got {status} {code:?} \
             {message:?} with {body}"
        );
    }

    // A name no policy of the group answers to, and a group the account does not hold.
    let (status, code, message, body) = get_forecast(&router, group, "nonesuch", start, end).await;
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (400, "ValidationError", "Policy nonesuch not found."),
        "case \"absent policy\": got {status} {code:?} {message:?} with {body}"
    );
    let (status, code, message, body) =
        get_forecast(&router, "forecast-absent", "step-policy", start, end).await;
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (400, "ValidationError", "Group forecast-absent not found."),
        "case \"absent group\": got {status} {code:?} {message:?} with {body}"
    );
}

/// RULE 2: the forecast is computed from the observations the store holds for the metric the
/// specification names, in both forms a specification names one in.
#[tokio::test]
async fn the_forecast_is_computed_from_the_observed_history() {
    // Two days of hourly observations, so every hour of the day is observed twice and the
    // mean of a slot is the load that slot states.
    let observed = 2 * PERIODS_PER_DAY;
    for (case, predefined) in [("a predefined metric pair", true), ("a customized load metric", false)] {
        let group = if predefined {
            "forecast-predefined"
        } else {
            "forecast-customized"
        };
        let (router, engine) = emulator();
        create_group(&router, group, GROUP_MAX_SIZE).await;
        let now = SystemTime::now();
        let hours = history_hours(now, observed);
        let metric = if predefined {
            seed_group_cpu_history(&engine, group, &hours);
            predefined_metric_pair()
        } else {
            publish_custom_history(&router, group, &hours).await;
            customized_load_metric(group)
        };
        put_policy(
            &router,
            group,
            "forecast-policy",
            "PredictiveScaling",
            &predictive_members(&metric, None),
        )
        .await;

        // The policy reads back carrying the configuration it was put with, so the forecast
        // below is taken against a stored policy.
        let (status, read) = query(
            &router,
            "DescribePolicies",
            &[
                ("AutoScalingGroupName", group),
                ("PolicyNames.member.1", "forecast-policy"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the policy must read back, got {status} with {read:?}"
        );
        assert_eq!(
            tag_value(&read, "PolicyType"),
            "PredictiveScaling",
            "case {case:?}: expected the stored policy to be predictive, got {read}"
        );

        let start = whole_hour(now) + PERIOD;
        let periods = 6_u64;
        let end = start + Duration::from_secs(periods * PERIOD.as_secs());
        let (status, code, message, body) =
            get_forecast(&router, group, "forecast-policy", start, end).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the forecast must be served, got {status} {code:?} {message:?}"
        );

        let expected_hours: Vec<SystemTime> = (0..periods)
            .map(|index| start + Duration::from_secs(index * PERIOD.as_secs()))
            .collect();
        let expected_load = Series {
            timestamps: expected_hours
                .iter()
                .map(|at| i64::try_from(at.duration_since(UNIX_EPOCH).unwrap().as_secs()).unwrap())
                .collect(),
            values: expected_hours
                .iter()
                .map(|at| load_of_slot(period_of_day(*at)))
                .collect(),
        };
        let expected_capacity = Series {
            timestamps: expected_load.timestamps.clone(),
            values: expected_load
                .values
                .iter()
                .map(|load| (load / TARGET_VALUE).ceil())
                .collect(),
        };
        let (load, capacity) = forecasts(&body);
        assert_eq!(
            load, expected_load,
            "case {case:?}: expected the load observed at each hour of the day, got {load:?} \
             in {body}"
        );
        assert_eq!(
            capacity, expected_capacity,
            "case {case:?}: expected the capacity each forecast load needs, got {capacity:?} \
             in {body}"
        );
        // The forecast reports the metric specification it was computed from.
        let reported = tag_value(&body, "MetricSpecification");
        assert_eq!(
            tag_value(&reported, "TargetValue"),
            format!("{TARGET_VALUE:.1}"),
            "case {case:?}: expected the forecast to report its own specification, got {body}"
        );
    }
}

/// RULE 3: fewer than the documented minimum hours of data is no PATTERN to forecast from, so
/// the hours asked for are reported with no value at all rather than dropped.
#[tokio::test]
async fn too_little_history_predicts_no_value() {
    let group = "forecast-short";
    let (router, engine) = emulator();
    create_group(&router, group, GROUP_MAX_SIZE).await;
    let now = SystemTime::now();
    seed_group_cpu_history(&engine, group, &history_hours(now, MIN_HISTORY_PERIODS - 1));
    put_policy(
        &router,
        group,
        "forecast-policy",
        "PredictiveScaling",
        &predictive_members(&predefined_metric_pair(), None),
    )
    .await;

    let periods = 6_u64;
    let start = whole_hour(now) + PERIOD;
    let end = start + Duration::from_secs(periods * PERIOD.as_secs());
    let (status, code, message, body) =
        get_forecast(&router, group, "forecast-policy", start, end).await;
    assert_eq!(
        status, 200,
        "the forecast must be served, got {status} {code:?} {message:?}"
    );
    let (load, capacity) = forecasts(&body);
    let expected_hours: Vec<i64> = (0..periods)
        .map(|index| {
            let at = start + Duration::from_secs(index * PERIOD.as_secs());
            i64::try_from(at.duration_since(UNIX_EPOCH).unwrap().as_secs()).unwrap()
        })
        .collect();
    assert_eq!(
        (load.timestamps.clone(), capacity.timestamps.clone()),
        (expected_hours.clone(), expected_hours),
        "expected both series to cover every hour of the range, got {load:?} and \
         {capacity:?} in {body}"
    );
    assert_eq!(
        (load.values.len(), capacity.values.len()),
        (periods as usize, periods as usize),
        "expected a value per hour in both series, got {load:?} and {capacity:?} in {body}"
    );
    for (slot, (predicted, needed)) in load.values.iter().zip(&capacity.values).enumerate() {
        assert!(
            predicted.is_nan() && needed.is_nan(),
            "expected no value from fewer than {MIN_HISTORY_PERIODS} hours of data: hour \
             {slot} reports the load {predicted} needing {needed} in {body}"
        );
    }
}

/// RULE 4: the forecast capacity is held to the group's `MaxSize` while the policy honours
/// it, and is not when the policy may raise it.
#[tokio::test]
async fn the_forecast_capacity_honours_the_groups_ceiling() {
    // A ceiling BELOW the capacity the observed load needs, so the two behaviours differ.
    let ceiling = 3;
    let rows = [
        ("HonorMaxCapacity", true),
        ("IncreaseMaxCapacity", false),
    ];
    for (behavior, clamped) in rows {
        let group = format!("forecast-{}", behavior.to_lowercase());
        let (router, engine) = emulator();
        create_group(&router, &group, ceiling).await;
        let now = SystemTime::now();
        let hours = history_hours(now, 2 * PERIODS_PER_DAY);
        seed_group_cpu_history(&engine, &group, &hours);
        let mut members = predictive_members(&predefined_metric_pair(), Some(behavior));
        if !clamped {
            // "The upper limit is determined by the forecasted capacity and the value for
            // MaxCapacityBuffer" — a policy that may raise the ceiling states the buffer.
            members.push(p("PredictiveScalingConfiguration.MaxCapacityBuffer", "10"));
        }
        put_policy(
            &router,
            &group,
            "forecast-policy",
            "PredictiveScaling",
            &members,
        )
        .await;

        let start = whole_hour(now) + PERIOD;
        let end = start + Duration::from_secs(24 * PERIOD.as_secs());
        let (status, code, message, body) =
            get_forecast(&router, &group, "forecast-policy", start, end).await;
        assert_eq!(
            status, 200,
            "case {behavior:?}: the forecast must be served, got {status} {code:?} {message:?}"
        );
        let (load, capacity) = forecasts(&body);
        let needed: Vec<f64> = load
            .values
            .iter()
            .map(|value| (value / TARGET_VALUE).ceil())
            .collect();
        let expected: Vec<f64> = needed
            .iter()
            .map(|value| {
                if clamped {
                    value.min(f64::from(ceiling))
                } else {
                    *value
                }
            })
            .collect();
        assert_eq!(
            capacity.values, expected,
            "case {behavior:?}: expected the ceiling to be {}, got {capacity:?} for loads \
             {load:?} in {body}",
            if clamped { "honoured" } else { "raised" }
        );
        assert!(
            needed.iter().any(|value| *value > f64::from(ceiling)),
            "case {behavior:?}: the case only tests the ceiling if some hour needs more than \
             {ceiling}, got {needed:?}"
        );
    }
}

/// RULE 5: a specification naming a form no load can be read for is answered as the
/// unimplemented form it is, never with a load aggregated some other way.
#[tokio::test]
async fn a_load_metric_no_series_can_be_read_for_is_not_forecast() {
    // Each row: the case, and the metric form the specification states. An extended
    // statistic (`MetricStat$Stat`) needs the individual published values a stored aggregate
    // does not carry, and a metric math expression names no single series at all.
    let rows: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "an extended statistic",
            customized_load_metric_at("forecast-unreadable", "p90"),
        ),
        ("a metric math expression", customized_load_expression()),
        (
            "an expression over named sub-metrics",
            customized_load_expression_over_sub_metrics("forecast-unreadable"),
        ),
        (
            "two queries that both state they are the result",
            customized_load_two_results("forecast-unreadable"),
        ),
    ];

    let group = "forecast-unreadable";
    let (router, engine) = emulator();
    create_group(&router, group, GROUP_MAX_SIZE).await;
    let now = SystemTime::now();
    // A full history of the client's own series, so the only thing standing between the
    // policy and a forecast is the form its specification states.
    publish_custom_history(&router, group, &history_hours(now, 2 * PERIODS_PER_DAY)).await;
    seed_group_cpu_history(&engine, group, &history_hours(now, 2 * PERIODS_PER_DAY));

    let start = whole_hour(now) + PERIOD;
    let end = start + Duration::from_secs(6 * PERIOD.as_secs());
    for (case, metric) in rows {
        let policy = format!("forecast-{}", case.replace(' ', "-"));
        put_policy(
            &router,
            group,
            &policy,
            "PredictiveScaling",
            &predictive_members(&metric, None),
        )
        .await;
        let (status, code, message, body) = get_forecast(&router, group, &policy, start, end).await;
        assert_eq!(
            (status, code.as_str()),
            (501, "NotImplemented"),
            "case {case:?}: expected the unimplemented form rather than a forecast, got \
             {status} {code:?} {message:?} with {body}"
        );
        assert!(
            tag_value(&body, "LoadForecast").is_empty()
                && tag_value(&body, "CapacityForecast").is_empty(),
            "case {case:?}: expected no forecast in the answer, got {body}"
        );
    }

    // The control: the SAME group and history, forecast through a form a series can be read
    // for, is served — so what rule 5 refuses is the form and not the policy or the data.
    put_policy(
        &router,
        group,
        "forecast-readable",
        "PredictiveScaling",
        &predictive_members(&customized_load_metric(group), None),
    )
    .await;
    let (status, code, message, body) =
        get_forecast(&router, group, "forecast-readable", start, end).await;
    assert_eq!(
        status, 200,
        "the readable form must be served, got {status} {code:?} {message:?}"
    );
    let (load, capacity) = forecasts(&body);
    assert_eq!(
        (load.values.len(), capacity.values.len()),
        (6, 6),
        "expected a forecast for every hour of the range, got {load:?} and {capacity:?} in \
         {body}"
    );
}

/// RULE 6: a predictive policy may be stored with one metric specification, so a put stating
/// more is refused for the member and stores nothing — the bound that makes the forecast a
/// read computes bounded too.
#[tokio::test]
async fn a_policy_may_state_one_metric_specification() {
    let group = "forecast-specifications";
    let (router, _engine) = emulator();
    create_group(&router, group, GROUP_MAX_SIZE).await;

    // The documented maximum ("we currently only support a single metric specification"),
    // written out here rather than taken from the crate.
    let supported = 1;
    let mut over = predictive_members(&predefined_metric_pair(), None);
    over.push(p(
        "PredictiveScalingConfiguration.MetricSpecifications.member.2.TargetValue",
        TARGET_VALUE.to_string(),
    ));
    over.push(p(
        "PredictiveScalingConfiguration.MetricSpecifications.member.2\
         .PredefinedMetricPairSpecification.PredefinedMetricType",
        "ASGNetworkIn",
    ));
    let (status, code, message) =
        put_policy_answer(&router, group, "two-specifications", "PredictiveScaling", &over).await;
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (
            400,
            "ValidationError",
            format!(
                "1 validation error detected: Value with 2 items at \
                 'predictiveScalingConfiguration.metricSpecifications' failed to satisfy \
                 constraint: Member must have length less than or equal to {supported}"
            )
            .as_str()
        ),
        "a put stating two specifications must be refused for the member, got {status} \
         {code:?} {message:?}"
    );

    // The refused put stored nothing, and the supported count is served.
    let (status, read) = query(
        &router,
        "DescribePolicies",
        &[("AutoScalingGroupName", group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the read must succeed, got {status} with {read:?}"
    );
    assert!(
        tag_value(&read, "ScalingPolicies").is_empty(),
        "expected the refused put to store no policy, got {read}"
    );
    put_policy(
        &router,
        group,
        "one-specification",
        "PredictiveScaling",
        &predictive_members(&predefined_metric_pair(), None),
    )
    .await;
    let (status, read) = query(
        &router,
        "DescribePolicies",
        &[("AutoScalingGroupName", group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the read must succeed, got {status} with {read:?}"
    );
    assert_eq!(
        tag_values(&read, "PolicyName"),
        vec!["one-specification".to_string()],
        "expected the group to hold the policy the supported count stored, got {read}"
    );
    assert_eq!(
        tag_values(&tag_value(&read, "MetricSpecifications"), "TargetValue").len(),
        supported,
        "expected the stored policy to carry the one specification the put stated, got {read}"
    );
}

/// RULE 7: the span a request may ask a forecast over is the one its own members state —
/// tested at each published bound and one forecast period past it.
#[tokio::test]
async fn the_span_a_request_may_ask_a_forecast_over() {
    // The bounds the members document, written out here rather than taken from the crate.
    let max_range = Duration::from_secs(30 * 24 * PERIOD.as_secs());
    let max_lookback = Duration::from_secs(365 * 24 * PERIOD.as_secs());
    let too_long = format!(
        "The maximum time duration between the start and end time is {} days.",
        max_range.as_secs() / (24 * PERIOD.as_secs())
    );
    let too_old = "At most, the date and time can be one year before the current date and \
                   time.";
    let window_shut = "Given end time must be after start time";

    let group = "forecast-range";
    let (router, _engine) = emulator();
    create_group(&router, group, GROUP_MAX_SIZE).await;
    put_policy(
        &router,
        group,
        "forecast-policy",
        "PredictiveScaling",
        &predictive_members(&predefined_metric_pair(), None),
    )
    .await;

    let next_hour = whole_hour(SystemTime::now()) + PERIOD;
    // Each row: the case, the range it asks for, and the sentence it must be refused with —
    // empty for a range the forecast is served over.
    let rows: Vec<(&str, SystemTime, SystemTime, &str)> = vec![
        ("the longest span the member allows", next_hour, next_hour + max_range, ""),
        (
            "one period past that span",
            next_hour,
            next_hour + max_range + PERIOD,
            too_long.as_str(),
        ),
        (
            "the furthest back the member allows",
            whole_hour(SystemTime::now()) - max_lookback + PERIOD,
            whole_hour(SystemTime::now()) - max_lookback + 2 * PERIOD,
            "",
        ),
        (
            "one period further back than that",
            whole_hour(SystemTime::now()) - max_lookback - PERIOD,
            whole_hour(SystemTime::now()) - max_lookback,
            too_old,
        ),
        ("a window of no width at all", next_hour, next_hour, window_shut),
        (
            "a window that closes before it opens",
            next_hour,
            next_hour - PERIOD,
            window_shut,
        ),
        ("one period of forecast", next_hour, next_hour + PERIOD, ""),
    ];

    for (case, start, end, expected) in rows {
        let (status, code, message, body) =
            get_forecast(&router, group, "forecast-policy", start, end).await;
        if expected.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: the range must be served, got {status} {code:?} {message:?}"
            );
            continue;
        }
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "ValidationError", expected),
            "case {case:?}: expected the range refusal, got {status} {code:?} {message:?} \
             with {body}"
        );
    }
}

