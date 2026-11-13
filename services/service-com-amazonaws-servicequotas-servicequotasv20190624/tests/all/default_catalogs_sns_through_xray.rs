// SPDX-License-Identifier: CC0-1.0

//! What `ListAWSDefaultServiceQuotas` publishes for the ten `ServiceCode`s whose default
//! catalogs are rate-and-plane catalogs: notification, queueing, sign-on, gateway,
//! time-series, networking, firewall, desktop and tracing services.
//!
//! `QuotaCode` is the only stable key Service Quotas exposes — `GetAWSDefaultServiceQuota`,
//! `GetServiceQuota` and `RequestServiceQuotaIncrease` all take it — so the rules asserted
//! here are about the IDENTITY and the CONTENT of a service's published catalog rather than
//! about one recorded listing:
//!
//! * a service's whole published catalog is served, every row keyed on the `L-XXXXXXXX`
//!   code AWS mints for that quota, with the service's own `ServiceCode`/`ServiceName`, a
//!   description on every row and one key per quota;
//! * a code this catalog does not publish resolves NOWHERE — neither a code built out of a
//!   quota's own name nor a minted code the service publishes no quota under — so a client
//!   enumerating the catalog is never handed a key no other API accepts;
//! * a published row reports the `QuotaName`, `Value`, `Unit`, `Adjustable`, `GlobalQuota`,
//!   `Period`, `UsageMetric` and `QuotaContext` AWS publishes for the quota, with a
//!   `QuotaArn` naming the Region it was read at and no account, identically through the
//!   listing and through `GetAWSDefaultServiceQuota`, and the same key resolves in the
//!   APPLIED plane.
//!
//! Each rule is driven over a TABLE of ten `ServiceCode`s whose catalogs differ in length
//! (8 rows to 48), in what they count (topic, queue, permission-set, gateway, table,
//! network, web-ACL, desktop and trace planes) and in which members their rows carry; the
//! pinned rows span counts and measured quotas (`None`, `Bytes`, `Seconds`, `Hours`,
//! `Days`, `Kilobytes`, `TiB`), adjustable and fixed rows, no window / a one-second rate
//! window / the five-minute window a resource count is published over, unmetered rows
//! beside both `AWS/Usage` metric shapes (`CallCount` and `ResourceCount`), and a quota
//! published as adjustable for an individual resource as well as account-wide.
//!
//! The harness (`client`, `emulator`, `default_quotas`, `walk_pages`) is the one
//! `quota_workflow.rs` owns, and "is this a code AWS mints" is asked through
//! `catalog_identity.rs`'s single copy of that question.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::catalog_identity::is_aws_minted_shape;
use crate::quota_workflow::{
    client, default_quotas as defaults, emulator, service_inventory, walk_pages, Client,
};

/// One service whose published default-quota catalog is keyed entirely on AWS's codes.
struct Catalog {
    service_code: &'static str,
    /// The display name AWS's own Service Quotas data reports for the code, which every
    /// row of the service repeats as its `ServiceName`.
    service_name: &'static str,
    /// How many quotas the service's published catalog holds.
    quota_count: usize,
    /// Codes no quota of this service is published under: the descriptive stand-in a
    /// quota name yields (`src/state.rs::descriptive_quota_code`), which AWS publishes
    /// nothing under, and a minted code this service publishes no quota under.
    withdrawn: &'static [&'static str],
}

/// The services this suite covers.
///
/// SOURCE: AWS's published Service Quotas data for each of these service codes (what
/// `aws service-quotas list-aws-default-service-quotas --service-code <code>` reports) —
/// `quota_count` is the number of quotas that catalog publishes. `withdrawn` names the
/// codes this emulator's catalog used to key these services' rows on, which AWS publishes
/// no quota under.
const CATALOGS: &[Catalog] = &[
    Catalog {
        service_code: "sns",
        service_name: "Amazon Simple Notification Service (Amazon SNS)",
        quota_count: 48,
        withdrawn: &[
            "TopicsPerAccount",
            "SubscriptionsPerTopic",
            "PendingSubscriptionsPerAccount",
        ],
    },
    Catalog {
        service_code: "sqs",
        service_name: "Amazon Simple Queue Service (Amazon SQS)",
        quota_count: 18,
        withdrawn: &[
            "FIFOQueueTransactionsPerSecond",
            "MessagesInFlightPerStandardQueue",
        ],
    },
    Catalog {
        service_code: "sso",
        service_name: "AWS IAM Identity Center",
        quota_count: 8,
        withdrawn: &["PermissionSets"],
    },
    Catalog {
        service_code: "storagegateway",
        service_name: "AWS Storage Gateway",
        quota_count: 26,
        withdrawn: &["GatewaysPerRegion"],
    },
    Catalog {
        service_code: "timestream",
        service_name: "Amazon Timestream",
        quota_count: 29,
        // Beside the descriptive stand-in: the two codes this catalog used to key the
        // scheduled-query and batch-load creation rates on, which the recorded listing
        // publishes those two rates under OTHER codes, so no quota of this service is
        // attributable to them.
        withdrawn: &["DatabasesPerAccount", "L-0C6E4BCB", "L-16B72159"],
    },
    Catalog {
        service_code: "vpc",
        service_name: "Amazon Virtual Private Cloud (Amazon VPC)",
        quota_count: 32,
        // The two stand-ins, beside a minted code AWS publishes no VPC quota under: the
        // network-interface plane of the Region is published under `L-DF5E4CA3`.
        withdrawn: &["PrefixListsPerRegion", "SecurityGroupsPerVPC", "L-045E6E9C"],
    },
    Catalog {
        service_code: "vpc-lattice",
        service_name: "Amazon VPC Lattice",
        quota_count: 17,
        withdrawn: &["ServiceNetworksPerAccount"],
    },
    Catalog {
        service_code: "wafv2",
        service_name: "AWS WAF",
        quota_count: 34,
        withdrawn: &["WebACLsPerRegion", "RuleGroupsPerRegion"],
    },
    Catalog {
        service_code: "workspaces",
        service_name: "Amazon WorkSpaces",
        quota_count: 38,
        withdrawn: &["WorkSpaces"],
    },
    Catalog {
        service_code: "xray",
        service_name: "AWS X-Ray",
        quota_count: 11,
        withdrawn: &["SamplingRules"],
    },
];

/// The `UsageMetric` a pinned row publishes, where it publishes one.
enum Metric {
    /// No usage metric at all.
    None,
    /// The `AWS/Usage` `CallCount` of the API a request rate throttles, under the
    /// `Service` dimension that count is published with.
    Calls {
        usage_service: &'static str,
        api: &'static str,
    },
    /// The `AWS/Usage` `ResourceCount` of a counted plane, under the `Service` dimension
    /// and the `Resource` series that count is published with.
    Resources {
        usage_service: &'static str,
        resource: &'static str,
    },
}

/// One published row, with every member AWS publishes for it.
struct Row {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    description: &'static str,
    /// `(PeriodValue, PeriodUnit)` — the window the quota is published over, for a row
    /// that is published over one.
    period: Option<(i32, &'static str)>,
    metric: Metric,
    /// The resource type the quota's per-resource plane applies to, for a row AWS
    /// publishes as adjustable for an individual resource as well as account-wide.
    resource_scope_type: Option<&'static str>,
}

/// The rows this suite pins, at most two per service so the table spans the ten catalogs
/// rather than repeating one.
///
/// SOURCE: AWS's published Service Quotas data for each `ServiceCode` (what
/// `aws service-quotas list-aws-default-service-quotas --service-code <code>` reports).
/// Only one of the twenty rows names a quota the reported instances cited, so the rule is
/// asserted on quotas beyond them.
const ROWS: &[Row] = &[
    Row {
        service_code: "sns",
        quota_code: "L-AB928142",
        quota_name: "Publish Transactions per Second",
        value: 30_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of Publish requests you can make per second in this \
                      account in the current Region.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "SNS",
            api: "Publish",
        },
        resource_scope_type: None,
    },
    Row {
        service_code: "sns",
        quota_code: "L-A228E0EB",
        quota_name: "Message Size",
        value: 262_144.0,
        unit: "Bytes",
        adjustable: false,
        description: "The size, in bytes, of a published message.",
        period: None,
        metric: Metric::None,
        resource_scope_type: None,
    },
    Row {
        service_code: "sqs",
        quota_code: "L-F61F33C3",
        quota_name: "Principals per Queue Policy",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: "The number of principals in a queue policy.",
        period: None,
        metric: Metric::None,
        resource_scope_type: None,
    },
    Row {
        service_code: "sqs",
        quota_code: "L-E8A9C91E",
        quota_name: "Visibility Timeout",
        value: 43_200.0,
        unit: "Seconds",
        adjustable: false,
        description: "The time, in seconds, that a received message is hidden from the other \
                      consumers of the queue.",
        period: None,
        metric: Metric::None,
        resource_scope_type: None,
    },
    Row {
        service_code: "sso",
        quota_code: "L-B44C7A29",
        quota_name: "Permission sets",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of permission sets in an IAM Identity Center instance.",
        period: None,
        metric: Metric::None,
        resource_scope_type: None,
    },
    Row {
        service_code: "sso",
        quota_code: "L-89954265",
        quota_name: "Inline policy size per permission set",
        value: 32_768.0,
        unit: "Bytes",
        adjustable: false,
        description: "The maximum size, in bytes, of the inline policy of a permission set.",
        period: None,
        metric: Metric::None,
        resource_scope_type: None,
    },
    Row {
        service_code: "storagegateway",
        quota_code: "L-14F83003",
        quota_name: "Gateways per Region",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of gateways in this account in the current Region.",
        period: None,
        metric: Metric::None,
        resource_scope_type: None,
    },
    Row {
        service_code: "storagegateway",
        quota_code: "L-2EC26EAB",
        quota_name: "Maximum size of a cached volume",
        value: 32.0,
        unit: "TiB",
        adjustable: false,
        description: "The maximum size, in TiB, of one cached volume.",
        period: None,
        metric: Metric::None,
        resource_scope_type: None,
    },
    Row {
        service_code: "timestream",
        quota_code: "L-F07A9AB4",
        quota_name: "ListBatchLoadTasks request rate",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: "Maximum number of ListBatchLoadTasks requests per second.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "Timestream",
            api: "ListBatchLoadTasks",
        },
        resource_scope_type: None,
    },
    Row {
        service_code: "timestream",
        quota_code: "L-A83B13DB",
        quota_name: "Memory store retention",
        value: 8766.0,
        unit: "Hours",
        adjustable: true,
        description: "The maximum time, in hours, that a table retains data in its memory store.",
        period: None,
        metric: Metric::None,
        resource_scope_type: None,
    },
    // The rule plane of ONE security group: metered by the count CloudWatch publishes for
    // it, and adjustable for an individual security group as well as account-wide.
    Row {
        service_code: "vpc",
        quota_code: "L-0EA8095F",
        quota_name: "Inbound or outbound rules per security group",
        value: 60.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of inbound or outbound rules per security group. This \
                      quota counts the inbound rules and the outbound rules of a security group \
                      separately.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "EC2",
            resource: "RulesPerSecurityGroup",
        },
        resource_scope_type: Some("AWS::EC2::SecurityGroup"),
    },
    Row {
        service_code: "vpc",
        quota_code: "L-DF5E4CA3",
        quota_name: "Network interfaces per Region",
        value: 5000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of network interfaces in this account in the current \
                      Region.",
        period: None,
        metric: Metric::None,
        resource_scope_type: None,
    },
    Row {
        service_code: "vpc-lattice",
        quota_code: "L-87FCA9B2",
        quota_name: "Service networks per account",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of service networks in this account in the current \
                      Region.",
        period: None,
        metric: Metric::None,
        resource_scope_type: None,
    },
    Row {
        service_code: "vpc-lattice",
        quota_code: "L-620C821E",
        quota_name: "Requests per second per service",
        value: 10_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of requests per second that one service serves.",
        period: Some((1, "SECOND")),
        metric: Metric::None,
        resource_scope_type: None,
    },
    Row {
        service_code: "wafv2",
        quota_code: "L-5B2F9B11",
        quota_name: "Web ACL capacity units (WCU) per web ACL",
        value: 1500.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of web ACL capacity units used by one web ACL.",
        period: None,
        metric: Metric::None,
        resource_scope_type: Some("AWS::WAFv2::WebACL"),
    },
    Row {
        service_code: "wafv2",
        quota_code: "L-D6DA96AE",
        quota_name: "Rate of GetSampledRequests requests",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of GetSampledRequests requests you can make per second \
                      in this account in the current Region.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "WAFV2",
            api: "GetSampledRequests",
        },
        resource_scope_type: None,
    },
    Row {
        service_code: "workspaces",
        quota_code: "L-011B3C20",
        quota_name: "WorkSpaces",
        value: 1.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of WorkSpaces in this account in the current Region.",
        period: None,
        metric: Metric::None,
        resource_scope_type: None,
    },
    Row {
        service_code: "workspaces",
        quota_code: "L-D6CFBE96",
        quota_name: "Rate of DescribeWorkspaces requests",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of DescribeWorkspaces requests you can make per second \
                      in this account in the current Region.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "WorkSpaces",
            api: "DescribeWorkspaces",
        },
        resource_scope_type: None,
    },
    Row {
        service_code: "xray",
        quota_code: "L-8C0C998A",
        quota_name: "Sampling rules",
        value: 10_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of sampling rules in this account in the current Region.",
        period: None,
        metric: Metric::None,
        resource_scope_type: None,
    },
    Row {
        service_code: "xray",
        quota_code: "L-D781C0FD",
        quota_name: "Trace document size",
        value: 64.0,
        unit: "Kilobytes",
        adjustable: false,
        description: "The maximum size, in KB, of one trace segment document.",
        period: None,
        metric: Metric::None,
        resource_scope_type: None,
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

/// Every `QuotaCode` a listing of `service_code` reports.
async fn listed_codes(caller: &Client, operation: &str, service_code: &str) -> BTreeSet<String> {
    walk_pages(
        caller,
        operation,
        &json!({"ServiceCode": service_code}),
        "Quotas",
        None,
    )
    .await
    .iter()
    .filter_map(|quota| quota["QuotaCode"].as_str().map(str::to_string))
    .collect()
}

/// Every covered service serves its whole published catalog, keyed only on codes AWS
/// mints, each row described and reachable by a key of its own.
#[tokio::test]
async fn every_covered_service_serves_its_published_catalog() {
    let engine = emulator();

    // Two endpoints, because the catalog is published per Region and the rule is about
    // the catalog rather than about one Region's view of it.
    for region in ["us-east-2", "eu-west-3"] {
        let caller = client(&engine, region);
        let inventory: BTreeMap<String, String> = service_inventory(&caller)
            .await
            .iter()
            .filter_map(|service| {
                Some((
                    service["ServiceCode"].as_str()?.to_string(),
                    service["ServiceName"].as_str()?.to_string(),
                ))
            })
            .collect();

        for catalog in CATALOGS {
            let case = format!("{region} {}", catalog.service_code);
            let quotas = defaults(&caller, catalog.service_code).await;

            assert_eq!(
                quotas.len(),
                catalog.quota_count,
                "case {case}: expected the whole published catalog ({} quotas), got {} ({:?})",
                catalog.quota_count,
                quotas.len(),
                quotas
                    .iter()
                    .filter_map(|quota| quota["QuotaCode"].as_str())
                    .collect::<Vec<_>>()
            );

            let mut names: BTreeSet<&str> = BTreeSet::new();
            for quota in &quotas {
                let code = quota["QuotaCode"].as_str().unwrap_or_default();
                let row = format!("{case} {code}");
                assert!(
                    is_aws_minted_shape(code),
                    "case {row}: every published row is keyed on the code AWS assigns the \
                     quota, got {code:?} in {quota}"
                );
                assert_eq!(
                    (
                        quota["ServiceCode"].as_str(),
                        quota["ServiceName"].as_str()
                    ),
                    (Some(catalog.service_code), Some(catalog.service_name)),
                    "case {row}: expected the service's own code and published name, got {quota}"
                );
                assert!(
                    !quota["Description"].as_str().unwrap_or_default().is_empty(),
                    "case {row}: every published row carries the quota's description, got \
                     {quota}"
                );
                let name = quota["QuotaName"].as_str().unwrap_or_default();
                assert!(
                    names.insert(name),
                    "case {row}: a quota name identifies one quota of a service, got a second \
                     row named {name:?}"
                );
                assert_eq!(
                    quota["QuotaArn"],
                    json!(format!(
                        "arn:aws:servicequotas:{region}::{}/{code}",
                        catalog.service_code
                    )),
                    "case {row}: a default quota's ARN names the Region and no account, got \
                     {quota}"
                );
            }

            let codes: BTreeSet<&str> = quotas
                .iter()
                .filter_map(|quota| quota["QuotaCode"].as_str())
                .collect();
            assert_eq!(
                codes.len(),
                quotas.len(),
                "case {case}: every published quota has a key of its own, got {codes:?}"
            );

            assert_eq!(
                inventory.get(catalog.service_code).map(String::as_str),
                Some(catalog.service_name),
                "case {case}: ListServices must report the same display name the quotas carry, \
                 got {:?}",
                inventory.get(catalog.service_code)
            );
        }
    }
}

/// A code this catalog does not publish resolves NOWHERE, while the service itself still
/// resolves.
#[tokio::test]
async fn a_code_this_catalog_does_not_publish_resolves_nowhere() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    for catalog in CATALOGS {
        for withdrawn in catalog.withdrawn {
            let case = format!("{}/{withdrawn}", catalog.service_code);

            for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
                let listed = listed_codes(&caller, operation, catalog.service_code).await;
                assert!(
                    !listed.contains(*withdrawn),
                    "case {case}: {operation} must not publish a code AWS publishes no quota \
                     of this service under, got {listed:?}"
                );
            }

            for operation in [
                "GetAWSDefaultServiceQuota",
                "GetServiceQuota",
                "RequestServiceQuotaIncrease",
            ] {
                let mut body =
                    json!({"ServiceCode": catalog.service_code, "QuotaCode": withdrawn});
                if operation == "RequestServiceQuotaIncrease" {
                    body["DesiredValue"] = json!(1_000_000.0);
                }
                let (status, error) = caller.refused(operation, body).await;
                assert_eq!(
                    (status, error.as_str()),
                    (400, "NoSuchResourceException"),
                    "case {case}: {operation} must refuse a code this service publishes \
                     nothing under"
                );
            }
        }

        // The SERVICE still resolves: it is the unpublished code that does not.
        let response = caller
            .ok(
                "ListAWSDefaultServiceQuotas",
                json!({"ServiceCode": catalog.service_code}),
            )
            .await;
        assert!(
            response["Quotas"].is_array(),
            "case {}: the service code must still resolve its listing, got {response}",
            catalog.service_code
        );
    }
}

/// A published row reports the facts AWS publishes for that quota, identically through the
/// listing and the single read, and the applied plane resolves the same key.
#[tokio::test]
async fn a_published_row_reports_the_facts_aws_publishes() {
    let engine = emulator();
    let region = "ap-south-1";
    let caller = client(&engine, region);

    for row in ROWS {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let quota = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();

        assert_eq!(
            quota["QuotaName"],
            json!(row.quota_name),
            "case {case}: expected the published quota name, got {quota}"
        );
        assert_eq!(
            quota["Value"],
            json!(row.value),
            "case {case}: expected the published default value, got {quota}"
        );
        assert_eq!(
            quota["Unit"],
            json!(row.unit),
            "case {case}: expected the published unit, got {quota}"
        );
        assert_eq!(
            quota["Adjustable"],
            json!(row.adjustable),
            "case {case}: expected the published adjustability, got {quota}"
        );
        assert_eq!(
            quota["GlobalQuota"],
            json!(false),
            "case {case}: AWS counts these quotas per Region, got {quota}"
        );
        assert_eq!(
            quota["Description"],
            json!(row.description),
            "case {case}: a description is the service's own published text, got {quota}"
        );
        assert_eq!(
            quota["QuotaArn"],
            json!(format!(
                "arn:aws:servicequotas:{region}::{}/{}",
                row.service_code, row.quota_code
            )),
            "case {case}: an AWS default quota's ARN names the Region and no account, got \
             {quota}"
        );
        match row.period {
            Some((value, unit)) => assert_eq!(
                quota["Period"],
                json!({"PeriodValue": value, "PeriodUnit": unit}),
                "case {case}: expected the window AWS publishes the quota over, got {quota}"
            ),
            None => assert!(
                quota.get("Period").is_none_or(Value::is_null),
                "case {case}: this quota is published over no window, got {quota}"
            ),
        }
        let metric = metric_json(&row.metric);
        if metric.is_null() {
            assert!(
                quota.get("UsageMetric").is_none_or(Value::is_null),
                "case {case}: this quota publishes no usage metric, got {quota}"
            );
        } else {
            assert_eq!(
                quota["UsageMetric"], metric,
                "case {case}: expected the usage metric the service publishes, got {quota}"
            );
        }
        // A quota adjustable for an individual resource describes that plane on every
        // record of it, with the RESOURCE scope the plane it is counted on gives it; a
        // quota managed for the account describes no plane at all.
        match row.resource_scope_type {
            Some(scope_type) => assert_eq!(
                quota["QuotaContext"],
                json!({
                    "ContextScope": "RESOURCE",
                    "ContextScopeType": scope_type,
                    "AdjustableAtLevel": "ALL",
                }),
                "case {case}: expected the per-resource plane AWS publishes, got {quota}"
            ),
            None => assert!(
                quota.get("QuotaContext").is_none_or(Value::is_null),
                "case {case}: a quota managed for the account carries no context, got {quota}"
            ),
        }

        // The listing carries exactly the record the single read returned.
        let listed = defaults(&caller, row.service_code).await;
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );

        // The applied plane is keyed on the same code, so a client that read a code out of
        // the default catalog resolves its value in force too.
        let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
        assert_eq!(
            (
                applied["QuotaCode"].clone(),
                applied["QuotaName"].clone(),
                applied["Value"].clone()
            ),
            (
                json!(row.quota_code),
                json!(row.quota_name),
                json!(row.value)
            ),
            "case {case}: the applied read resolves the same quota at its default value, got \
             {applied}"
        );
    }
}
