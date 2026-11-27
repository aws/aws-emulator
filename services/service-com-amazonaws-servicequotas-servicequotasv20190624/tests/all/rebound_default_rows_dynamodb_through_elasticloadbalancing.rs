// SPDX-License-Identifier: CC0-1.0

//! A `QuotaCode` is the IDENTITY a client resolves a limit by, so the row published under
//! one carries the facts of the quota AWS publishes under THAT code — and only under it.
//!
//! The rule asserted here, over the seven catalogs whose bindings this suite covers
//! (`dynamodb`, `ebs`, `ecr`, `eks`, `elasticache`, `elasticfilesystem` and
//! `elasticloadbalancing`):
//!
//! * BINDING: for one `ServiceCode`, every `QuotaCode` AWS publishes appears exactly once
//!   and carries that code's own `QuotaName`, `Description`, `Value`, `Unit`, `Adjustable`,
//!   `GlobalQuota`, `Period` and `UsageMetric`. A plane whose code another binding takes
//!   moves off that code rather than publishing its facts there, and a code AWS publishes
//!   answers a row rather than nothing — a code that resolves to nothing makes a quota that
//!   exists in AWS look nonexistent.
//! * CLASS: where a service words a whole family of rows one way, every row of that family
//!   carries that wording — the EBS capacity planes say what may be PROVISIONED across a
//!   volume type while the modification planes beside them say what may be REQUESTED IN
//!   VOLUME MODIFICATIONS, the EFS planes close without a full stop, the ECR push-path rates
//!   close on the clause saying when the operation is invoked during a push, and the EKS
//!   planes whose allowance is set with the cluster close on the clause saying when.
//!
//! The pinned table spans the class rather than one recording's rows: counted planes beside
//! request rates and throughput and size planes, the one-second window beside the
//! five-minute window a resource count is published over and rows with no window at all,
//! `None` beside `Bytes` and `Megabytes` units, adjustable beside fixed rows, values from 0
//! to 500 000, rows with each `AWS/Usage` metric shape and rows with none.
//!
//! SOURCE: AWS's published Service Quotas data for each of the seven service codes (what
//! `aws service-quotas list-aws-default-service-quotas --service-code <code>` reports) and
//! the recorded default-quota listing of each code for which `QuotaCode` names which quota,
//! as `src/state/quota_rows/<code>.rs` carries them.
//!
//! The harness (`client`, `emulator`, `default_quotas`, `walk_pages`) is the one
//! `quota_workflow.rs` owns.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas as defaults, emulator, walk_pages};

/// The seven catalogs this suite covers.
const SERVICE_CODES: &[&str] = &[
    "dynamodb",
    "ebs",
    "ecr",
    "eks",
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

/// The rows this suite pins: the bindings the recorded listings state, and the planes the
/// displaced ones took.
const ROWS: &[Row] = &[
    // The secondary-index plane of a DynamoDB table, which the recorded listing publishes
    // under this code: a count with no window and no usage metric.
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
    // The export-job concurrency that binding displaced, on the code it vacated and with
    // that code's published adjustability.
    Row {
        service_code: "dynamodb",
        quota_code: "L-6F30DCE1",
        quota_name: "Concurrent export jobs",
        value: 300.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of concurrent export jobs in this account in the \
                      current Region.",
        period: None,
        metric: Metric::None,
    },
    // The throughput-decrease rule of a table, published as the rule itself under the name
    // the recorded listing states.
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
    // The EBS capacity plane of one volume type: what may be PROVISIONED across the type's
    // volumes, with the TiB named in the quota's own name so the row publishes no unit.
    Row {
        service_code: "ebs",
        quota_code: "L-09BD8365",
        quota_name: "Storage for Provisioned IOPS SSD (io2) volumes, in TiB",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: "The maximum aggregated amount of storage, in TiB, that can be provisioned \
                      across Provisioned IOPS SSD (io2) volumes in this Region.",
        period: None,
        metric: Metric::None,
    },
    // The storage plane that binding displaced, on the code it vacated.
    Row {
        service_code: "ebs",
        quota_code: "L-8D977E7E",
        quota_name: "Storage for Magnetic (standard) volumes, in TiB",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: "The maximum aggregated amount of storage, in TiB, that can be provisioned \
                      across Magnetic (standard) volumes in this Region.",
        period: None,
        metric: Metric::None,
    },
    // The MODIFICATION planes beside the capacity planes: what may be requested in volume
    // modifications, for storage and for IOPS.
    Row {
        service_code: "ebs",
        quota_code: "L-9A0E0F82",
        quota_name: "Storage modifications for Provisioned IOPS SSD (io2) volumes, in TiB",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: "The maximum aggregated amount of storage, in TiB, that can be requested \
                      in volume modifications across Provisioned IOPS SSD (io2) volumes in \
                      this Region.",
        period: None,
        metric: Metric::None,
    },
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
    // The snapshot-copy THROUGHPUT plane of one destination Region, and the two codes the
    // catalog answers again: the attachment plane of a Multi-Attach enabled volume and the
    // fast-snapshot-restore plane of the Region.
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
    Row {
        service_code: "ebs",
        quota_code: "L-C125AE42",
        quota_name: "Attachments per Multi-Attach enabled volume",
        value: 16.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of instances that one Multi-Attach enabled volume can \
                      be attached to at the same time.",
        period: None,
        metric: Metric::None,
    },
    // An ECR push-path rate: the one-second window, the call count of the operation it
    // throttles, and the clause saying when the operation is invoked during a push.
    Row {
        service_code: "ecr",
        quota_code: "L-AD52DFB2",
        quota_name: "Rate of PutImage requests",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of PutImage requests that you can make per second in \
                      the current Region. When an image is pushed and all new image layers \
                      have been uploaded, the PutImage API is called once to create or update \
                      the image manifest and the tags associated with the image.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "ECR",
            api: "PutImage",
        },
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
    },
    // The pull-path rate the PutImage binding displaced, on the code it vacated.
    Row {
        service_code: "ecr",
        quota_code: "L-CFEB8E8D",
        quota_name: "Rate of GetDownloadUrlForLayer requests",
        value: 3000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of GetDownloadUrlForLayer requests that you can make \
                      per second in the current Region.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "ECR",
            api: "GetDownloadUrlForLayer",
        },
    },
    // The two EKS planes whose allowance is specified with the cluster, and the access-entry
    // plane of a cluster with the limit its code publishes.
    Row {
        service_code: "eks",
        quota_code: "L-93A74D60",
        quota_name: "Public endpoint access CIDR ranges per cluster",
        value: 40.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of public endpoint access CIDR ranges per cluster \
                      (these are specified when you create or update the cluster).",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "eks",
        quota_code: "L-C56B9FC3",
        quota_name: "Access entries per cluster",
        value: 3000.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of access entries per cluster.",
        period: None,
        metric: Metric::None,
    },
    // The ElastiCache planes of the Region: metered over the window those counts are
    // published with, under the series CloudWatch carries each count as.
    Row {
        service_code: "elasticache",
        quota_code: "L-3F15A733",
        quota_name: "Parameter groups per Region",
        value: 300.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of parameter groups you can create in a Region.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "ElastiCache",
            resource: "ParameterGroup",
        },
    },
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
    // The shard plane the Memcached node binding displaced, on the code it vacated and with
    // that code's published adjustability.
    Row {
        service_code: "elasticache",
        quota_code: "L-7D6587E6",
        quota_name: "Shards per cluster (cluster mode enabled)",
        value: 500.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of shards in one cluster with cluster mode enabled.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "ElastiCache",
            resource: "ShardsPerCluster",
        },
    },
    // Two EFS planes published in a unit of their own: the throughput of one NFS client,
    // over the one-second window, and the length of a symbolic link, in bytes.
    Row {
        service_code: "elasticfilesystem",
        quota_code: "L-06519EB8",
        quota_name: "Throughput per NFS client",
        value: 1500.0,
        unit: "Megabytes",
        adjustable: false,
        description: "The maximum throughput allowed per NFS client",
        period: Some((1, "SECOND")),
        metric: Metric::None,
    },
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
    // The ELB certificate planes, each under the code the recorded listing states for it,
    // with that plane's own resource count and as adjustable.
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-9365A611",
        quota_name: "Certificates per Application Load Balancer",
        value: 25.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of certificates per Application Load Balancer",
        period: None,
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "CertificatesPerApplicationLoadBalancer",
        },
    },
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-52964454",
        quota_name: "Certificates per Network Load Balancer",
        value: 25.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of certificates per Network Load Balancer",
        period: None,
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "CertificatesPerNetworkLoadBalancer",
        },
    },
    // The revocation ENTRIES of a trust store, a different quota from the lists themselves,
    // beside the Region's reserved capacity published with nothing reserved.
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
    // The two Gateway Load Balancer planes the certificate bindings displaced, each on a
    // code those bindings vacated.
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-69A177A2",
        quota_name: "Listeners per Gateway Load Balancer",
        value: 1.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of listeners per Gateway Load Balancer",
        period: None,
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "ListenersPerGatewayLoadBalancer",
        },
    },
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-723DCCB6",
        quota_name: "Targets per Availability Zone per Gateway Load Balancer",
        value: 300.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of targets per Availability Zone per Gateway Load \
                      Balancer",
        period: None,
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "TargetsPerAvailabilityZonePerGatewayLoadBalancer",
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

/// The facts `record` reports, as one comparable value.
fn facts(record: &Value) -> Value {
    json!({
        "QuotaName": record["QuotaName"],
        "Value": record["Value"],
        "Unit": record["Unit"],
        "Adjustable": record["Adjustable"],
        "GlobalQuota": record["GlobalQuota"],
        "Description": record["Description"],
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
async fn a_rebound_code_reports_its_own_quotas_facts() {
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

/// The window a row publishes belongs to that code's quota too: a rate carries the
/// one-second window, a metered plane of a Region the five-minute window its resource count
/// is published over, and a plane published over neither carries none.
#[tokio::test]
async fn a_rebound_code_reports_its_own_window() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

    let mut windows = 0_usize;
    for row in ROWS {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let listed = caller
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code}),
            )
            .await["Quota"]
            .clone();
        let wanted = match row.period {
            None => Value::Null,
            Some((value, unit)) => {
                windows += 1;
                json!({"PeriodValue": value, "PeriodUnit": unit})
            }
        };
        assert_eq!(
            listed.get("Period").cloned().unwrap_or(Value::Null),
            wanted,
            "case {case}: expected Period {wanted}, got {listed}"
        );
    }
    assert!(
        windows >= 5,
        "expected the table to span rows published over a window, got {windows}"
    );
}

/// EVERY code the seven catalogs publish is one AWS mints and resolves to exactly one
/// quota, through the listing and through the single default read — so a client holding a
/// code read out of AWS's catalog never finds it answering nothing, and never finds two
/// quotas under it.
#[tokio::test]
async fn every_published_code_of_the_seven_catalogs_resolves_to_one_quota() {
    let engine = emulator();
    let caller = client(&engine, "ap-south-1");

    for service_code in SERVICE_CODES {
        let quotas = walk_pages(
            &caller,
            "ListAWSDefaultServiceQuotas",
            &json!({"ServiceCode": service_code}),
            "Quotas",
            None,
        )
        .await;
        assert!(
            !quotas.is_empty(),
            "case {service_code}: expected a published catalog, got none"
        );

        let mut by_code: BTreeMap<String, String> = BTreeMap::new();
        let mut by_name: BTreeMap<String, String> = BTreeMap::new();
        for quota in &quotas {
            let code = quota["QuotaCode"].as_str().unwrap_or_default().to_string();
            let name = quota["QuotaName"].as_str().unwrap_or_default().to_string();
            let case = format!("{service_code}/{code}");
            // A code AWS mints is `L-` and eight hex characters; a stand-in built from a
            // quota's own name is not, and is not a code a client can have read anywhere.
            assert!(
                code.len() == 10
                    && code.starts_with("L-")
                    && code[2..].chars().all(|char| char.is_ascii_hexdigit()),
                "case {case}: expected a code AWS mints, got {code:?}"
            );
            if let Some(held) = by_code.insert(code.clone(), name.clone()) {
                panic!("case {case}: one code carries one quota, got both {held:?} and {name:?}");
            }
            if let Some(held) = by_name.insert(name.clone(), code.clone()) {
                panic!(
                    "case {service_code} {name:?}: one quota is published under one code, got \
                     both {held} and {code}"
                );
            }

            let single = caller
                .ok(
                    "GetAWSDefaultServiceQuota",
                    json!({"ServiceCode": service_code, "QuotaCode": code}),
                )
                .await["Quota"]
                .clone();
            assert_eq!(
                single["QuotaName"].as_str().unwrap_or_default(),
                name,
                "case {case}: the single read must resolve the same quota, got {single}"
            );
        }
    }
}

/// A family of rows a service words ONE way carries that wording on every one of its rows,
/// so a row of the family cannot drift into another family's sentence.
#[tokio::test]
async fn a_rows_family_carries_its_services_wording_on_every_row() {
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
            assert!(
                !text.is_empty(),
                "case {case}: a published row carries the quota's own sentence, got {quota}"
            );

            match *service_code {
                // The EBS capacity planes of a volume type say what may be PROVISIONED
                // across the type's volumes; the modification planes beside them say what
                // may be REQUESTED IN VOLUME MODIFICATIONS across the same volumes.
                "ebs" if name.starts_with("Storage modifications for")
                    || name.starts_with("IOPS modifications for") =>
                {
                    *families.entry("ebs modifications").or_default() += 1;
                    assert!(
                        text.contains(" that can be requested in volume modifications across ")
                            && text.ends_with(" volumes in this Region."),
                        "case {case}: expected the modification-plane form, got {text:?}"
                    );
                }
                "ebs" if name.starts_with("Storage for") || name.starts_with("IOPS for") => {
                    *families.entry("ebs capacity").or_default() += 1;
                    assert!(
                        text.contains(" that can be provisioned across ")
                            && text.ends_with(" volumes in this Region."),
                        "case {case}: expected the capacity-plane form, got {text:?}"
                    );
                }
                // Amazon EFS closes a PLANE's sentence without a full stop; its per-API
                // rates are a family of their own.
                "elasticfilesystem" if !name.starts_with("Rate of ") => {
                    *families.entry("efs planes").or_default() += 1;
                    assert!(
                        !text.ends_with('.'),
                        "case {case}: expected a plane sentence with no closing full stop, got \
                         {text:?}"
                    );
                }
                // Amazon ECR words a rate without naming the account, and a rate of an
                // operation called on the PUSH PATH closes on the clause saying when.
                "ecr" => {
                    if let Some(api) = name
                        .strip_prefix("Rate of ")
                        .and_then(|rest| rest.strip_suffix(" requests"))
                    {
                        *families.entry("ecr rates").or_default() += 1;
                        let class = format!(
                            "The maximum number of {api} requests that you can make per second \
                             in the current Region."
                        );
                        let on_push = format!("{class} When an image is pushed");
                        assert!(
                            text == class
                                || (text.starts_with(&on_push) && text.ends_with('.')),
                            "case {case}: expected {class:?}, alone or closing on the push-path \
                             clause, got {text:?}"
                        );
                    }
                }
                // Amazon EKS words every plane as a maximum number of the plane's own name,
                // and a plane whose allowance is set with the cluster closes on the clause
                // that states when.
                "eks" => {
                    assert!(
                        text.starts_with("The maximum number of "),
                        "case {case}: expected the service's counted-plane form, got {text:?}"
                    );
                    if text.contains("(these are specified when you ") {
                        *families.entry("eks specified-at").or_default() += 1;
                        assert!(
                            text.ends_with(" the cluster).")
                                && name.ends_with(" per cluster"),
                            "case {case}: expected the clause to close a per-cluster plane, got \
                             {text:?}"
                        );
                    }
                }
                // Elastic Load Balancing closes none of its sentences with a full stop.
                "elasticloadbalancing" => {
                    *families.entry("elb planes").or_default() += 1;
                    assert!(
                        !text.ends_with('.'),
                        "case {case}: expected a sentence with no closing full stop, got {text:?}"
                    );
                }
                _ => {}
            }
        }
    }

    // Each guarded family must have been seen on more than the one row a recording would
    // pin, or the rule asserted nothing.
    for (family, wanted) in [
        ("ebs capacity", 7),
        ("ebs modifications", 3),
        ("ecr rates", 14),
        ("efs planes", 10),
        ("eks specified-at", 2),
        ("elb planes", 25),
    ] {
        let seen = families.get(family).copied().unwrap_or_default();
        assert!(
            seen >= wanted,
            "case {family}: expected at least {wanted} rows of the family, got {seen}"
        );
    }
}

/// A plane whose code another binding took is published under ONE code or under none at
/// all: it never goes on answering the code it moved off, because that code now names
/// another quota.
#[tokio::test]
async fn a_displaced_plane_never_answers_the_code_it_moved_off() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    // `(ServiceCode, QuotaName, the code the plane moved OFF)` — the plane now answers a
    // code of its own, or none, and the code it left answers the quota that took it.
    const DISPLACED: &[(&str, &str, &str)] = &[
        ("dynamodb", "Concurrent export jobs", "L-F7858A77"),
        ("ebs", "Storage for Magnetic (standard) volumes, in TiB", "L-9A0E0F82"),
        ("ecr", "Rate of GetDownloadUrlForLayer requests", "L-AD52DFB2"),
        ("ecr", "Rate of BatchGetImage requests", "L-95B28F8D"),
        ("elasticache", "Nodes per cluster (Memcached)", "L-3F15A733"),
        ("elasticache", "Shards per cluster (cluster mode enabled)", "L-8C334AD1"),
        ("elasticfilesystem", "File systems per account", "L-03A6A61D"),
        ("elasticloadbalancing", "Listeners per Gateway Load Balancer", "L-52964454"),
        ("elasticloadbalancing", "Network Load Balancers per Region", "L-1A491844"),
        ("elasticloadbalancing", "Target groups per Region", "L-332EE142"),
        (
            "elasticloadbalancing",
            "Targets per Availability Zone per Gateway Load Balancer",
            "L-9365A611",
        ),
    ];
    // The planes this catalog can cite no code for at all: published under NO row, rather
    // than under a neighbour's code.
    const UNCITED: &[(&str, &str)] = &[
        ("dynamodb", "Account-level read throughput limit (On-demand mode)"),
        ("ebs", "Rate of CopySnapshot requests"),
        ("ebs", "Rate of UnlockSnapshot requests"),
        ("ebs", "Rate of ModifySnapshotAttribute requests"),
        ("ebs", "Rate of DeleteSnapshot requests"),
        ("ebs", "Rate of DescribeVolumeStatus requests"),
        ("elasticache", "Manual backups per Region"),
        ("elasticache", "Security groups per Region"),
        ("elasticfilesystem", "Total provisioned throughput per Region"),
        ("elasticfilesystem", "Rate of DescribeAccessPoints requests"),
        ("elasticloadbalancing", "Subnets per Availability Zone per load balancer"),
        ("elasticloadbalancing", "Condition wildcards per rule"),
    ];

    let mut catalogs: BTreeMap<&str, BTreeMap<String, Value>> = BTreeMap::new();
    for service_code in SERVICE_CODES {
        catalogs.insert(service_code, catalog(&caller, service_code).await);
    }

    for (service_code, quota_name, vacated) in DISPLACED {
        let rows = &catalogs[service_code];
        let holders: BTreeSet<&String> = rows
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
        let rows = &catalogs[service_code];
        let holders: Vec<&String> = rows
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
