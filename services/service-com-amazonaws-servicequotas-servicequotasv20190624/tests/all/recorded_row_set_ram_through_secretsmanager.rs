// SPDX-License-Identifier: CC0-1.0

//! The default-quota rows the recorded listings of `ram`, `rds`, `redshift`,
//! `route53resolver`, `s3`, `scheduler` and `secretsmanager` state, over the wire.
//!
//! THE RULE: for a `ServiceCode` AWS publishes, `ListAWSDefaultServiceQuotas` reports one
//! `ServiceQuota` per default quota that service publishes, keyed on the `QuotaCode` AWS
//! assigns the quota, and that row carries THAT quota's published `QuotaName`, `Value`,
//! `Unit`, `Adjustable`, `GlobalQuota`, `Period`, `UsageMetric` and `Description` —
//! never a neighbouring quota's, never a value scaled from one and never a sentence
//! re-worded here. A code AWS publishes resolves a quota through
//! `GetAWSDefaultServiceQuota` and through the applied plane too, so a client that read a
//! code out of AWS's catalog resolves the same limit here.
//!
//! The table below is driven over everything that rule generalises over: a plain count, a
//! count of a plane INSIDE one resource (a subnet group, a cluster, a database, a Route 53
//! Profile) beside the account's own, sizes published in `Bytes`, `Kilobytes`,
//! `Megabytes` and `Gigabytes`, a plane metered on an `AWS/Usage` `ResourceCount` series
//! over a ONE-HOUR window, a per-API rate metered on a `CallCount` series over one
//! second, a rate published over a window that no call count meters, rows AWS publishes
//! as HARD limits beside adjustable ones, and a sentence carrying the service's
//! adjustment guidance beside sentences without it.
//!
//! SOURCE for every expectation: the recorded `ListAWSDefaultServiceQuotas` response of
//! each of those service codes — which states, per code, the quota published under it and
//! its published members — beside AWS's published Service Quotas data for the service.
//! `src/state/quota_rows/<service>.rs` holds each table and names the source of every
//! member it carries.
//!
//! The harness (`client`, `emulator`, `default_quotas`) is the one `quota_workflow.rs`
//! owns.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas, emulator};

/// The `AWS/Usage` series a row's usage is counted on:
/// `(MetricName, MetricStatisticRecommendation, Service, Resource, Type)`.
type Series = Option<(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
)>;

/// One published row, as the recorded default-quota listing of its `ServiceCode` reports
/// it.
struct Row {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    description: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// `(PeriodValue, PeriodUnit)` for a quota AWS publishes over a window.
    period: Option<(i32, &'static str)>,
    usage_series: Series,
}

/// The `AWS/Usage` `ResourceCount` series of one resource plane: the largest count in the
/// window is the one that matters, so `Maximum` is the statistic published with it.
const fn resource_count(usage_service: &'static str, resource: &'static str) -> Series {
    Some(("ResourceCount", "Maximum", usage_service, resource, "Resource"))
}

/// The `AWS/Usage` `CallCount` series of one API: a rate is the calls ADDED UP over the
/// window, so `Sum` is the statistic published with it.
const fn call_count(usage_service: &'static str, api: &'static str) -> Series {
    Some(("CallCount", "Sum", usage_service, api, "API"))
}

/// The sentence Amazon EventBridge Scheduler publishes for a per-API request rate, with
/// the operation it throttles named in it.
macro_rules! scheduler_rate_sentence {
    ($api:literal) => {
        concat!(
            "Maximum ",
            $api,
            " requests per second. When you reach this quota, EventBridge Scheduler \
             rejects requests for this operation for the remainder of the interval."
        )
    };
}

/// The rows the rules below are driven over.
const PUBLISHED_ROWS: &[Row] = &[
    // ram — the account's custom-permission plane, published far above the per-resource-
    // type plane beside it.
    Row {
        service_code: "ram",
        quota_code: "L-9EBA15DD",
        quota_name: "Number of custom permissions",
        description: "The maximum number of custom permissions.",
        value: 1500.0,
        unit: "None",
        adjustable: true,
        period: None,
        usage_series: None,
    },
    // rds — planes metered on the series the service publishes each plane's count under,
    // over the ONE-HOUR window it publishes them over, beside a request-body size
    // published in megabytes as a hard limit.
    Row {
        service_code: "rds",
        quota_code: "L-5BC124EF",
        quota_name: "Read replicas per primary",
        description: "The maximum number of read replicas per primary DB instance. This quota \
                      can't be adjusted for Amazon Aurora.",
        value: 15.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "HOUR")),
        usage_series: resource_count("RDS", "ReadReplicasPerMaster"),
    },
    Row {
        service_code: "rds",
        quota_code: "L-6F3ACC36",
        quota_name: "Subnets per DB subnet group",
        description: "The maximum number of subnets per DB subnet group",
        value: 20.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "HOUR")),
        usage_series: resource_count("RDS", "SubnetsPerDBSubnetGroup"),
    },
    Row {
        service_code: "rds",
        quota_code: "L-78E853F4",
        quota_name: "Reserved DB instances",
        description: "The maximum number of reserved DB instances allowed in this account in \
                      the current AWS Region",
        value: 40.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "HOUR")),
        usage_series: resource_count("RDS", "ReservedDBInstances"),
    },
    Row {
        service_code: "rds",
        quota_code: "L-D87A28C7",
        quota_name: "Data API HTTP request body size",
        description: "The maximum size of the HTTP request body in a Data API request",
        value: 4.0,
        unit: "Megabytes",
        adjustable: false,
        period: None,
        usage_series: None,
    },
    // redshift — planes of the CLUSTER and of the database inside it, one of them measured
    // in kilobytes rather than counted.
    Row {
        service_code: "redshift",
        quota_code: "L-21B48DE6",
        quota_name: "Stored procedures in a database",
        description: "The maximum number of stored procedures for a database.",
        value: 10_000.0,
        unit: "None",
        adjustable: false,
        period: None,
        usage_series: None,
    },
    Row {
        service_code: "redshift",
        quota_code: "L-84537943",
        quota_name: "DC2 nodes in a cluster",
        description: "The maximum number of DC2 nodes that you can allocate to a cluster.",
        value: 128.0,
        unit: "None",
        adjustable: true,
        period: None,
        usage_series: None,
    },
    Row {
        service_code: "redshift",
        quota_code: "L-CA4E81F8",
        quota_name: "User-defined databases in a cluster",
        description: "The maximum number of user-defined databases that you can create per \
                      cluster.",
        value: 60.0,
        unit: "None",
        adjustable: false,
        period: None,
        usage_series: None,
    },
    Row {
        service_code: "redshift",
        quota_code: "L-D236BDB3",
        quota_name: "Size of a string value in an ION or JSON file when using an AWS Glue Data \
                     Catalog",
        description: "The maximum size of a string value in an ION or JSON file when using an \
                      AWS Glue Data Catalog.",
        value: 16.0,
        unit: "Kilobytes",
        adjustable: true,
        period: None,
        usage_series: None,
    },
    // route53resolver — the Resolver planes this service words in the "Maximum number
    // of …" form with no closing stop, beside the Route 53 Profile planes it words with
    // one.
    Row {
        service_code: "route53resolver",
        quota_code: "L-4A669CC0",
        quota_name: "Maximum number of resolver endpoints per AWS Region",
        description: "Maximum number of resolver endpoints per AWS Region",
        value: 4.0,
        unit: "None",
        adjustable: true,
        period: None,
        usage_series: None,
    },
    Row {
        service_code: "route53resolver",
        quota_code: "L-740A4B31",
        quota_name: "Domains per account",
        description: "The maximum number of domains per account.",
        value: 100_000.0,
        unit: "None",
        adjustable: true,
        period: None,
        usage_series: None,
    },
    Row {
        service_code: "route53resolver",
        quota_code: "L-8B4B9B75",
        quota_name: "Resolver rule associations to a Route 53 Profile.",
        description: "The maximum number of Resolver rule associations per Route 53 Profile.",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        period: None,
        usage_series: None,
    },
    Row {
        service_code: "route53resolver",
        quota_code: "L-94E19253",
        quota_name: "Associations between resolver rules and VPCs per AWS Region",
        description: "Maximum number of associations between resolver rules and VPCs per AWS \
                      Region",
        value: 2000.0,
        unit: "None",
        adjustable: true,
        period: None,
        usage_series: None,
    },
    Row {
        service_code: "route53resolver",
        quota_code: "L-F8A07EF1",
        quota_name: "DNS Firewall rule groups associations to a Route 53 Profile.",
        description: "The maximum number of DNS Firewall rule group associations per Route 53 \
                      Profile.",
        value: 5.0,
        unit: "None",
        adjustable: false,
        period: None,
        usage_series: None,
    },
    // s3 — the account's bucket and access-point planes beside two planes measured in a
    // SIZE, and the account-wide retrieval-request rate.
    Row {
        service_code: "s3",
        quota_code: "L-1923B91D",
        quota_name: "S3 Glacier: Retrieval request rate per second.",
        description: "The maximum number of retrieval requests per second allowed per account.",
        value: 1000.0,
        unit: "None",
        adjustable: false,
        period: None,
        usage_series: None,
    },
    Row {
        service_code: "s3",
        quota_code: "L-748707F3",
        quota_name: "Bucket policy",
        description: "The maximum size of an Amazon S3 bucket policy.",
        value: 20.0,
        unit: "Kilobytes",
        adjustable: false,
        period: None,
        usage_series: None,
    },
    Row {
        service_code: "s3",
        quota_code: "L-775A314D",
        quota_name: "Directory buckets",
        description: "The maximum number of directory buckets.",
        value: 100.0,
        unit: "None",
        adjustable: true,
        period: None,
        usage_series: None,
    },
    Row {
        service_code: "s3",
        quota_code: "L-9CD4D118",
        quota_name: "Object size (Console upload)",
        description: "The maximum size of an object that you can upload with the Amazon S3 \
                      console.",
        value: 160.0,
        unit: "Gigabytes",
        adjustable: true,
        period: None,
        usage_series: None,
    },
    Row {
        service_code: "s3",
        quota_code: "L-FAABEEBA",
        quota_name: "Access Points",
        description: "The maximum number of access points per Region.",
        value: 10_000.0,
        unit: "None",
        adjustable: true,
        period: None,
        usage_series: None,
    },
    // scheduler — per-API rates metered on each operation's call count, one of them
    // carrying the service's adjustment guidance, beside the invocation rate no call
    // count meters.
    Row {
        service_code: "scheduler",
        quota_code: "L-1DC60392",
        quota_name: "ListTagsForResource request rate",
        description: scheduler_rate_sentence!("ListTagsForResource"),
        value: 10.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "SECOND")),
        usage_series: call_count("EventBridge Scheduler", "ListTagsForResource"),
    },
    Row {
        service_code: "scheduler",
        quota_code: "L-53944E21",
        quota_name: "DeleteSchedule request rate",
        description: concat!(
            scheduler_rate_sentence!("DeleteSchedule"),
            " This is adjustable to tens of thousands of requests per second."
        ),
        value: 1000.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "SECOND")),
        usage_series: call_count("EventBridge Scheduler", "DeleteSchedule"),
    },
    Row {
        service_code: "scheduler",
        quota_code: "L-B7845AAE",
        quota_name: "UpdateSchedule request rate",
        description: concat!(
            scheduler_rate_sentence!("UpdateSchedule"),
            " This is adjustable to tens of thousands of requests per second."
        ),
        value: 1000.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "SECOND")),
        usage_series: call_count("EventBridge Scheduler", "UpdateSchedule"),
    },
    Row {
        service_code: "scheduler",
        quota_code: "L-F6D2596E",
        quota_name: "Invocations throttle limit in transactions per second",
        description: "The maximum number of schedule invocations per second per region.",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "SECOND")),
        usage_series: None,
    },
    // secretsmanager — the account's secret plane and the secret-value size, which this
    // service publishes as hard limits like the per-API rates beside them.
    Row {
        service_code: "secretsmanager",
        quota_code: "L-2F24C883",
        quota_name: "Secret value size",
        description: "The maximum size of a secret value.",
        value: 65_536.0,
        unit: "Bytes",
        adjustable: false,
        period: None,
        usage_series: None,
    },
    Row {
        service_code: "secretsmanager",
        quota_code: "L-2F66C23C",
        quota_name: "Secrets",
        description: "The maximum number of secrets in this account in the current Region.",
        value: 500_000.0,
        unit: "None",
        adjustable: false,
        period: None,
        usage_series: None,
    },
    Row {
        service_code: "secretsmanager",
        quota_code: "L-6A9CA93A",
        quota_name: "Rate of GetRandomPassword API requests",
        description: "The maximum transactions per second for GetRandomPassword API requests.",
        value: 50.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "SECOND")),
        usage_series: call_count("Secrets Manager", "GetRandomPassword"),
    },
    Row {
        service_code: "secretsmanager",
        quota_code: "L-F452D4B5",
        quota_name: "Rate of BatchGetSecretValue API requests",
        description: "The maximum transactions per second for BatchGetSecretValue API requests.",
        value: 100.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "SECOND")),
        usage_series: call_count("Secrets Manager", "BatchGetSecretValue"),
    },
];

/// The published `Period` of a row, as the wire renders it.
fn period_of(row: &Row) -> Value {
    match row.period {
        Some((value, unit)) => json!({"PeriodValue": value, "PeriodUnit": unit}),
        None => Value::Null,
    }
}

/// The published `UsageMetric` of a row, as the wire renders it.
fn usage_metric_of(row: &Row) -> Value {
    match row.usage_series {
        Some((metric_name, statistic, usage_service, resource, usage_type)) => json!({
            "MetricNamespace": "AWS/Usage",
            "MetricName": metric_name,
            "MetricStatisticRecommendation": statistic,
            "MetricDimensions": {
                "Class": "None",
                "Resource": resource,
                "Service": usage_service,
                "Type": usage_type,
            },
        }),
        None => Value::Null,
    }
}

/// RULE: the row published under a `QuotaCode` carries the quota AWS publishes under that
/// code — every member of it — and the listing, the default read and the applied read
/// report one record.
#[tokio::test]
async fn a_row_carries_the_quota_published_under_its_code() {
    let engine = emulator();
    let region = "us-west-2";
    let caller = client(&engine, region);

    for row in PUBLISHED_ROWS {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let quota = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();

        let served = json!({
            "ServiceCode": quota["ServiceCode"],
            "QuotaCode": quota["QuotaCode"],
            "QuotaArn": quota["QuotaArn"],
            "QuotaName": quota["QuotaName"],
            "Description": quota["Description"],
            "Value": quota["Value"],
            "Unit": quota["Unit"],
            "Adjustable": quota["Adjustable"],
            "GlobalQuota": quota["GlobalQuota"],
            "Period": quota["Period"],
            "UsageMetric": quota["UsageMetric"],
        });
        let published = json!({
            "ServiceCode": row.service_code,
            "QuotaCode": row.quota_code,
            "QuotaArn": format!(
                "arn:aws:servicequotas:{region}::{}/{}",
                row.service_code, row.quota_code
            ),
            "QuotaName": row.quota_name,
            "Description": row.description,
            "Value": row.value,
            "Unit": row.unit,
            "Adjustable": row.adjustable,
            "GlobalQuota": false,
            "Period": period_of(row),
            "UsageMetric": usage_metric_of(row),
        });
        assert_eq!(
            served, published,
            "case {case}: expected the quota AWS publishes under this code, got {quota}"
        );

        let listed = default_quotas(&caller, row.service_code).await;
        assert!(
            listed.contains(&quota),
            "case {case}: the whole listing must carry the record the single read returned, got \
             {listed:?}"
        );
        let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
        for member in ["QuotaName", "Description", "Value", "Unit", "Adjustable"] {
            assert_eq!(
                applied[member], quota[member],
                "case {case}: the applied read must resolve the same {member}, got {applied}"
            );
        }
    }
}

/// RULE: a `QuotaCode` identifies ONE quota of a service, and so does a `QuotaName`: the
/// published set of each of these services carries every code of the table exactly once,
/// under its own name.
#[tokio::test]
async fn a_published_code_and_name_identify_one_quota_of_the_service() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

    let services: BTreeSet<&str> = PUBLISHED_ROWS.iter().map(|row| row.service_code).collect();
    for service_code in services {
        let listed = default_quotas(&caller, service_code).await;
        let mut codes: BTreeSet<String> = BTreeSet::new();
        let mut names: BTreeSet<String> = BTreeSet::new();
        for quota in &listed {
            let code = quota["QuotaCode"].as_str().unwrap_or_default().to_string();
            let name = quota["QuotaName"].as_str().unwrap_or_default().to_string();
            assert!(
                codes.insert(code.clone()),
                "case {service_code}: a code identifies one quota, got a second row under \
                 {code:?}"
            );
            assert!(
                names.insert(name.clone()),
                "case {service_code}: a name identifies one quota, got a second row named \
                 {name:?}"
            );
        }

        for row in PUBLISHED_ROWS.iter().filter(|row| row.service_code == service_code) {
            assert!(
                codes.contains(row.quota_code),
                "case {service_code}/{}: the published set must carry the row AWS publishes \
                 under this code, got {codes:?}",
                row.quota_code
            );
        }
    }
}

/// RULE: `Adjustable` is the flag a client tests before asking for more — a quota AWS
/// publishes as a HARD limit refuses `RequestServiceQuotaIncrease`, an adjustable one
/// takes it.
#[tokio::test]
async fn adjustability_decides_whether_an_increase_is_taken() {
    let engine = emulator();
    let caller = client(&engine, "ap-southeast-2");

    let mut hard_rows = 0_usize;
    let mut adjustable_rows = 0_usize;
    for row in PUBLISHED_ROWS {
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
            adjustable_rows += 1;
            let accepted = caller.ok("RequestServiceQuotaIncrease", body).await;
            let requested = &accepted["RequestedQuota"];
            assert_eq!(
                (
                    requested["QuotaCode"].as_str(),
                    requested["DesiredValue"].as_f64(),
                ),
                (Some(row.quota_code), Some(desired)),
                "case {case}: an adjustable quota takes the increase, got {accepted}"
            );
            continue;
        }

        hard_rows += 1;
        let (status, error) = caller.refused("RequestServiceQuotaIncrease", body).await;
        assert_eq!(
            (status, error.as_str()),
            (400, "IllegalArgumentException"),
            "case {case}: a quota AWS publishes as a hard limit refuses an increase"
        );
    }

    assert!(
        hard_rows >= 8 && adjustable_rows >= 8,
        "case table: the rule must be driven over both adjustabilities, got {hard_rows} hard and \
         {adjustable_rows} adjustable rows"
    );
}

/// RULE: the published catalog is the same in every Region — only the `QuotaArn` carries
/// the Region the caller asked in.
#[tokio::test]
async fn a_published_row_reads_the_same_in_every_region() {
    let engine = emulator();

    for region in ["us-east-1", "eu-central-1"] {
        let caller = client(&engine, region);
        for row in PUBLISHED_ROWS {
            let case = format!("{region} {}/{}", row.service_code, row.quota_code);
            let quota = caller
                .ok(
                    "GetAWSDefaultServiceQuota",
                    json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code}),
                )
                .await["Quota"]
                .clone();
            assert_eq!(
                (
                    quota["QuotaName"].as_str(),
                    quota["Description"].as_str(),
                    quota["Value"].as_f64(),
                    quota["Unit"].as_str(),
                    quota["Adjustable"].as_bool(),
                    quota["QuotaArn"].as_str(),
                ),
                (
                    Some(row.quota_name),
                    Some(row.description),
                    Some(row.value),
                    Some(row.unit),
                    Some(row.adjustable),
                    Some(
                        format!(
                            "arn:aws:servicequotas:{region}::{}/{}",
                            row.service_code, row.quota_code
                        )
                        .as_str()
                    ),
                ),
                "case {case}: expected the published row with this Region's ARN, got {quota}"
            );
        }
    }
}
