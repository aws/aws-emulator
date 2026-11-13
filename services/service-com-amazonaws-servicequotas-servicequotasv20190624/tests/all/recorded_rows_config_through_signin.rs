// SPDX-License-Identifier: CC0-1.0

//! What a default-quota row published under an AWS-minted `QuotaCode` has to CARRY,
//! over the wire.
//!
//! A `QuotaCode` is the identity a client resolves a limit by, so a row served under one
//! is a claim about the quota AWS publishes under THAT code. The rules asserted here,
//! each over a table of rows rather than over one quota:
//!
//! * a published row carries the quota's own published facts — its `QuotaName`,
//!   `Value`, `Unit`, `Adjustable`, `GlobalQuota`, `Description`, the `Period` it is
//!   measured over and the `UsageMetric` its usage is published in — identically through
//!   the listing and the single read, and never a neighbouring quota's;
//! * `Adjustable` is not decoration: a quota AWS publishes as not adjustable refuses
//!   `RequestServiceQuotaIncrease` with `IllegalArgumentException`, while an adjustable
//!   one takes the request — so a row that reports the wrong adjustability sends a
//!   client down a path the service does not serve;
//! * a row of the per-API REQUEST-RATE class of a service is derived from the operation
//!   it throttles: one-second window, that operation's `AWS/Usage` call count, and the
//!   class's sentence with that operation named in it. The expectation is computed from
//!   each row's own `QuotaName`, so every row the catalog publishes in that class is a
//!   case and none may borrow another's text;
//! * a `QuotaName` AWS publishes under TWO codes stays two quotas: each code resolves
//!   its own record, because the `(ServiceCode, QuotaCode)` pair is the identity and the
//!   name is not.
//!
//! The rows span what the rules generalise over: a plain count and a count in bytes, a
//! resource count metered over five minutes, a per-API rate metered over one second, a
//! rate measured in the service's OWN metric namespace, a quota published over a
//! 24-hour window, a rate with no usage metric at all, adjustable and non-adjustable
//! rows, and two codes sharing one published name. The harness (`client`, `emulator`,
//! `default_quotas`, `service_inventory`, `walk_pages`) is the one `quota_workflow.rs`
//! owns.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::quota_workflow::{
    client, default_quotas as defaults, emulator, service_inventory as inventory, Client,
};

/// The `UsageMetric` a row publishes its usage in.
struct Metric {
    namespace: &'static str,
    name: &'static str,
    statistic: &'static str,
    /// Every dimension of the series, including the `Service` one `AWS/Usage` carries.
    dimensions: &'static [(&'static str, &'static str)],
}

/// One published default-quota row, with every member AWS publishes for it.
struct Row {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    description: &'static str,
    /// `(PeriodValue, PeriodUnit)`, or `None` for a quota published as a plain count.
    period: Option<(i32, &'static str)>,
    /// The usage metric the owning service publishes for the quota, or `None` where no
    /// published series identifies it.
    metric: Option<Metric>,
}

/// An `AWS/Usage` resource count: the largest count in the window is the one that
/// matters, so `Maximum` is the statistic published with it.
fn resource_count(dimensions: &'static [(&'static str, &'static str)]) -> Metric {
    Metric {
        namespace: "AWS/Usage",
        name: "ResourceCount",
        statistic: "Maximum",
        dimensions,
    }
}

/// An `AWS/Usage` call count: a rate is the calls ADDED UP over the window, so `Sum` is
/// the statistic published with it.
fn call_count(dimensions: &'static [(&'static str, &'static str)]) -> Metric {
    Metric {
        namespace: "AWS/Usage",
        name: "CallCount",
        statistic: "Sum",
        dimensions,
    }
}

/// The rows the rules are driven over.
///
/// SOURCE: AWS's published default-quota data for each `ServiceCode` (what `aws
/// service-quotas list-aws-default-service-quotas --service-code <code>` reports), which
/// states each row's code, name, default value, unit, adjustability, description and —
/// where AWS publishes them — the window the quota is measured over and the metric its
/// usage is published in. `src/state.rs::seed_quota_catalog` and
/// `src/state/quota_rows.rs` are where the table lives and name the source of each
/// member.
fn rows() -> Vec<Row> {
    vec![
        // A plain count AWS publishes as NOT adjustable.
        Row {
            service_code: "config",
            quota_code: "L-0BE82991",
            quota_name: "AWS Config Rules per region",
            value: 1000.0,
            unit: "None",
            adjustable: false,
            description: "The maximum number of AWS Config Rules per region in an AWS account",
            period: None,
            metric: None,
        },
        // A RESOURCE COUNT: metered over the five-minute window `AWS/Usage` publishes a
        // resource count on, under the series the owning service publishes it as.
        Row {
            service_code: "kinesis",
            quota_code: "L-0918CF54",
            quota_name: "Shards per Region",
            value: 20_000.0,
            unit: "None",
            adjustable: true,
            description: "The maximum number of shards that you can create in this account in \
                          the current Region.",
            period: Some((5, "MINUTE")),
            metric: Some(resource_count(&[
                ("Class", "None"),
                ("Resource", "ProvisionedShards"),
                ("Service", "Kinesis Data Streams"),
                ("Type", "Resource"),
            ])),
        },
        // Three rows of the per-API REQUEST-RATE class: one second, and the call count
        // of the API each throttles. The rate is the API's own published one where it
        // states one, else the rate the class is published at.
        Row {
            service_code: "monitoring",
            quota_code: "L-6FCAAA2E",
            quota_name: "Rate of GetMetricWidgetImage requests",
            value: 20.0,
            unit: "None",
            adjustable: true,
            description: "The maximum number of GetMetricWidgetImage requests that you can \
                          make, per second, in this account in the current region.",
            period: Some((1, "SECOND")),
            metric: Some(call_count(&[
                ("Class", "None"),
                ("Resource", "GetMetricWidgetImage"),
                ("Service", "CloudWatch"),
                ("Type", "API"),
            ])),
        },
        Row {
            service_code: "monitoring",
            quota_code: "L-69C44FFD",
            quota_name: "Rate of ListDashboards requests",
            value: 5.0,
            unit: "None",
            adjustable: true,
            description: "The maximum number of ListDashboards requests that you can make, \
                          per second, in this account in the current region.",
            period: Some((1, "SECOND")),
            metric: Some(call_count(&[
                ("Class", "None"),
                ("Resource", "ListDashboards"),
                ("Service", "CloudWatch"),
                ("Type", "API"),
            ])),
        },
        Row {
            service_code: "monitoring",
            quota_code: "L-6300B446",
            quota_name: "Rate of DescribeAnomalyDetectors requests",
            value: 5.0,
            unit: "None",
            adjustable: true,
            description: "The maximum number of DescribeAnomalyDetectors requests that you \
                          can make, per second, in this account in the current region.",
            period: Some((1, "SECOND")),
            metric: Some(call_count(&[
                ("Class", "None"),
                ("Resource", "DescribeAnomalyDetectors"),
                ("Service", "CloudWatch"),
                ("Type", "API"),
            ])),
        },
        // A rate on a metric-math FUNCTION rather than on an API, published as not
        // adjustable: no call-count series counts one function's use inside a call, so
        // the row publishes a window and no usage metric.
        Row {
            service_code: "monitoring",
            quota_code: "L-722EAE58",
            quota_name: "Rate of ANOMALY_DETECTION_BAND usage in GetMetricData",
            value: 1000.0,
            unit: "None",
            adjustable: false,
            description: "The maximum number of times the ANOMALY_DETECTION_BAND function can \
                          be used in all GetMetricData requests, per second, in this account \
                          in the current region.",
            period: Some((1, "SECOND")),
            metric: None,
        },
        // A rate measured in the service's OWN metric namespace, with the empty
        // dimension map that namespace needs to identify the series.
        Row {
            service_code: "ses",
            quota_code: "L-CDEF9B6B",
            quota_name: "Sending rate",
            value: 1.0,
            unit: "None",
            adjustable: true,
            description: "The maximum number of emails that Amazon SES can accept each second \
                          for this account in the current Region.",
            period: Some((1, "SECOND")),
            metric: Some(Metric {
                namespace: "AWS/SES",
                name: "Send",
                statistic: "Sum",
                dimensions: &[],
            }),
        },
        // A quota published over a 24-HOUR window, metered against the series the
        // owning service publishes that usage under.
        Row {
            service_code: "ses",
            quota_code: "L-804C8AE8",
            quota_name: "Sending quota",
            value: 200.0,
            unit: "None",
            adjustable: true,
            description: "The maximum number of emails that you can send in a 24-hour period \
                          for this account in the current Region.",
            period: Some((24, "HOUR")),
            metric: Some(Metric {
                namespace: "AWS/Usage",
                name: "CallCount",
                statistic: "Maximum",
                dimensions: &[
                    ("Class", "None"),
                    ("Resource", "SendLast24Hours"),
                    ("Service", "SES"),
                    ("Type", "API"),
                ],
            }),
        },
        // Two RESOURCE COUNTS of the Region, metered by the service's resource count
        // under the singular resource type each counts.
        Row {
            service_code: "ses",
            quota_code: "L-E7F21B4C",
            quota_name: "Tenant count",
            value: 10_000.0,
            unit: "None",
            adjustable: true,
            description: "The maximum number of tenants that can exist within the current \
                          Region.",
            period: Some((5, "MINUTE")),
            metric: Some(resource_count(&[
                ("Class", "None"),
                ("Resource", "Tenant"),
                ("Service", "SES"),
                ("Type", "Resource"),
            ])),
        },
        Row {
            service_code: "ses",
            quota_code: "L-10E24536",
            quota_name: "Configuration set count",
            value: 10_000.0,
            unit: "None",
            adjustable: true,
            description: "The maximum number of configuration sets that can exist within the \
                          current Region.",
            period: Some((5, "MINUTE")),
            metric: Some(resource_count(&[
                ("Class", "None"),
                ("Resource", "ConfigurationSet"),
                ("Service", "SES"),
                ("Type", "Resource"),
            ])),
        },
        // Two codes under ONE published name: a size in bytes, and the count of the
        // same kind of statement.
        Row {
            service_code: "signin",
            quota_code: "L-42359A23",
            quota_name: "Maximum Signin resource based policy statement size",
            value: 10_240.0,
            unit: "Bytes",
            adjustable: false,
            description: "The maximum size of individual Resource-Based Policy statements in \
                          bytes",
            period: None,
            metric: None,
        },
        Row {
            service_code: "signin",
            quota_code: "L-5ECCC143",
            quota_name: "Maximum Signin resource based policy statement size",
            value: 100.0,
            unit: "None",
            adjustable: false,
            description: "The maximum number of Resource-Based Policy statements that can be \
                          created per AWS account",
            period: None,
            metric: None,
        },
    ]
}

/// The published `Period` of a row, as the wire renders it.
fn period_of(row: &Row) -> Value {
    match row.period {
        Some((value, unit)) => json!({"PeriodValue": value, "PeriodUnit": unit}),
        None => Value::Null,
    }
}

/// The published `UsageMetric` of a row, as the wire renders it.
fn metric_of(row: &Row) -> Value {
    match &row.metric {
        Some(metric) => {
            let dimensions: BTreeMap<&str, &str> = metric.dimensions.iter().copied().collect();
            json!({
                "MetricNamespace": metric.namespace,
                "MetricName": metric.name,
                "MetricStatisticRecommendation": metric.statistic,
                "MetricDimensions": dimensions,
            })
        }
        None => Value::Null,
    }
}

/// `ServiceCode -> ServiceName`, as `ListServices` publishes the inventory: a row's
/// label is the service's, so it is read rather than restated here.
async fn labels(caller: &Client) -> BTreeMap<String, String> {
    inventory(caller)
        .await
        .iter()
        .filter_map(|service| {
            Some((
                service["ServiceCode"].as_str()?.to_string(),
                service["ServiceName"].as_str()?.to_string(),
            ))
        })
        .collect()
}

/// RULE: a row published under an AWS code carries that quota's own published facts,
/// and the listing and the single read report the same record.
#[tokio::test]
async fn a_published_row_carries_the_published_facts_of_its_own_quota() {
    let engine = emulator();
    let region = "us-west-2";
    let caller = client(&engine, region);
    let labels = labels(&caller).await;

    // The table has to span the class rather than repeat one shape of row.
    let table = rows();
    let units: BTreeSet<&str> = table.iter().map(|row| row.unit).collect();
    let windows: BTreeSet<Option<&str>> = table
        .iter()
        .map(|row| row.period.map(|(_, unit)| unit))
        .collect();
    assert!(
        units.len() > 1 && windows.len() > 3 && table.iter().any(|row| !row.adjustable),
        "case table: expected rows spanning units {units:?}, windows {windows:?} and both \
         adjustabilities"
    );

    for row in &table {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let quota = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();
        let expected_label = labels
            .get(row.service_code)
            .unwrap_or_else(|| panic!("case {case}: the inventory must list the service"));

        let served = json!({
            "ServiceCode": quota["ServiceCode"],
            "ServiceName": quota["ServiceName"],
            "QuotaCode": quota["QuotaCode"],
            "QuotaArn": quota["QuotaArn"],
            "QuotaName": quota["QuotaName"],
            "Value": quota["Value"],
            "Unit": quota["Unit"],
            "Adjustable": quota["Adjustable"],
            "GlobalQuota": quota["GlobalQuota"],
            "Description": quota["Description"],
            "Period": quota["Period"],
            "UsageMetric": quota["UsageMetric"],
        });
        let published = json!({
            "ServiceCode": row.service_code,
            "ServiceName": expected_label,
            "QuotaCode": row.quota_code,
            "QuotaArn": format!(
                "arn:aws:servicequotas:{region}::{}/{}",
                row.service_code, row.quota_code
            ),
            "QuotaName": row.quota_name,
            "Value": row.value,
            "Unit": row.unit,
            "Adjustable": row.adjustable,
            "GlobalQuota": false,
            "Description": row.description,
            "Period": period_of(row),
            "UsageMetric": metric_of(row),
        });
        assert_eq!(
            served, published,
            "case {case}: expected the quota's own published members, got {quota}"
        );

        // The listing publishes the identical record, and the applied read answers the
        // same catalog members while nothing is applied over the default.
        let listed = defaults(&caller, row.service_code).await;
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );
        let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
        for member in [
            "QuotaName",
            "Value",
            "Unit",
            "Adjustable",
            "Description",
            "Period",
            "UsageMetric",
        ] {
            assert_eq!(
                applied[member], quota[member],
                "case {case}: the applied read must report the same {member}, got {applied}"
            );
        }
    }
}

/// RULE: `Adjustable` decides whether the service takes an increase request — a quota
/// AWS publishes as fixed refuses one, an adjustable quota accepts it.
#[tokio::test]
async fn adjustability_decides_whether_an_increase_is_accepted() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

    for row in &rows() {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        // Above the value in force, so the only thing left to refuse the request for is
        // the quota's adjustability.
        let desired = row.value * 2.0;
        let body = json!({
            "ServiceCode": row.service_code,
            "QuotaCode": row.quota_code,
            "DesiredValue": desired,
        });

        if row.adjustable {
            let accepted = caller.ok("RequestServiceQuotaIncrease", body).await;
            let requested = &accepted["RequestedQuota"];
            assert_eq!(
                (
                    requested["QuotaCode"].as_str(),
                    requested["DesiredValue"].as_f64(),
                    requested["Status"].as_str(),
                ),
                (Some(row.quota_code), Some(desired), Some("PENDING")),
                "case {case}: an adjustable quota takes the increase it published room for, \
                 got {accepted}"
            );
            continue;
        }

        let (status, error) = caller.refused("RequestServiceQuotaIncrease", body).await;
        assert_eq!(
            (status, error.as_str()),
            (400, "IllegalArgumentException"),
            "case {case}: a quota AWS publishes as not adjustable refuses an increase"
        );
    }
}

/// The API operation a `QuotaName` of the per-API request-rate class throttles, or
/// `None` for a name outside that class.
///
/// The class is `Rate of <Api> requests`, where `<Api>` is ONE operation's own name — a
/// single word in the casing an operation is named in. A name whose middle is prose
/// ("ANOMALY_DETECTION_BAND usage in GetMetricData") describes a rate on something other
/// than one operation's calls and publishes its own sentence, so it is not of this
/// class.
fn rated_api(quota_name: &str) -> Option<&str> {
    let api = quota_name
        .strip_prefix("Rate of ")?
        .strip_suffix(" requests")?;
    let names_one_operation = !api.is_empty()
        && !api.contains(char::is_whitespace)
        && api.starts_with(char::is_uppercase)
        && api.chars().all(char::is_alphanumeric);
    names_one_operation.then_some(api)
}

/// RULE: every row of a service's per-API request-rate class is derived from the
/// operation it throttles — the one-second window, that operation's call count, and the
/// class's sentence naming it — so no row of the class borrows a neighbour's.
#[tokio::test]
async fn an_api_rate_row_publishes_the_window_metric_and_sentence_of_its_own_operation() {
    let engine = emulator();
    let caller = client(&engine, "ap-southeast-2");

    // The CloudWatch catalog, whose published rows are rates of single operations
    // beside a rate that is not.
    let service_code = "monitoring";
    let listed = defaults(&caller, service_code).await;
    let mut rate_rows = 0_usize;
    let mut other_rows = 0_usize;

    for quota in &listed {
        let name = quota["QuotaName"].as_str().unwrap_or_default();
        let code = quota["QuotaCode"].as_str().unwrap_or_default();
        let case = format!("{service_code}/{code}");

        let Some(api) = rated_api(name) else {
            other_rows += 1;
            // A row outside the class must not publish the sentence of an operation it
            // does not throttle, which is the shape of the defect.
            let described = quota["Description"].as_str().unwrap_or_default();
            for other in &listed {
                let Some(other_api) = other["QuotaName"].as_str().and_then(rated_api) else {
                    continue;
                };
                assert!(
                    !described.contains(&format!("{other_api} requests that you can make")),
                    "case {case} ({name:?}): a row must not publish another quota's sentence, \
                     got {described:?}"
                );
            }
            continue;
        };

        rate_rows += 1;
        let expected = json!({
            "Period": {"PeriodValue": 1, "PeriodUnit": "SECOND"},
            "UsageMetric": {
                "MetricNamespace": "AWS/Usage",
                "MetricName": "CallCount",
                "MetricStatisticRecommendation": "Sum",
                "MetricDimensions": {
                    "Class": "None",
                    "Resource": api,
                    "Service": "CloudWatch",
                    "Type": "API",
                },
            },
            "Description": format!(
                "The maximum number of {api} requests that you can make, per second, in this \
                 account in the current region."
            ),
        });
        let served = json!({
            "Period": quota["Period"],
            "UsageMetric": quota["UsageMetric"],
            "Description": quota["Description"],
        });
        assert_eq!(
            served, expected,
            "case {case} ({name:?}): expected the class's window, call count and sentence for \
             {api}, got {quota}"
        );
    }

    assert!(
        rate_rows >= 3 && other_rows >= 1,
        "case {service_code}: the rule must be asserted over the class and both its branches, \
         got {rate_rows} rate rows and {other_rows} others"
    );
}

/// RULE: one published `QuotaName` under two codes is two quotas — the pair is the
/// identity, so each code resolves its own record and neither read answers with the
/// other's limit.
#[tokio::test]
async fn one_published_name_under_two_codes_resolves_two_quotas() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    let shared_name = "Maximum Signin resource based policy statement size";
    let listed = defaults(&caller, "signin").await;
    let under_name: Vec<&Value> = listed
        .iter()
        .filter(|quota| quota["QuotaName"] == json!(shared_name))
        .collect();
    assert!(
        under_name.len() >= 2,
        "case signin: expected the published rows that share the name {shared_name:?}, got \
         {listed:?}"
    );

    // Each row of the group reads back under its OWN code, with its own limit and unit.
    let mut limits: BTreeSet<String> = BTreeSet::new();
    for quota in under_name {
        let code = quota["QuotaCode"]
            .as_str()
            .unwrap_or_else(|| panic!("case signin: a quota without a code: {quota}"));
        let read = caller
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": "signin", "QuotaCode": code}),
            )
            .await["Quota"]
            .clone();
        assert_eq!(
            read, *quota,
            "case signin/{code}: the single read must resolve the row published under that \
             code, got {read}"
        );
        limits.insert(format!("{}{}", quota["Value"], quota["Unit"]));
    }
    assert!(
        limits.len() >= 2,
        "case signin: two codes under one name publish two limits, got {limits:?}"
    );
}
