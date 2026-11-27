// SPDX-License-Identifier: CC0-1.0

//! A `QuotaCode` identifies ONE quota, so the row published under a code carries that
//! quota's own published facts — `QuotaName`, `Description`, `Value`, `Unit`, `Adjustable`,
//! `Period` and `UsageMetric` — and the `Description` is the service's published sentence
//! rather than a shorter re-wording of it.
//!
//! This suite pins the bindings and the sentences of the six catalogs whose codes the
//! recorded listings re-key (`dynamodb`, `ebs`, `ecr`, `elasticache`, `elasticfilesystem`
//! and `elasticloadbalancing`), and asserts the two halves of the rule:
//!
//! * BINDING: each code answers the quota that code publishes, with every member of it, and
//!   the plane a binding displaced answers a code of its own or none at all — never the code
//!   it moved off.
//! * CLASS: where a service words a FAMILY of rows one way, every row of the family carries
//!   that wording, including the rows no recording in the issue history cites: both DynamoDB
//!   account-level capacity planes, every EBS storage and IOPS plane, every ECR push-path
//!   rate, both ElastiCache per-cluster node planes and both ELB trust-store planes.
//!
//! The pinned table spans the class rather than one recording's row: counted planes beside
//! per-API rates, storage, IOPS, throughput and length planes, `None` beside a `Bytes` unit,
//! adjustable beside fixed rows, rows published over a one-second and a five-minute window
//! beside rows published over none, values from 30 to 500 000, and rows carrying each
//! `AWS/Usage` metric shape beside rows carrying none.
//!
//! SOURCE: AWS's published Service Quotas data for each of the six service codes (what
//! `aws service-quotas list-aws-default-service-quotas --service-code <code>` reports) and
//! the recorded default-quota listing of each code for which `QuotaCode` names which quota
//! and for each row's published wording, as `src/state/quota_rows/<code>.rs` carries them.
//!
//! The harness (`client`, `emulator`, `default_quotas`) is the one `quota_workflow.rs` owns.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas as defaults, emulator};

/// The six catalogs this suite covers.
const SERVICE_CODES: &[&str] = &[
    "dynamodb",
    "ebs",
    "ecr",
    "elasticache",
    "elasticfilesystem",
    "elasticloadbalancing",
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

/// The rows this suite pins: the bindings the recorded listings state for these codes, and
/// the planes the displaced ones took.
const ROWS: &[Row] = &[
    // DynamoDB publishes the write-throughput plane of a table with STREAMS enabled under
    // this code, in the capacity-units sentence of the class and closing on the streams
    // section of its Service Quotas page.
    Row {
        service_code: "dynamodb",
        quota_code: "L-923BEB7A",
        quota_name: "Write throughput limit for DynamoDB Streams (Provisioned mode)",
        value: 40_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of write capacity units allowed for a table with \
                      streams enabled; applicable only for tables in provisioned read/write \
                      capacity mode. Other quotas might also apply. For more information, see \
                      https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/\
                      ServiceQuotas.html#limits-dynamodb-streams",
        period: None,
        metric: Metric::None,
    },
    // The account-level provisioned WRITE plane, whose sentence names the capacity units and
    // the tables they are counted over.
    Row {
        service_code: "dynamodb",
        quota_code: "L-34F8CCC8",
        quota_name: "Account-level write throughput limit (Provisioned mode)",
        value: 80_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of write capacity units allocated for the account; \
                      applicable only for tables (including all associated global secondary \
                      indexes) in provisioned read/write capacity mode. For more information, \
                      see https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/\
                      Limits.html#default-limits-throughput-capacity-modes",
        period: None,
        metric: Metric::Resources {
            usage_service: "DynamoDB",
            resource: "AccountProvisionedWriteCapacityUnits",
        },
    },
    // The READ plane of the same family, which no recorded row in the issue history quotes:
    // the class sentence holds for it too.
    Row {
        service_code: "dynamodb",
        quota_code: "L-34F6A552",
        quota_name: "Account-level read throughput limit (Provisioned mode)",
        value: 80_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of read capacity units allocated for the account; \
                      applicable only for tables (including all associated global secondary \
                      indexes) in provisioned read/write capacity mode. For more information, \
                      see https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/\
                      Limits.html#default-limits-throughput-capacity-modes",
        period: None,
        metric: Metric::Resources {
            usage_service: "DynamoDB",
            resource: "AccountProvisionedReadCapacityUnits",
        },
    },
    // The secondary-index plane of a table, published with the comma after `see` the service
    // itself publishes.
    Row {
        service_code: "dynamodb",
        quota_code: "L-F7858A77",
        quota_name: "Global Secondary Indexes per table",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of global secondary indexes that can be created for \
                      a table. For more information, see, \
                      https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/\
                      ServiceQuotas.html#limits-secondary-indexes",
        period: None,
        metric: Metric::None,
    },
    // The EBS storage-MODIFICATION plane of the General Purpose SSD (gp3) type, on the code
    // the recorded listing states for it: a plane that names its TiB in its own name, so it
    // publishes no unit of its own and no window.
    Row {
        service_code: "ebs",
        quota_code: "L-59C8FC87",
        quota_name: "Storage modifications for General Purpose SSD (gp3) volumes, in TiB",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: "The maximum aggregated amount of storage, in TiB, that can be requested \
                      in volume modifications across General Purpose SSD (gp3) volumes in \
                      this Region.",
        period: None,
        metric: Metric::None,
    },
    // The snapshot plane of the Region: a plain count, far above the per-API rate the code
    // had carried.
    Row {
        service_code: "ebs",
        quota_code: "L-309BACF6",
        quota_name: "Snapshots per Region",
        value: 100_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of snapshots per Region.",
        period: None,
        metric: Metric::None,
    },
    // The IOPS-modification plane of the io1 type: the IOPS family counts a NUMBER of
    // operations where the storage family counts an AMOUNT in TiB.
    Row {
        service_code: "ebs",
        quota_code: "L-98A0B26D",
        quota_name: "IOPS modifications for Provisioned IOPS SSD (io1) volumes",
        value: 500_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum aggregated number of IOPS that can be requested in volume \
                      modifications across Provisioned IOPS SSD (io1) volumes in this Region.",
        period: None,
        metric: Metric::None,
    },
    // The snapshot-copy throughput of one destination Region, published as an account-level
    // throughput in MiB/sec.
    Row {
        service_code: "ebs",
        quota_code: "L-E137849C",
        quota_name: "Time-based snapshot copy throughput per destination Region",
        value: 2000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum account-level throughput, in MiB/sec, for time-based \
                      snapshot copy operations per destination Region.",
        period: None,
        metric: Metric::None,
    },
    // The ECR layer-upload rate of the push path, on the code the recorded listing states
    // for it: the one-second window, that operation's call count, and the clause naming when
    // the API is called during a push.
    Row {
        service_code: "ecr",
        quota_code: "L-95B28F8D",
        quota_name: "Rate of InitiateLayerUpload requests",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of InitiateLayerUpload requests that you can make \
                      per second in the current Region. When an image is pushed, the \
                      InitiateLayerUpload API is called once per image layer that has not \
                      already been uploaded. Whether or not an image layer has been uploaded \
                      is determined by the BatchCheckLayerAvailability API action.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "ECR",
            api: "InitiateLayerUpload",
        },
    },
    // The image-read rate that binding displaced, on the code it vacated and with that
    // code's published adjustability.
    Row {
        service_code: "ecr",
        quota_code: "L-C0B33BA1",
        quota_name: "Rate of BatchGetImage requests",
        value: 10_000.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of BatchGetImage requests that you can make per \
                      second in the current Region.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "ECR",
            api: "BatchGetImage",
        },
    },
    // The user-group plane of an ElastiCache Region, on the code the recorded listing states
    // for it: metered over the window a resource count is published with, under the resource
    // TYPE the count is carried as.
    Row {
        service_code: "elasticache",
        quota_code: "L-AD484FC5",
        quota_name: "User Groups per Region",
        value: 200.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of user groups you can create in a Region.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "ElastiCache",
            resource: "UserGroup",
        },
    },
    // The node plane of one MEMCACHED cluster, and below it the same sentence for the Redis
    // or Valkey cluster plane: one family, two engines.
    Row {
        service_code: "elasticache",
        quota_code: "L-8C334AD1",
        quota_name: "Nodes per cluster (Memcached)",
        value: 40.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of nodes in an individual Memcached cluster.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "ElastiCache",
            resource: "NodesPerCluster",
        },
    },
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
    // The node plane of the Region, which AWS words around the reserved and non-reserved
    // nodes its count is made up of rather than in the per-Region voice beside it.
    Row {
        service_code: "elasticache",
        quota_code: "L-DFE45DF3",
        quota_name: "Nodes per Region",
        value: 300.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of nodes across all clusters in a Region. This quota \
                      applies to both your reserved and non-reserved nodes within the given \
                      Region. You can have up to 300 reserved nodes and 300 non-reserved \
                      nodes in the same Region.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "ElastiCache",
            resource: "NodesPerRegion",
        },
    },
    // The EFS symbolic-link plane: a LENGTH, published in bytes and as fixed, closing
    // without a full stop the way every plane of this service does.
    Row {
        service_code: "elasticfilesystem",
        quota_code: "L-B8B14E21",
        quota_name: "File system symbolic link (symlink) length",
        value: 4080.0,
        unit: "Bytes",
        adjustable: false,
        description: "The maximum length that a file system symbolic link, or symlink, can be",
        period: None,
        metric: Metric::None,
    },
    // The file-system plane of the account, on the code the recorded listing states for it
    // and with that code's published adjustability.
    Row {
        service_code: "elasticfilesystem",
        quota_code: "L-848C634D",
        quota_name: "File systems per account",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of file systems that a customer account can have in \
                      an AWS Region",
        period: None,
        metric: Metric::None,
    },
    // The access-point deletion rate that binding displaced, on the code it vacated and with
    // the adjustability this service publishes its rate class with.
    Row {
        service_code: "elasticfilesystem",
        quota_code: "L-03A6A61D",
        quota_name: "Rate of DeleteAccessPoint requests",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of DeleteAccessPoint requests you can make per \
                      second in this account in the current Region.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "EFS",
            api: "DeleteAccessPoint",
        },
    },
    // The two trust-store planes of Elastic Load Balancing: the LISTS a store holds and the
    // ENTRIES those lists carry together, each capitalised in the sentence the way the
    // quota's own name capitalises it, and neither closing on a full stop.
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-332EE142",
        quota_name: "Revocation lists per trust store",
        value: 30.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of Revocation lists per trust store",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-37E1617B",
        quota_name: "Revocation entries per trust store",
        value: 500_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of Revocation entries per trust store",
        period: None,
        metric: Metric::None,
    },
    // The listener plane of a CLASSIC Load Balancer, with that plane's own resource count.
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-1A491844",
        quota_name: "Listeners per Classic Load Balancer",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of listeners per Classic Load Balancer",
        period: None,
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "ListenersPerClassicLoadBalancer",
        },
    },
    // The per-Availability Zone target plane of a Network Load Balancer.
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-B211E961",
        quota_name: "Targets per Availability Zone per Network Load Balancer",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of targets per Availability Zone per Network Load \
                      Balancer",
        period: None,
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "TargetsPerAvailabilityZonePerNetworkLoadBalancer",
        },
    },
    // The per-Region planes the trust-store and Classic-listener bindings displaced, each on
    // a code those bindings vacated and with that code's published adjustability.
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-EBC0C08B",
        quota_name: "Target groups per Region",
        value: 3000.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of target groups in this account in the current \
                      Region",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "TargetGroupsPerRegion",
        },
    },
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-53DA6B97",
        quota_name: "Network Load Balancers per Region",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of Network Load Balancers in this account in the \
                      current Region",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "NetworkLoadBalancersPerRegion",
        },
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

/// Every published fact of one record, as one comparable value.
fn facts(record: &Value) -> Value {
    json!({
        "QuotaName": record["QuotaName"],
        "Value": record["Value"],
        "Unit": record["Unit"],
        "Adjustable": record["Adjustable"],
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
async fn a_rekeyed_code_reports_its_own_quotas_facts() {
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

/// A sentence a service words for a FAMILY of rows holds on every row of that family, not
/// only on the rows a recording quotes: a row of the family cannot keep a shorter re-wording
/// of its own.
#[tokio::test]
async fn a_familys_sentence_holds_on_every_row_of_the_family() {
    let engine = emulator();
    let caller = client(&engine, "eu-central-1");

    // How many rows of each guarded family were seen, so a rule cannot pass by matching no
    // row at all.
    let mut families: BTreeMap<&str, usize> = BTreeMap::new();
    for service_code in SERVICE_CODES {
        for quota in defaults(&caller, service_code).await {
            let code = quota["QuotaCode"].as_str().unwrap_or_default();
            let name = quota["QuotaName"].as_str().unwrap_or_default();
            let text = quota["Description"].as_str().unwrap_or_default();
            let case = format!("{service_code}/{code} {name:?}");

            match *service_code {
                // Every DynamoDB plane of CAPACITY UNITS names the units, whose they are and
                // the capacity mode they apply in, and closes on a Developer Guide link.
                "dynamodb" if text.contains(" capacity units ") => {
                    *families.entry("dynamodb capacity units").or_default() += 1;
                    assert!(
                        text.contains("; applicable only for ")
                            && text.contains(" in provisioned read/write capacity mode.")
                            && text.contains(
                                "https://docs.aws.amazon.com/amazondynamodb/latest/\
                                 developerguide/"
                            ),
                        "case {case}: expected the capacity-units form, got {text:?}"
                    );
                }
                // Every EBS aggregated plane counts storage as an AMOUNT in the TiB it is
                // sold in and IOPS as a NUMBER of operations.
                "ebs" if name.contains("Storage for")
                    || name.contains("Storage modifications for")
                    || name.contains("IOPS for")
                    || name.contains("IOPS modifications for") =>
                {
                    *families.entry("ebs aggregated").or_default() += 1;
                    let counted = if name.starts_with("IOPS") {
                        "number of IOPS"
                    } else {
                        "amount of storage, in TiB,"
                    };
                    assert!(
                        text.starts_with(&format!("The maximum aggregated {counted} that can be "))
                            && text.ends_with(" volumes in this Region."),
                        "case {case}: expected the aggregated-plane form, got {text:?}"
                    );
                }
                // An ECR rate of an operation on the PUSH PATH closes on the clause naming
                // when THAT operation is called during a push, with the API named in it.
                "ecr" => {
                    if let Some(api) = name
                        .strip_prefix("Rate of ")
                        .and_then(|rest| rest.strip_suffix(" requests"))
                    {
                        let class = format!(
                            "The maximum number of {api} requests that you can make per second \
                             in the current Region."
                        );
                        if let Some(clause) = text.strip_prefix(&format!("{class} ")) {
                            *families.entry("ecr push path").or_default() += 1;
                            assert!(
                                clause.starts_with("When an image is pushed")
                                    && clause.contains(&format!("the {api} API is called"))
                                    && clause.ends_with('.'),
                                "case {case}: expected the push-path clause of this API, got \
                                 {clause:?}"
                            );
                        } else {
                            assert_eq!(
                                text, class,
                                "case {case}: expected the service's rate form, got {text:?}"
                            );
                        }
                    }
                }
                // Both ElastiCache per-cluster node planes name the ENGINE the cluster runs.
                "elasticache" if name.starts_with("Nodes per cluster") => {
                    *families.entry("elasticache nodes per cluster").or_default() += 1;
                    assert!(
                        text.starts_with("The maximum number of nodes in an individual ")
                            && text.ends_with(" cluster."),
                        "case {case}: expected the per-cluster node form, got {text:?}"
                    );
                }
                // Both ELB REVOCATION planes of a trust store word the plane the way the
                // quota's own name words it, capital included, and close without a full
                // stop.
                "elasticloadbalancing" if name.starts_with("Revocation ") => {
                    *families.entry("elb trust store").or_default() += 1;
                    assert_eq!(
                        text,
                        format!("The maximum number of {name}"),
                        "case {case}: expected the plane's own phrase, got {text:?}"
                    );
                }
                _ => {}
            }
        }
    }

    for (family, wanted) in [
        ("dynamodb capacity units", 3),
        ("ebs aggregated", 13),
        ("ecr push path", 4),
        ("elasticache nodes per cluster", 2),
        ("elb trust store", 2),
    ] {
        let seen = families.get(family).copied().unwrap_or_default();
        assert!(
            seen >= wanted,
            "case {family}: expected at least {wanted} rows of the family, got {seen}"
        );
    }
}

/// A plane whose code another binding took answers ONE code of its own, or none at all — it
/// never goes on answering the code it moved off, because that code now names another quota.
#[tokio::test]
async fn a_plane_a_rekey_displaced_never_answers_the_code_it_left() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    // `(ServiceCode, QuotaName, the code the plane moved OFF)`.
    const DISPLACED: &[(&str, &str, &str)] = &[
        ("ecr", "Rate of BatchGetImage requests", "L-95B28F8D"),
        ("elasticache", "Shards per cluster (cluster mode enabled)", "L-8C334AD1"),
        ("elasticache", "Nodes per cluster (Memcached)", "L-7D6587E6"),
        ("elasticfilesystem", "Rate of DeleteAccessPoint requests", "L-848C634D"),
        ("elasticfilesystem", "File systems per account", "L-03A6A61D"),
        ("elasticloadbalancing", "Target groups per Region", "L-332EE142"),
        ("elasticloadbalancing", "Network Load Balancers per Region", "L-1A491844"),
    ];
    // The planes no source here can cite a code for: published under NO row, rather than
    // under a neighbour's code.
    const UNCITED: &[(&str, &str)] = &[
        ("dynamodb", "Account-level read throughput limit (On-demand mode)"),
        ("ebs", "Rate of DeleteSnapshot requests"),
        ("ebs", "Rate of DescribeVolumeStatus requests"),
        ("elasticache", "Security groups per Region"),
        ("elasticloadbalancing", "Condition wildcards per rule"),
    ];

    let mut catalogs: BTreeMap<&str, BTreeMap<String, Value>> = BTreeMap::new();
    for service_code in SERVICE_CODES {
        catalogs.insert(service_code, catalog(&caller, service_code).await);
    }

    for (service_code, quota_name, vacated) in DISPLACED {
        let holders: BTreeSet<&String> = catalogs[service_code]
            .iter()
            .filter(|(_, quota)| quota["QuotaName"].as_str() == Some(*quota_name))
            .map(|(code, _)| code)
            .collect();
        assert_eq!(
            holders.len(),
            1,
            "case {service_code} {quota_name:?}: expected the plane under exactly one code, got \
             {holders:?}"
        );
        assert!(
            holders.iter().all(|code| code.as_str() != *vacated),
            "case {service_code} {quota_name:?}: the plane must not answer {vacated}, which now \
             names another quota"
        );
    }

    for (service_code, quota_name) in UNCITED {
        let holders: Vec<&String> = catalogs[service_code]
            .iter()
            .filter(|(_, quota)| quota["QuotaName"].as_str() == Some(*quota_name))
            .map(|(code, _)| code)
            .collect();
        assert!(
            holders.is_empty(),
            "case {service_code} {quota_name:?}: a plane with no code of its own is published \
             under no row, got {holders:?}"
        );
    }
}
