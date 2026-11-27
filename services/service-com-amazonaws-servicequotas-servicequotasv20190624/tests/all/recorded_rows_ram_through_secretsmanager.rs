// SPDX-License-Identifier: CC0-1.0

//! What the default-quota row published under an AWS `QuotaCode` has to CARRY, over the
//! wire, for the eight service codes this suite spans.
//!
//! The rules asserted here, each over a TABLE of rows rather than over one quota:
//!
//! * a row published under a `QuotaCode` carries the quota AWS publishes under that code
//!   — its `QuotaName`, `Value`, `Unit`, `Adjustable`, `GlobalQuota`, the window it is
//!   published over, the `AWS/Usage` series its usage is counted on and the `Description`
//!   the OWNING SERVICE publishes for it, in that service's own wording rather than
//!   re-authored here — and the listing, the default read and the applied read all report
//!   the same record;
//! * `Adjustable` is the flag a client tests before asking for more: a quota AWS
//!   publishes as a HARD limit refuses `RequestServiceQuotaIncrease` with
//!   `IllegalArgumentException`, an adjustable one takes the request;
//! * the published set is the same in every Region, and a row's `QuotaArn` is the one
//!   ARN template with the caller's Region in it.
//!
//! The rows differ along everything those rules generalise over: a plain count, a count
//! per RESOURCE (one resource share, one snapshot, one resolver endpoint) beside the
//! account-wide count of the same thing, a plane measured in `Terabytes` and a rate
//! measured in `Gigabits`, a plane metered over a ONE-HOUR window and one metered over
//! five minutes, a per-API rate metered over one second, a rate published over several
//! operations at once that no call count meters, hard and adjustable rows, a sentence
//! with the service's adjustment guidance appended and one without, and rows this suite's
//! issue never cited (`ram`'s resource-share plane, `s3`'s bucket plane).
//!
//! SOURCE for every expectation: the recorded `ListAWSDefaultServiceQuotas` response of
//! each of these service codes, which states for each code the quota published under it
//! and the sentence published with it, beside AWS's published Service Quotas data for the
//! service. `src/state/quota_rows.rs` holds the table and names the source of each member.
//!
//! The harness (`client`, `emulator`, `default_quotas`) is the one `quota_workflow.rs`
//! owns.

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas, emulator};

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
    /// `(MetricName, MetricStatisticRecommendation, Service, Resource, Type)` of the
    /// `AWS/Usage` series the quota's usage is counted on, where it has one.
    metric: Option<(
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    )>,
}

/// The `AWS/Usage` `ResourceCount` series of one resource plane: the largest count in the
/// window is the one that matters, so `Maximum` is the statistic published with it.
const fn resource_count(
    usage_service: &'static str,
    resource: &'static str,
) -> Option<(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
)> {
    Some(("ResourceCount", "Maximum", usage_service, resource, "Resource"))
}

/// The `AWS/Usage` `CallCount` series of one API: a rate is the calls ADDED UP over the
/// window, so `Sum` is the statistic published with it.
const fn call_count(
    usage_service: &'static str,
    api: &'static str,
) -> Option<(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
)> {
    Some(("CallCount", "Sum", usage_service, api, "API"))
}

/// The rows the rules are driven over.
const PUBLISHED_ROWS: &[Row] = &[
    // ram — the planes of ONE resource share, which the service publishes as hard
    // limits, beside the account-wide planes it publishes as adjustable.
    Row {
        service_code: "ram",
        quota_code: "L-1F7F8A25",
        quota_name: "Number of resource associations per resource share",
        description: "The maximum number of resources that you can include in a resource share.",
        value: 5000.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    Row {
        service_code: "ram",
        quota_code: "L-275DAC00",
        quota_name: "Number of principal associations per resource share",
        description: "The maximum number of principals that you can specify in a resource share.",
        value: 5000.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    Row {
        service_code: "ram",
        quota_code: "L-238C96EE",
        quota_name: "Number of pending invitations",
        description: "The maximum number of pending invitations.",
        value: 250.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    Row {
        service_code: "ram",
        quota_code: "L-4A6CEE66",
        quota_name: "Number of resource associations",
        description: "The maximum number of resources associations.",
        value: 25_000.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    Row {
        service_code: "ram",
        quota_code: "L-2870BE9D",
        quota_name: "Number of custom permissions per resource type",
        description: "The maximum number of custom permissions that you can create per resource \
                      type.",
        value: 10.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    // A row the issue behind this suite never cited.
    Row {
        service_code: "ram",
        quota_code: "L-595828F9",
        quota_name: "Number of resource shares",
        description: "The maximum number of resource shares.",
        value: 25_000.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    // The account's custom-permission plane, whose published sentence, adjustability and
    // VALUE are all the recording's: pinned here so it cannot drift.
    Row {
        service_code: "ram",
        quota_code: "L-9EBA15DD",
        quota_name: "Number of custom permissions",
        description: "The maximum number of custom permissions.",
        value: 1500.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    // rds — planes metered over a ONE-HOUR window, each on the series the service
    // publishes that plane's count under, beside a concurrency plane published as hard.
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
        metric: resource_count("RDS", "ReadReplicasPerMaster"),
    },
    Row {
        service_code: "rds",
        quota_code: "L-DE55804A",
        quota_name: "Parameter groups",
        description: "The maximum number of parameter groups",
        value: 50.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "HOUR")),
        metric: resource_count("RDS", "DBParameterGroups"),
    },
    Row {
        service_code: "rds",
        quota_code: "L-E3721453",
        quota_name: "Data API maximum concurrent cluster-secret pairs",
        description: "The maximum number of unique pairs of Aurora Serverless v1 DB clusters and \
                      secrets in concurrent Data API requests for this account in the current \
                      AWS Region.",
        value: 30.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    // redshift — the node-type and node planes, which the service publishes as HARD
    // limits, and the snapshot-sharing plane of one snapshot.
    Row {
        service_code: "redshift",
        quota_code: "L-99F53FC8",
        quota_name: "Tables for 4xlarge cluster node type",
        description: "The maximum number of tables for the 4xlarge cluster node type.",
        value: 200_000.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    Row {
        service_code: "redshift",
        quota_code: "L-FA4166C9",
        quota_name: "Tables for 16xlarge cluster node type",
        description: "The maximum number of tables for the 16xlarge cluster node type.",
        value: 200_000.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    Row {
        service_code: "redshift",
        quota_code: "L-BB966966",
        quota_name: "Nodes in a cluster",
        description: "The maximum number of nodes that you can allocate to a cluster, based on \
                      the cluster's node type.",
        value: 128.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    Row {
        service_code: "redshift",
        quota_code: "L-909949E5",
        quota_name: "AWS accounts that you can authorize to restore a snapshot per snapshot",
        description: "The maximum number of AWS accounts that you can authorize to restore a \
                      snapshot, per snapshot.",
        value: 20.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    // redshift-serverless — planes published in the `Count` unit and metered over five
    // minutes, each on the SINGULAR series name the service counts one resource of the
    // plane under.
    Row {
        service_code: "redshift-serverless",
        quota_code: "L-96772D74",
        quota_name: "Serverless Namespaces",
        description: "The default number of Redshift Serverless Namespaces that are allowed in \
                      an account.",
        value: 25.0,
        unit: "Count",
        adjustable: true,
        period: Some((5, "MINUTE")),
        metric: resource_count("RedshiftServerless", "ServerlessNamespace"),
    },
    Row {
        service_code: "redshift-serverless",
        quota_code: "L-51089E7F",
        quota_name: "Serverless Workgroups",
        description: "The default number of Redshift Serverless Workgroups that are allowed in \
                      an account.",
        value: 25.0,
        unit: "Count",
        adjustable: true,
        period: Some((5, "MINUTE")),
        metric: resource_count("RedshiftServerless", "ServerlessWorkgroup"),
    },
    Row {
        service_code: "redshift-serverless",
        quota_code: "L-1854027F",
        quota_name: "Redshift Processing Units (RPUs)",
        description: "The default number of Redshift Processing Units (RPUs) that are allocated \
                      for an account.",
        value: 3200.0,
        unit: "Count",
        adjustable: true,
        period: Some((5, "MINUTE")),
        metric: resource_count("RedshiftServerless", "RedshiftProcessingUnit"),
    },
    // route53resolver — the endpoint plane the service publishes as adjustable beside the
    // rule's target plane it publishes as hard at the same count, and the Profiles plane.
    Row {
        service_code: "route53resolver",
        quota_code: "L-D2FE9758",
        quota_name: "IP addresses per resolver endpoint",
        description: "Maximum number of IP addresses per resolver endpoint",
        value: 6.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    Row {
        service_code: "route53resolver",
        quota_code: "L-D74B6237",
        quota_name: "Target IP addresses per resolver rule",
        description: "Maximum number of target IP addresses per resolver rule",
        value: 6.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    Row {
        service_code: "route53resolver",
        quota_code: "L-02CC8B74",
        quota_name: "DNS Firewall rules groups per Region",
        description: "The maximum number of DNS Firewall rules groups per Region.",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    Row {
        service_code: "route53resolver",
        quota_code: "L-1B2BDF0A",
        quota_name: "Domains in a file imported from S3",
        description: "The maximum number of domains that you can import from a single file \
                      that's stored in an Amazon S3 bucket.",
        value: 250_000.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    // The Route 53 Profiles plane, whose value is the recording's.
    Row {
        service_code: "route53resolver",
        quota_code: "L-8B4B9B75",
        quota_name: "Resolver rule associations to a Route 53 Profile.",
        description: "The maximum number of Resolver rule associations per Route 53 Profile.",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    // s3 — a plane measured in `Terabytes` that is a HARD limit of the service, a rate
    // measured in `Gigabits` per second, the lifecycle plane, and the bucket plane the
    // issue behind this suite never cited.
    Row {
        service_code: "s3",
        quota_code: "L-89BABEE8",
        quota_name: "Object size",
        description: "The maximum size (in TB) of an Amazon S3 object",
        value: 48.828_125,
        unit: "Terabytes",
        adjustable: false,
        period: None,
        metric: None,
    },
    Row {
        service_code: "s3",
        quota_code: "L-349AD9CA",
        quota_name: "Replication transfer rate",
        description: "The maximum Replication Time Control transfer rate that you can replicate \
                      from the source region in this account.",
        value: 1.0,
        unit: "Gigabits",
        adjustable: true,
        period: Some((1, "SECOND")),
        metric: None,
    },
    Row {
        service_code: "s3",
        quota_code: "L-146D5F0C",
        quota_name: "Lifecycle rules",
        description: "The maximum number of rules you can specify for an Amazon S3 lifecycle \
                      configuration",
        value: 1000.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    Row {
        service_code: "s3",
        quota_code: "L-1A9B467F",
        quota_name: "Replication Destinations",
        description: "The number of destination S3 buckets you can add for each source S3 bucket \
                      across all replication rules.",
        value: 28.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    Row {
        service_code: "s3",
        quota_code: "L-DC2B2D3D",
        quota_name: "General purpose buckets",
        description: "The maximum number of general purpose buckets.",
        value: 10_000.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    // The retrieval-request plane, whose published sentence counts the requests of the
    // whole ACCOUNT rather than of one storage class.
    Row {
        service_code: "s3",
        quota_code: "L-1923B91D",
        quota_name: "S3 Glacier: Retrieval request rate per second.",
        description: "The maximum number of retrieval requests per second allowed per account.",
        value: 1000.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    // scheduler — the listing rate the service publishes without its adjustment
    // guidance, a write rate that carries it, and the account's schedule plane.
    Row {
        service_code: "scheduler",
        quota_code: "L-66629D52",
        quota_name: "ListSchedules request rate",
        description: "Maximum ListSchedules requests per second. When you reach this quota, \
                      EventBridge Scheduler rejects requests for this operation for the \
                      remainder of the interval.",
        value: 50.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "SECOND")),
        metric: call_count("EventBridge Scheduler", "ListSchedules"),
    },
    Row {
        service_code: "scheduler",
        quota_code: "L-B7845AAE",
        quota_name: "UpdateSchedule request rate",
        description: "Maximum UpdateSchedule requests per second. When you reach this quota, \
                      EventBridge Scheduler rejects requests for this operation for the \
                      remainder of the interval. This is adjustable to tens of thousands of \
                      requests per second.",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "SECOND")),
        metric: call_count("EventBridge Scheduler", "UpdateSchedule"),
    },
    Row {
        service_code: "scheduler",
        quota_code: "L-EE5D6FF0",
        quota_name: "Number of schedules",
        description: "The maximum number of schedules per region. This quota includes one-time \
                      schedules that have completed running. We recommend configuring your \
                      schedules to automatically delete after completion using the \
                      ActionAfterCompletion feature. This is adjustable to billions of \
                      schedules.",
        value: 10_000_000.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    // secretsmanager — rates this service publishes as HARD limits, one of them over two
    // operations at once, which no single call count meters.
    Row {
        service_code: "secretsmanager",
        quota_code: "L-CBA4CAC4",
        quota_name: "Rate of CreateSecret API requests",
        description: "The maximum transactions per second for CreateSecret API requests.",
        value: 50.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "SECOND")),
        metric: call_count("Secrets Manager", "CreateSecret"),
    },
    Row {
        service_code: "secretsmanager",
        quota_code: "L-385CE7B7",
        quota_name: "Rate of DescribeSecret API requests",
        description: "The maximum transactions per second for DescribeSecret API requests.",
        value: 40_000.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "SECOND")),
        metric: call_count("Secrets Manager", "DescribeSecret"),
    },
    Row {
        service_code: "secretsmanager",
        quota_code: "L-BC8DD7C4",
        quota_name: "Combined rate of TagResource and UntagResource API requests",
        description: "The maximum transactions per second for TagResource and UntagResource API \
                      requests combined.",
        value: 50.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "SECOND")),
        metric: None,
    },
    Row {
        service_code: "secretsmanager",
        quota_code: "L-8D82A813",
        quota_name: "Combined rate of DeleteResourcePolicy, GetResourcePolicy, \
                     PutResourcePolicy, and ValidateResourcePolicy API requests",
        description: "The maximum transactions per second for DeleteResourcePolicy, \
                      GetResourcePolicy, PutResourcePolicy, and ValidateResourcePolicy API \
                      requests combined.",
        value: 50.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "SECOND")),
        metric: None,
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
fn metric_of(row: &Row) -> Value {
    match row.metric {
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
/// code — name, value, unit, adjustability, window, usage series and the owning service's
/// own sentence — and the listing, the default read and the applied read report one
/// record.
#[tokio::test]
async fn a_published_row_carries_the_quota_published_under_its_code() {
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
            "UsageMetric": metric_of(row),
        });
        assert_eq!(
            served, published,
            "case {case}: expected the quota AWS publishes under this code, got {quota}"
        );

        let listed = default_quotas(&caller, row.service_code).await;
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );
        let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
        for member in ["QuotaName", "Description", "Value", "Unit", "Adjustable"] {
            assert_eq!(
                applied[member], quota[member],
                "case {case}: the applied read must report the same {member}, got {applied}"
            );
        }
    }
}

/// RULE: `Adjustable` decides whether the service takes an increase request — a quota AWS
/// publishes as a hard limit refuses one, an adjustable quota accepts it.
#[tokio::test]
async fn a_hard_limit_refuses_an_increase_and_an_adjustable_one_takes_it() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

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
                "case {case}: an adjustable quota takes the increase it published room for, got \
                 {accepted}"
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

/// RULE: the published set does not depend on the Region the caller asks in — only the
/// `QuotaArn` carries that Region.
#[tokio::test]
async fn the_published_rows_are_the_same_in_every_region() {
    let engine = emulator();

    for region in ["us-east-1", "ap-southeast-2", "eu-central-1"] {
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
                    quota["Adjustable"].as_bool(),
                    quota["QuotaArn"].as_str(),
                ),
                (
                    Some(row.quota_name),
                    Some(row.description),
                    Some(row.value),
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
