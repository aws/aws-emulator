// SPDX-License-Identifier: CC0-1.0

//! The published rows of nine catalogs — `dynamodb`, `ebs`, `ecr`, `eks`, `elasticache`,
//! `elasticbeanstalk`, `elasticfilesystem`, `elasticloadbalancing` and `es` — carry the
//! facts of the quota published UNDER THEIR OWN `QuotaCode`, in the words that service
//! publishes them with.
//!
//! Two rules are asserted, and both are about the CATALOG rather than about one row:
//!
//! * BINDING: every member of a row — `QuotaName`, `Value`, `Unit`, `Adjustable`,
//!   `GlobalQuota`, `Description`, and `Period`/`UsageMetric` where the quota publishes
//!   them — belongs to the quota that one code names, and the binding is one-to-one, so a
//!   client that resolved a limit by a code read out of AWS's catalog cannot land on a
//!   neighbouring quota's name, limit or adjustability.
//! * WORDING: a row's `Description` and `QuotaName` are the text the service publishes,
//!   not a sentence reworded in the catalog's own voice. Each service words its rows its
//!   own way — Elastic Load Balancing closes none of its sentences with a full stop,
//!   Amazon ECR words a request rate without naming the account, Amazon DynamoDB points at
//!   its Developer Guide instead of restating the rule, OpenSearch Service names itself in
//!   full, EKS words every plane as a maximum number of the plane's own name — so the
//!   wording rule is asserted PER SERVICE over every row of that service's catalog, not
//!   only over the rows a table pins.
//!
//! The pinned table spans the class rather than one recording's rows: counted planes beside
//! request rates, rate windows beside the five-minute window a resource count is published
//! over and rows with no window at all, adjustable beside fixed rows, values from 0 to
//! 250 000, rows with each `AWS/Usage` metric shape and rows with none, and a row published
//! with nothing reserved.
//!
//! SOURCE: AWS's published Service Quotas data for each of the nine service codes (what
//! `aws service-quotas list-aws-default-service-quotas --service-code <code>` reports) and
//! the recorded default-quota listing of each code for which `QuotaCode` names which quota,
//! as `src/state/quota_rows/<code>.rs` carries them.
//!
//! The harness (`client`, `emulator`, `default_quotas`, `walk_pages`) is the one
//! `quota_workflow.rs` owns.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas as defaults, emulator, walk_pages};

/// The nine catalogs this suite covers.
const SERVICE_CODES: &[&str] = &[
    "dynamodb",
    "ebs",
    "ecr",
    "eks",
    "elasticache",
    "elasticbeanstalk",
    "elasticfilesystem",
    "elasticloadbalancing",
    "es",
];

/// The `UsageMetric` a row publishes, where it publishes one.
enum Metric {
    /// No usage metric at all.
    None,
    /// The `AWS/Usage` `CallCount` of the API a request rate throttles.
    Calls {
        usage_service: &'static str,
        api: &'static str,
    },
    /// The `AWS/Usage` `ResourceCount` of a counted plane, under the series CloudWatch
    /// publishes that count as.
    Resources {
        usage_service: &'static str,
        resource: &'static str,
    },
}

/// One published row: the code, and every fact the service publishes under it.
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
}

/// The rows this suite pins, across the nine catalogs.
const ROWS: &[Row] = &[
    // DynamoDB points the reader at the section of its Developer Guide that states the
    // limit, and closes the sentence on the link rather than on a full stop.
    Row {
        service_code: "dynamodb",
        quota_code: "L-AB614373",
        quota_name: "Table-level write throughput limit",
        value: 40_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of write throughput allocated for a table or global \
                      secondary index. For more information, see \
                      https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/Limits\
                      .html#default-limits-throughput-capacity-modes",
        period: None,
        metric: Metric::None,
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
    },
    // The throughput-DECREASE rule of a table: published as the rule itself rather than as
    // a count of something, under the code the recorded listing states for it.
    Row {
        service_code: "dynamodb",
        quota_code: "L-F3CA5463",
        quota_name: "Provisioned capacity decreases per day",
        value: 27.0,
        unit: "None",
        adjustable: true,
        description: "A decrease is allowed up to four times any time per day (GMT time zone). \
                      Also, if there was no decrease in the past hour, an additional decrease \
                      is allowed, effectively bringing the maximum number of decreases in a \
                      day to 27 times. For more information, see \
                      https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/\
                      ServiceQuotas.html",
        period: None,
        metric: Metric::None,
    },
    // An aggregated EBS plane names the unit it is counted in IN ITS OWN NAME, so it
    // publishes no `Unit` of its own.
    Row {
        service_code: "ebs",
        quota_code: "L-7A658B76",
        quota_name: "Storage for General Purpose SSD (gp3) volumes, in TiB",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "The maximum aggregated amount of storage, in TiB, that can be provisioned \
                      across General Purpose SSD (gp3) volumes in this Region.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "ebs",
        quota_code: "L-B3A130E6",
        quota_name: "IOPS for Provisioned IOPS SSD (io1) volumes",
        value: 300_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum aggregated number of IOPS that can be provisioned across \
                      Provisioned IOPS SSD (io1) volumes in this Region.",
        period: None,
        metric: Metric::None,
    },
    // The storage-modification plane of one volume type, published beside the capacity
    // plane of the same type and named for the TiB it is counted in.
    Row {
        service_code: "ebs",
        quota_code: "L-87C9DEA6",
        quota_name: "Storage modifications for Throughput Optimized HDD (st1) volumes, in TiB",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: "The maximum aggregated amount of storage, in TiB, that can be requested \
                      in volume modifications across Throughput Optimized HDD (st1) volumes \
                      in this Region.",
        period: None,
        metric: Metric::None,
    },
    // An EBS direct-API plane: a rate, over the one-second window, counted on the scope it
    // names and published with no usage metric.
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
    },
    // ECR publishes the `CompleteLayerUpload` rate under this code: the window, the call
    // count of that operation and the service's own rate wording, closing on the clause
    // that states when the operation is invoked during a push.
    Row {
        service_code: "ecr",
        quota_code: "L-44194860",
        quota_name: "Rate of CompleteLayerUpload requests",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of CompleteLayerUpload requests that you can make per \
                      second in the current Region. When an image is pushed, the \
                      CompleteLayerUpload API is called once per each new image layer to \
                      verify that the upload has completed.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "ECR",
            api: "CompleteLayerUpload",
        },
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
    },
    // The EKS rows the codes of two neighbouring planes had carried: the control-plane
    // security groups of a cluster and the clusters of the account. The security-group row
    // closes on the clause that states when the allowance is set.
    Row {
        service_code: "eks",
        quota_code: "L-11427A54",
        quota_name: "Control plane security groups per cluster",
        value: 4.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of control plane security groups per cluster (these \
                      are specified when you create the cluster).",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "eks",
        quota_code: "L-1194D53C",
        quota_name: "Clusters",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of EKS clusters in this account in the current \
                      Region.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "eks",
        quota_code: "L-5C12D558",
        quota_name: "Remote node networks per cluster",
        value: 15.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of remote node networks per cluster.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "eks",
        quota_code: "L-BD136A63",
        quota_name: "Nodes per managed node group",
        value: 450.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of nodes per managed node group.",
        period: None,
        metric: Metric::None,
    },
    // An ElastiCache plane of the Region: metered over the window those counts are
    // published with, under the resource TYPE the count is carried as.
    Row {
        service_code: "elasticache",
        quota_code: "L-80E085C7",
        quota_name: "Users per Region",
        value: 2000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of users you can create in a Region.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "ElastiCache",
            resource: "User",
        },
    },
    // A plane counted INSIDE one cluster, metered under that plane's own series.
    Row {
        service_code: "elasticache",
        quota_code: "L-AF354865",
        quota_name: "Nodes per cluster (cluster mode enabled)",
        value: 90.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of nodes in an individual Redis or Valkey cluster.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "ElastiCache",
            resource: "NodesPerCluster",
        },
    },
    // The one Elastic Beanstalk plane counted across the account's custom platforms rather
    // than per platform, which its own second sentence states.
    Row {
        service_code: "elasticbeanstalk",
        quota_code: "L-E593A077",
        quota_name: "Custom platform versions",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of custom platform versions that you can create in \
                      this account in the current Region. The limit applies across custom \
                      platforms, not per custom platform.",
        period: None,
        metric: Metric::None,
    },
    // Two planes of the file system itself: the read IOPS one file system may drive, and
    // the users one NFS client may have active at once.
    Row {
        service_code: "elasticfilesystem",
        quota_code: "L-B34D07B4",
        quota_name: "Maximum read IOPS per file system with Elastic throughput",
        value: 250_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum read IOPS for one file system with Elastic throughput mode \
                      enabled",
        period: Some((1, "SECOND")),
        metric: Metric::None,
    },
    Row {
        service_code: "elasticfilesystem",
        quota_code: "L-FA0AAA42",
        quota_name: "Active users per NFS client",
        value: 65_536.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of active users per NFS client that can have files \
                      open at the same time",
        period: None,
        metric: Metric::None,
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
    },
    // The listener plane of an Application Load Balancer, published under the code the
    // per-Availability-Zone subnet plane had carried, with the plane's own resource count.
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-B6DF7632",
        quota_name: "Listeners per Application Load Balancer",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of listeners per Application Load Balancer",
        period: None,
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "ListenersPerApplicationLoadBalancer",
        },
    },
    // The RESERVED capacity plane of the Region: published with nothing reserved, and
    // adjustable, with the capacity units named the way the quota's own name capitalises
    // them.
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-8A66D0E6",
        quota_name: "Reserved Application Load Balancer Capacity Units (LCU) per Region",
        value: 0.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of Reserved Application Load Balancer Capacity Units \
                      (LCU) per Region",
        period: None,
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "ReservedApplicationLoadBalancerCapacityUnitsPerRegion",
        },
    },
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-7E6692B2",
        quota_name: "Targets per Application Load Balancer",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of targets per Application Load Balancer",
        period: None,
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "TargetsPerApplicationLoadBalancer",
        },
    },
    // OpenSearch Service names itself in full, and publishes its burstable-instance plane
    // under the instance TYPE it restricts.
    Row {
        service_code: "es",
        quota_code: "L-E9BC8C95",
        quota_name: "Instances per domain (T2 instance type)",
        value: 10.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of T2 instances in a single Amazon OpenSearch Service \
                      domain.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "es",
        quota_code: "L-1F053E6F",
        quota_name: "Warm instances per domain",
        value: 150.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of warm nodes in a single Amazon OpenSearch Service \
                      cluster.",
        period: None,
        metric: Metric::None,
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
        "Period": match row.period {
            None => Value::Null,
            Some((value, unit)) => json!({"PeriodValue": value, "PeriodUnit": unit}),
        },
        "UsageMetric": metric_json(&row.metric),
    })
}

/// Every row of one service's default catalog, indexed by its `QuotaCode`.
async fn catalog(
    caller: &crate::quota_workflow::Client,
    service_code: &str,
) -> BTreeMap<String, Value> {
    defaults(caller, service_code)
        .await
        .into_iter()
        .filter_map(|quota| Some((quota["QuotaCode"].as_str()?.to_string(), quota)))
        .collect()
}

/// A row published under a `QuotaCode` reports the facts of THAT code's quota, through the
/// listing and through the single default read alike.
#[tokio::test]
async fn a_published_code_reports_its_own_quotas_facts() {
    let engine = emulator();
    let caller = client(&engine, "us-west-2");

    let mut listings: BTreeMap<&str, BTreeMap<String, Value>> = BTreeMap::new();
    for service_code in SERVICE_CODES {
        listings.insert(service_code, catalog(&caller, service_code).await);
    }

    for row in ROWS {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let wanted = expected_facts(row);

        let listed = listings[row.service_code]
            .get(row.quota_code)
            .unwrap_or_else(|| panic!("case {case}: the published catalog must carry this code"));
        assert_eq!(
            facts(listed),
            wanted,
            "case {case} in ListAWSDefaultServiceQuotas: expected {wanted}, got {listed}"
        );

        let single = caller
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code}),
            )
            .await["Quota"]
            .clone();
        assert_eq!(
            facts(&single),
            wanted,
            "case {case} in GetAWSDefaultServiceQuota: expected {wanted}, got {single}"
        );
    }
}

/// Each of the nine catalogs words EVERY one of its rows the way that service publishes its
/// rows, which is what makes the text the published text rather than one reworded here.
#[tokio::test]
async fn every_row_carries_its_services_published_wording() {
    let engine = emulator();
    let caller = client(&engine, "eu-central-1");

    for service_code in SERVICE_CODES {
        // How many rows this service's guarded rule matched, so a rule cannot pass by
        // matching no row at all.
        let mut matched = 0_usize;
        for quota in defaults(&caller, service_code).await {
            let code = quota["QuotaCode"].as_str().unwrap_or_default();
            let name = quota["QuotaName"].as_str().unwrap_or_default();
            let text = quota["Description"].as_str().unwrap_or_default();
            let case = format!("{service_code}/{code} {name:?}");
            assert!(
                !text.is_empty(),
                "case {case}: a published row carries the quota's own sentence, got {quota}"
            );

            match *service_code {
                // Elastic Load Balancing closes none of its sentences with a full stop.
                "elasticloadbalancing" => assert!(
                    !text.ends_with('.'),
                    "case {case}: expected a sentence with no closing full stop, got {text:?}"
                ),
                // Amazon ECR words a request rate without naming the account the rate is
                // counted on.
                "ecr" => {
                    if let Some(api) = name.strip_prefix("Rate of ").and_then(|rest| {
                        rest.strip_suffix(" requests")
                    }) {
                        matched += 1;
                        let class = format!(
                            "The maximum number of {api} requests that you can make per second \
                             in the current Region."
                        );
                        // A rate of an operation a client calls on the PUSH PATH closes on
                        // the clause that states when it is invoked during a push, which
                        // the service publishes per operation.
                        let on_push = format!("{class} When an image is pushed");
                        assert!(
                            text == class
                                || (text.starts_with(&on_push) && text.ends_with('.')),
                            "case {case}: expected {class:?}, alone or closing on the \
                             push-path clause, got {text:?}"
                        );
                    }
                }
                // Amazon DynamoDB points at the page of its Developer Guide that states the
                // limit, and ends the sentence on that link.
                "dynamodb" => {
                    if let Some((_, link)) = text.split_once("For more information, see ") {
                        matched += 1;
                        assert!(
                            link.starts_with(
                                "https://docs.aws.amazon.com/amazondynamodb/latest/\
                                 developerguide/"
                            ) && !link.ends_with('.'),
                            "case {case}: expected a Developer Guide link with no closing \
                             stop, got {text:?}"
                        );
                    }
                }
                // Amazon OpenSearch Service names itself in full on every plane counted in
                // one domain or cluster of its own.
                "es" => assert!(
                    text.contains("Amazon OpenSearch Service"),
                    "case {case}: expected the service named in full, got {text:?}"
                ),
                // Amazon EKS words every plane as a maximum number of the plane's own name.
                "eks" => assert!(
                    text.starts_with("The maximum number of "),
                    "case {case}: expected the service's counted-plane form, got {text:?}"
                ),
                // Elastic Beanstalk words every plane as what an account may create in the
                // Region.
                "elasticbeanstalk" => assert!(
                    text.starts_with("The maximum number of ")
                        && text.contains("that you can create in this account in the current \
                                           Region."),
                    "case {case}: expected the service's created-plane form, got {text:?}"
                ),
                // Amazon ElastiCache words a plane of the Region as what an account may
                // create in a Region, without naming the account. The node plane of the
                // Region is the one AWS words around the reserved and non-reserved nodes
                // the count is made up of instead, so it states its own sentence.
                "elasticache" => {
                    if name.ends_with(" per Region") && name != "Nodes per Region" {
                        matched += 1;
                        assert!(
                            text.ends_with(" you can create in a Region.")
                                && !text.contains("in this account"),
                            "case {case}: expected the service's per-Region form, got {text:?}"
                        );
                    }
                }
                // Amazon EBS words an aggregated plane as what may be requested across the
                // volumes of one type in the Region, counting storage as an AMOUNT in the
                // TiB it is sold in and IOPS as a NUMBER of operations.
                "ebs" if name.starts_with("Storage for")
                    || name.starts_with("Storage modifications for")
                    || name.starts_with("IOPS for")
                    || name.starts_with("IOPS modifications for") =>
                {
                    matched += 1;
                    let counted = if name.starts_with("IOPS") {
                        "number of IOPS"
                    } else {
                        "amount of storage, in TiB,"
                    };
                    assert!(
                        text.starts_with(&format!("The maximum aggregated {counted} that can be "))
                            && text.ends_with(" volumes in this Region."),
                        "case {case}: expected the service's aggregated-plane form, got \
                         {text:?}"
                    );
                }
                _ => {}
            }
        }
        // The services whose rule is GUARDED by a name or a phrase: the guard must have
        // matched the rows that carry the form, or the rule asserted nothing.
        let wanted_matches = match *service_code {
            "dynamodb" => 6,
            "ebs" => 10,
            "ecr" => 14,
            "elasticache" => 5,
            _ => 0,
        };
        assert!(
            matched >= wanted_matches,
            "case {service_code}: expected at least {wanted_matches} rows of the service's \
             guarded form, got {matched}"
        );
    }
}

/// A `Unit` is the unit the quota is MEASURED in, so a plane that names the unit it is
/// counted in IN ITS OWN NAME — the way the EBS capacity and capacity-modification planes
/// name their TiB — is a count, and publishes `None`.
#[tokio::test]
async fn a_plane_that_names_its_unit_in_its_name_publishes_no_unit() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    // Every plane of the nine catalogs whose own name states the unit it is counted in.
    let mut named_its_unit = 0_usize;
    for service_code in SERVICE_CODES {
        for quota in defaults(&caller, service_code).await {
            let code = quota["QuotaCode"].as_str().unwrap_or_default();
            let name = quota["QuotaName"].as_str().unwrap_or_default();
            if !name.contains(", in TiB") {
                continue;
            }
            named_its_unit += 1;
            let unit = quota["Unit"].as_str().unwrap_or_default();
            assert_eq!(
                unit, "None",
                "case {service_code}/{code} {name:?}: a name that states its own unit \
                 publishes the count unit, got {unit:?}"
            );
        }
    }
    assert!(
        named_its_unit >= 8,
        "expected the capacity planes whose names state their unit, got {named_its_unit}"
    );
}

/// `Adjustable` is published PER CODE: it says whether an increase can be requested for
/// THAT quota at all, so it follows the code's own quota rather than the class its row is
/// built from — a service publishes part of one class as adjustable and part of it not.
///
/// SOURCE: the recorded default-quota listings of `ebs`, `ecr` and `elasticfilesystem`,
/// which report these codes' adjustability. The table pairs rows of ONE class that are
/// published both ways (the EBS and ECR rate planes, the EFS rate planes), so a class
/// constructor's default cannot satisfy it.
const ADJUSTABILITY: &[(&str, &str, bool)] = &[
    ("ebs", "L-1D4D9345", false),
    ("ebs", "L-835364B2", false),
    ("ebs", "L-8656991D", false),
    ("ebs", "L-028ACFB9", false),
    ("ebs", "L-39BD5252", true),
    ("ebs", "L-7A658B76", true),
    ("ecr", "L-0A556EFC", false),
    ("ecr", "L-16E70933", true),
    ("ecr", "L-8BE1781E", false),
    ("ecr", "L-9B60BFFB", false),
    ("ecr", "L-EF8F6F4E", false),
    ("ecr", "L-44194860", true),
    ("ecr", "L-55A41110", true),
    ("ecr", "L-B9173138", true),
    ("elasticfilesystem", "L-03A6A61D", false),
    ("elasticfilesystem", "L-06519EB8", false),
    ("elasticfilesystem", "L-336A920F", false),
    ("elasticfilesystem", "L-48B15094", false),
    ("elasticfilesystem", "L-CDCE244F", false),
    ("elasticfilesystem", "L-FA0AAA42", false),
    ("elasticfilesystem", "L-848C634D", true),
    ("elasticfilesystem", "L-B34D07B4", true),
    ("eks", "L-11427A54", false),
    ("eks", "L-93A74D60", false),
    ("eks", "L-C56B9FC3", false),
    ("eks", "L-1194D53C", true),
    ("eks", "L-6D54EA21", true),
    ("es", "L-1F053E6F", false),
    ("es", "L-076D529E", true),
    ("elasticloadbalancing", "L-8A66D0E6", true),
    ("elasticloadbalancing", "L-B6DF7632", true),
];

/// Whether an increase can be requested is reported for the quota the code names, the same
/// through the listing and through the single default read — which is what decides whether
/// a client attempts `RequestServiceQuotaIncrease` at all.
#[tokio::test]
async fn adjustability_is_the_published_fact_of_the_code() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

    let mut listings: BTreeMap<&str, BTreeMap<String, Value>> = BTreeMap::new();
    for (service_code, _, _) in ADJUSTABILITY {
        if !listings.contains_key(service_code) {
            listings.insert(service_code, catalog(&caller, service_code).await);
        }
    }

    for (service_code, quota_code, adjustable) in ADJUSTABILITY {
        let case = format!("{service_code}/{quota_code}");
        let listed = listings[service_code]
            .get(*quota_code)
            .unwrap_or_else(|| panic!("case {case}: the published catalog must carry this code"));
        assert_eq!(
            listed["Adjustable"],
            Value::Bool(*adjustable),
            "case {case} in ListAWSDefaultServiceQuotas: expected Adjustable {adjustable}, \
             got {listed}"
        );

        let single = caller
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            )
            .await["Quota"]
            .clone();
        assert_eq!(
            single["Adjustable"],
            Value::Bool(*adjustable),
            "case {case} in GetAWSDefaultServiceQuota: expected Adjustable {adjustable}, \
             got {single}"
        );
    }
}

/// The binding is ONE-TO-ONE in all nine catalogs: one code carries one quota, one quota is
/// published under one code, and every row is keyed on a published `L-` code rather than on
/// a stand-in — so a code a client read out of AWS's catalog resolves here to the same
/// quota, through the default listing and through the applied listing alike.
#[tokio::test]
async fn the_nine_catalogs_bind_one_quota_to_one_code() {
    let engine = emulator();
    let caller = client(&engine, "ap-south-1");

    for service_code in SERVICE_CODES {
        for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
            let quotas = walk_pages(
                &caller,
                operation,
                &json!({"ServiceCode": service_code}),
                "Quotas",
                None,
            )
            .await;
            assert!(
                !quotas.is_empty(),
                "case {service_code} {operation}: expected a published catalog, got none"
            );

            let mut by_code: BTreeMap<String, String> = BTreeMap::new();
            let mut by_name: BTreeMap<String, String> = BTreeMap::new();
            for quota in &quotas {
                let code = quota["QuotaCode"].as_str().unwrap_or_default().to_string();
                let name = quota["QuotaName"].as_str().unwrap_or_default().to_string();
                assert!(
                    code.starts_with("L-"),
                    "case {service_code} {operation} {name:?}: expected a published code, got \
                     {code:?}"
                );
                if let Some(held) = by_code.insert(code.clone(), name.clone()) {
                    panic!(
                        "case {service_code} {operation} {code}: one code carries one quota, \
                         got both {held:?} and {name:?}"
                    );
                }
                if let Some(held) = by_name.insert(name.clone(), code.clone()) {
                    panic!(
                        "case {service_code} {operation} {name:?}: one quota is published under \
                         one code, got both {held} and {code}"
                    );
                }
            }
        }
    }
}
