// SPDX-License-Identifier: CC0-1.0

//! Which quota a published `QuotaCode` resolves to: the row served under a code carries
//! the facts of the quota AWS publishes UNDER THAT CODE, never a neighbouring quota's.
//!
//! `QuotaCode` is the identity a client resolves a limit by — `GetAWSDefaultServiceQuota`,
//! `GetServiceQuota` and `RequestServiceQuotaIncrease` all take it — so a catalog whose
//! rows carry the right facts in the wrong order is wrong on every one of them: the code a
//! client read out of AWS's catalog then answers with another quota's name, limit, unit,
//! adjustability, window, usage metric or context. The rule asserted here is therefore
//! about the BINDING rather than about one service's rows:
//!
//! * a published code reports the whole fact tuple of its own quota — `QuotaName`,
//!   `Value`, `Unit`, `Adjustable`, `GlobalQuota`, `Description`, `Period`, `UsageMetric`
//!   and `QuotaContext` — identically through the listing, through
//!   `GetAWSDefaultServiceQuota` and through the applied read `GetServiceQuota`;
//! * the binding is one-to-one in every covered catalog: no code carries two quotas and no
//!   quota is published under two codes, so a name can never travel to a neighbour's code.
//!
//! The table spans TEN `ServiceCode`s and the whole shape of the class rather than the
//! rows one recording showed: counted planes beside request rates and concurrency limits,
//! adjustable beside fixed rows, values from 5 to 250 000, rows with no window beside the
//! one-second rate window and the five-minute window a resource count is published over,
//! unmetered rows beside both `AWS/Usage` metric shapes (`CallCount` and `ResourceCount`),
//! and a quota counted per resource beside account-wide ones.
//!
//! SOURCE: AWS's published Service Quotas data for each `ServiceCode` (what
//! `aws service-quotas list-aws-default-service-quotas --service-code <code>` reports) and
//! the recorded default-quota listing of each code for WHICH `QuotaCode` names which
//! quota, as `src/state/quota_rows.rs` carries them.
//!
//! The harness (`client`, `emulator`, `default_quotas`, `walk_pages`) is the one
//! `quota_workflow.rs` owns.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas as defaults, emulator, walk_pages};

/// The `UsageMetric` a row publishes, where it publishes one.
enum Metric {
    /// No usage metric at all.
    None,
    /// The `AWS/Usage` `CallCount` of the API a request rate throttles.
    Calls {
        usage_service: &'static str,
        api: &'static str,
    },
    /// The `AWS/Usage` `ResourceCount` of a counted plane, under the `Resource` series
    /// CloudWatch publishes that count as.
    Resources {
        usage_service: &'static str,
        resource: &'static str,
    },
}

/// The `QuotaContext` a row publishes, for a quota AWS publishes as a quota of a resource.
struct Context {
    scope: &'static str,
    scope_type: &'static str,
    adjustable_at_level: &'static str,
}

/// One published row: the code, and every fact AWS publishes under it.
struct Row {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    description: &'static str,
    /// `(PeriodValue, PeriodUnit)`, for a row published over a window.
    period: Option<(i32, &'static str)>,
    metric: Metric,
    context: Option<Context>,
}

/// The rows this suite pins, across ten service codes.
const ROWS: &[Row] = &[
    Row {
        service_code: "ds",
        quota_code: "L-092080C3",
        quota_name: "AD Connector directories",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of AD Connector directories that you can create in \
                      the current Region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "ds",
        quota_code: "L-25146888",
        quota_name: "AWS Managed Microsoft AD domain controllers",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of domain controllers that you can add to your AWS \
                      Managed Microsoft AD directory.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "ds",
        quota_code: "L-92A19B29",
        quota_name: "AWS Managed Microsoft AD manual snapshots",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of manual snapshots that you can take for your AWS \
                      Managed Microsoft AD directory.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "ds",
        quota_code: "L-EF86B739",
        quota_name: "AWS Managed Microsoft AD directories",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of AWS Managed Microsoft AD directories that you can \
                      create in the current region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "dynamodb",
        quota_code: "L-1BB77E89",
        quota_name: "Concurrent control plane operations",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of allowed concurrent control plane operations. For \
                      more information, see \
                      https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/Limits\
                      .html#limits-api",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "dynamodb",
        quota_code: "L-34F8CCC8",
        quota_name: "Account-level write throughput limit (Provisioned mode)",
        value: 80_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of write capacity units allocated for the account; \
                      applicable only for tables (including all associated global secondary \
                      indexes) in provisioned read/write capacity mode. For more \
                      information, see \
                      https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/Limits\
                      .html#default-limits-throughput-capacity-modes",
        period: None,
        metric: Metric::Resources {
            usage_service: "DynamoDB",
            resource: "AccountProvisionedWriteCapacityUnits",
        },
        context: None,
    },
    Row {
        service_code: "ebs",
        quota_code: "L-18E976AB",
        quota_name: "ListSnapshotBlocks requests per account",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of ListSnapshotBlocks requests allowed per account.",
        period: Some((1, "SECOND")),
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "ebs",
        quota_code: "L-1774F84A",
        quota_name: "PutSnapshotBlock requests per snapshot",
        value: 1000.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of PutSnapshotBlock requests allowed per snapshot.",
        period: Some((1, "SECOND")),
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "ebs",
        quota_code: "L-39BD5252",
        quota_name: "Max concurrent copy volume operations per account",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: "Maximum number of concurrent copy volume operations allowed per account \
                      in this region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "ebs",
        quota_code: "L-F06E64A8",
        quota_name: "Storage modifications for General Purpose SSD (gp2) volumes, in TiB",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: "The maximum aggregated amount of storage, in TiB, that can be requested \
                      in volume modifications across General Purpose SSD (gp2) volumes in this \
                      Region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "ecr",
        quota_code: "L-55A41110",
        quota_name: "Rate of GetAuthorizationToken requests",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of GetAuthorizationToken requests that you can make \
                      per second in the current Region.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "ECR",
            api: "GetAuthorizationToken",
        },
        context: None,
    },
    Row {
        service_code: "ecr",
        quota_code: "L-6EF7FC96",
        quota_name: "Pull through cache rules per registry",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of pull-through cache rules.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "ecr",
        quota_code: "L-B9173138",
        quota_name: "Rate of BatchCheckLayerAvailability requests",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of BatchCheckLayerAvailability requests that you can \
                      make per second in the current Region. When an image is pushed, the \
                      BatchCheckLayerAvailability API is called once per image layer to \
                      verify if that image layer has already been uploaded.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "ECR",
            api: "BatchCheckLayerAvailability",
        },
        context: None,
    },
    Row {
        service_code: "eks",
        quota_code: "L-23414FF3",
        quota_name: "Label pairs per Fargate profile selector",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of label pairs per Fargate profile selector.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "eks",
        quota_code: "L-6AFFD1D8",
        quota_name: "Remote pod networks per cluster",
        value: 15.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of remote pod networks per cluster.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "eks",
        quota_code: "L-D78D8AF8",
        quota_name: "Selectors per Fargate profile",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of selectors per Fargate profile.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "eks",
        quota_code: "L-EA277FDC",
        quota_name: "EKS Anywhere Enterprise Subscriptions",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of EKS Anywhere Enterprise Subscriptions in this \
                      account in the current Region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "eks",
        quota_code: "L-FDFA5F81",
        quota_name: "Registered clusters",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of registered clusters in this account in the current \
                      Region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "elasticache",
        quota_code: "L-3E7F7726",
        quota_name: "Subnet groups per Region",
        value: 300.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of subnet groups you can create in a Region.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "ElastiCache",
            resource: "SubnetGroup",
        },
        context: None,
    },
    Row {
        service_code: "elasticache",
        quota_code: "L-75A7B5A4",
        quota_name: "Serverless snapshots per day per cache",
        value: 24.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of snapshots that can be taken per day per serverless \
                      cache.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "ElastiCache",
            resource: "SnapshotsPerDayPerCache",
        },
        context: None,
    },
    Row {
        service_code: "elasticache",
        quota_code: "L-943F0F1C",
        quota_name: "Users per User Group",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of users you can define for a user group.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "ElastiCache",
            resource: "UsersPerUserGroup",
        },
        context: None,
    },
    Row {
        service_code: "elasticbeanstalk",
        quota_code: "L-8EFC1C51",
        quota_name: "Environments",
        value: 200.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of environments that you can create in this account \
                      in the current Region. The limit applies across applications, not per \
                      application.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "elasticbeanstalk",
        quota_code: "L-9838E43F",
        quota_name: "Configuration templates",
        value: 2000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of configuration templates that you can create in \
                      this account in the current Region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "elasticbeanstalk",
        quota_code: "L-D64F1F14",
        quota_name: "Application versions",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of application versions that you can create in this \
                      account in the current Region. The limit applies across applications, \
                      not per application.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "elasticfilesystem",
        quota_code: "L-336A920F",
        quota_name: "Rate of file system operations",
        value: 250_000.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of file system operations per second for a file system \
                      using General Purpose performance mode. Each read operation consumes one \
                      file operation, and each write operation consumes five file operations",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "elasticfilesystem",
        quota_code: "L-4CCB99B3",
        quota_name: "Access points per file system",
        value: 10_000.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of access points allowed per file system",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-23568085",
        quota_name: "Network Load Balancer ENIs per VPC",
        value: 1200.0,
        unit: "None",
        adjustable: true,
        description: "The total number of Network Load Balancer eni's per VPC",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "NetworkLoadBalancersENIsPerVPC",
        },
        context: None,
    },
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-7EED9B64",
        quota_name: "Rules per Application Load Balancer",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of rules per Application Load Balancer",
        period: None,
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "RoutingRulesPerApplicationLoadBalancer",
        },
        context: None,
    },
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-822D1B1B",
        quota_name: "Target Groups per Application Load Balancer",
        value: 100.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of target groups per Application Load Balancer",
        period: None,
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "TargetGroupsPerApplicationLoadBalancer",
        },
        context: None,
    },
    Row {
        service_code: "es",
        quota_code: "L-076D529E",
        quota_name: "Domains per Region",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of Amazon OpenSearch Service domains you can create \
                      in each AWS Region.",
        period: None,
        metric: Metric::None,
        context: None,
    },
    Row {
        service_code: "es",
        quota_code: "L-6408ABDE",
        quota_name: "Instances per domain",
        value: 200.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of instances in a single Amazon OpenSearch Service \
                      domain. You can request an increase up to 1002 instances per domain.",
        period: None,
        metric: Metric::None,
        context: Some(Context {
            scope: "RESOURCE",
            scope_type: "AWS::OpenSearchService::Domain",
            adjustable_at_level: "ALL",
        }),
    },
    Row {
        service_code: "es",
        quota_code: "L-AE676A72",
        quota_name: "Dedicated master instances per domain",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of dedicated master instances in a single Amazon \
                      OpenSearch Service domain.",
        period: None,
        metric: Metric::None,
        context: None,
    },
];

/// The `UsageMetric` a row's [`Metric`] publishes, as the wire reports it.
fn metric_json(metric: &Metric) -> Value {
    match metric {
        Metric::None => Value::Null,
        Metric::Calls { usage_service, api } => json!({
            "MetricNamespace": "AWS/Usage",
            "MetricName": "CallCount",
            "MetricStatisticRecommendation": "Sum",
            "MetricDimensions": {
                "Service": usage_service,
                "Class": "None",
                "Resource": api,
                "Type": "API",
            },
        }),
        Metric::Resources {
            usage_service,
            resource,
        } => json!({
            "MetricNamespace": "AWS/Usage",
            "MetricName": "ResourceCount",
            "MetricStatisticRecommendation": "Maximum",
            "MetricDimensions": {
                "Service": usage_service,
                "Class": "None",
                "Resource": resource,
                "Type": "Resource",
            },
        }),
    }
}

/// The `Period` a row publishes, as the wire reports it.
fn period_json(period: Option<(i32, &str)>) -> Value {
    match period {
        None => Value::Null,
        Some((value, unit)) => json!({"PeriodValue": value, "PeriodUnit": unit}),
    }
}

/// The `QuotaContext` a row publishes, as the wire reports it.
fn context_json(context: &Option<Context>) -> Value {
    match context {
        None => Value::Null,
        Some(context) => json!({
            "ContextScope": context.scope,
            "ContextScopeType": context.scope_type,
            "AdjustableAtLevel": context.adjustable_at_level,
        }),
    }
}

/// The facts `record` reports, as one comparable value.
fn facts(record: &Value) -> Value {
    json!({
        "QuotaName": record["QuotaName"],
        "Value": record["Value"],
        "Unit": record["Unit"],
        "Adjustable": record["Adjustable"],
        "GlobalQuota": record["GlobalQuota"],
        "Description": record["Description"],
        "Period": record.get("Period").cloned().unwrap_or(Value::Null),
        "UsageMetric": record.get("UsageMetric").cloned().unwrap_or(Value::Null),
        "QuotaContext": record.get("QuotaContext").cloned().unwrap_or(Value::Null),
    })
}

/// The facts a pinned row expects, in the same shape.
fn expected_facts(row: &Row) -> Value {
    json!({
        "QuotaName": row.quota_name,
        "Value": row.value,
        "Unit": row.unit,
        "Adjustable": row.adjustable,
        "GlobalQuota": false,
        "Description": row.description,
        "Period": period_json(row.period),
        "UsageMetric": metric_json(&row.metric),
        "QuotaContext": context_json(&row.context),
    })
}

/// A published code reports the quota AWS publishes UNDER THAT CODE — the whole fact
/// tuple, and the same one through the listing, the single default read and the applied
/// read.
#[tokio::test]
async fn a_published_code_reports_the_quota_aws_publishes_under_it() {
    let engine = emulator();
    let caller = client(&engine, "us-west-2");

    // The listing of each covered service, read once and indexed by code: the rule is
    // about which row a code resolves to, so the lookup is the listing's own.
    let mut listings: BTreeMap<&str, BTreeMap<String, Value>> = BTreeMap::new();
    for row in ROWS {
        if listings.contains_key(row.service_code) {
            continue;
        }
        let indexed = defaults(&caller, row.service_code)
            .await
            .into_iter()
            .filter_map(|quota| {
                Some((quota["QuotaCode"].as_str()?.to_string(), quota))
            })
            .collect();
        listings.insert(row.service_code, indexed);
    }

    for row in ROWS {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let wanted = expected_facts(row);

        let listed = listings[row.service_code]
            .get(row.quota_code)
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the published catalog must carry this code, got {:?}",
                    listings[row.service_code].keys().collect::<Vec<_>>()
                )
            });
        assert_eq!(
            facts(listed),
            wanted,
            "case {case} in ListAWSDefaultServiceQuotas: expected {wanted}, got {listed}"
        );

        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let default = caller
            .ok("GetAWSDefaultServiceQuota", pair.clone())
            .await["Quota"]
            .clone();
        assert_eq!(
            facts(&default),
            wanted,
            "case {case} in GetAWSDefaultServiceQuota: expected {wanted}, got {default}"
        );

        // The applied plane answers from the same row while nothing is applied over it,
        // so a client that reads the limit in force resolves the same quota by the code.
        let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
        assert_eq!(
            facts(&applied),
            wanted,
            "case {case} in GetServiceQuota: expected {wanted}, got {applied}"
        );
    }
}

/// The binding of codes to quotas is ONE-TO-ONE in every covered catalog: a code carries
/// one quota and a quota is published under one code, which is what makes a code a client
/// resolves a limit by unambiguous.
#[tokio::test]
async fn every_covered_catalog_binds_one_quota_to_one_code() {
    let engine = emulator();
    let caller = client(&engine, "eu-central-1");

    let mut service_codes: Vec<&str> = Vec::new();
    for row in ROWS {
        if !service_codes.contains(&row.service_code) {
            service_codes.push(row.service_code);
        }
    }

    for service_code in service_codes {
        for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
            let quotas = walk_pages(
                &caller,
                operation,
                &json!({"ServiceCode": service_code}),
                "Quotas",
                None,
            )
            .await;
            let mut by_code: BTreeMap<String, String> = BTreeMap::new();
            let mut by_name: BTreeMap<String, String> = BTreeMap::new();
            for quota in &quotas {
                let code = quota["QuotaCode"].as_str().unwrap_or_default().to_string();
                let name = quota["QuotaName"].as_str().unwrap_or_default().to_string();
                if let Some(held) = by_code.insert(code.clone(), name.clone()) {
                    panic!(
                        "case {service_code} {operation} {code}: one code carries one quota, \
                         got both {held:?} and {name:?}"
                    );
                }
                if let Some(held) = by_name.insert(name.clone(), code.clone()) {
                    panic!(
                        "case {service_code} {operation} {name:?}: one quota is published \
                         under one code, got both {held} and {code}"
                    );
                }
            }
            assert_eq!(
                by_code.len(),
                quotas.len(),
                "case {service_code} {operation}: expected one row per code, got {quotas:?}"
            );
        }
    }
}
