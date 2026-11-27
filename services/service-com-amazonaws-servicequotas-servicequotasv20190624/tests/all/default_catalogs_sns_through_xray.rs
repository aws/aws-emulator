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
//!   code AWS mints for that quota, with the service's own `ServiceCode`/`ServiceName`,
//!   the quota's own description where a source available here states it, and one key per
//!   quota;
//! * a code this catalog does not publish resolves NOWHERE — neither a code built out of a
//!   quota's own name nor a minted code the service publishes no quota under — so a client
//!   enumerating the catalog is never handed a key no other API accepts;
//! * a published row reports the `QuotaName`, `Value`, `Unit`, `Adjustable`, `GlobalQuota`,
//!   `Period`, `UsageMetric` and `QuotaContext` AWS publishes for the quota, with a
//!   `QuotaArn` naming the Region it was read at and no account, identically through the
//!   listing and through `GetAWSDefaultServiceQuota`, and the same key resolves in the
//!   APPLIED plane;
//! * and, read from the other end, the quota published under a `QuotaCode` is the one the
//!   recorded listing BINDS to that code, while a quota whose own code no source available
//!   here states is published under no code at all rather than under a neighbouring row's.
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
        // Only the descriptive stand-ins: every one of this service's 48 published codes
        // answers a row, because a code AWS publishes a quota under that resolved nowhere
        // would make a quota that exists in AWS look nonexistent.
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
        service_name: "AWS IAM Identity Center (successor to AWS Single Sign-On)",
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
        quota_count: 31,
        withdrawn: &["DatabasesPerAccount"],
    },
    Catalog {
        service_code: "vpc",
        service_name: "Amazon Virtual Private Cloud (Amazon VPC)",
        quota_count: 32,
        // The two stand-ins, beside a minted code AWS publishes no VPC quota under: the
        // network-interface plane of the Region is published under `L-DF5E4CA3`.
        withdrawn: &[
            "PrefixListsPerRegion",
            "SecurityGroupsPerVPC",
            "L-045E6E9C",
        ],
    },
    Catalog {
        service_code: "vpc-lattice",
        service_name: "VPC Lattice",
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

/// The `(ServiceCode, QuotaCode)` pairs whose published `Description` no source available
/// here states, and which therefore carry none.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of each `ServiceCode`
/// states the quota these codes carry but not the sentence the service describes it with.
/// `Description` is an optional member, so such a row publishes none; wording one here
/// would be this emulator's sentence rather than the service's.
const DESCRIPTION_WITHHELD: &[(&str, &str)] = &[
    ("sqs", "L-A7816957"),
    ("sqs", "L-C491D5A4"),
    ("sqs", "L-F115E65A"),
    ("sso", "L-0299121C"),
    ("sso", "L-7E1D4E33"),
    ("sso", "L-89954265"),
    ("sso", "L-B44C7A29"),
    ("storagegateway", "L-CEFD39A0"),
    ("timestream", "L-1222731D"),
    ("vpc", "L-42B9C2CA"),
    ("vpc", "L-5F53652F"),
    ("vpc", "L-A272D574"),
    ("vpc", "L-CA6CC422"),
    ("vpc", "L-DFA99DE7"),
    ("vpc-lattice", "L-5FF8F9B9"),
    ("vpc-lattice", "L-6095700C"),
    ("vpc-lattice", "L-73D0F278"),
    ("vpc-lattice", "L-9CAD07FB"),
    ("vpc-lattice", "L-CA6A1CC5"),
    ("wafv2", "L-5B2F9B11"),
    ("wafv2", "L-71C2E81B"),
    ("wafv2", "L-B32E8672"),
    ("wafv2", "L-BF0029B0"),
    ("wafv2", "L-D6DA96AE"),
    ("wafv2", "L-D95DED7E"),
    ("workspaces", "L-40A36AF9"),
    ("workspaces", "L-63F14199"),
    ("workspaces", "L-8C70C791"),
    ("workspaces", "L-BD784BE1"),
    ("workspaces", "L-F64A92CF"),
    ("xray", "L-428FB8E0"),
    ("xray", "L-5DB17969"),
    ("xray", "L-8C0C998A"),
    ("xray", "L-998BFF16"),
    ("xray", "L-DB51D338"),
];

/// The `UsageMetric` a pinned row publishes, where it publishes one.
///
/// `pub(crate)` so the sibling suite that pins the recorded bindings of these same ten
/// services describes a metric through this one shape rather than a second copy of it.
pub(crate) enum Metric {
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
    /// The `AWS/Usage` resource usage of a quota published as a count over a WINDOW — the
    /// usage SUMMED over the window rather than the largest count seen in it — under the
    /// `Resource` SERIES that usage is published on.
    ResourceRate {
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
    /// The `Description` the service publishes for the quota, or `""` for a quota whose
    /// published sentence no source available here states: `Description` is an optional
    /// member (`@length 0..350`, not required), and a row whose text is unknown carries
    /// none rather than one this emulator words itself.
    description: &'static str,
    /// `(PeriodValue, PeriodUnit)` — the window the quota is published over, for a row
    /// that is published over one.
    period: Option<(i32, &'static str)>,
    metric: Metric,
    /// `(ContextScopeType, AdjustableAtLevel)` — the resource type the quota's
    /// per-resource plane applies to and the level AWS publishes an increase as askable
    /// at, for a row published as a quota OF A RESOURCE.
    resource_plane: Option<(&'static str, &'static str)>,
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
        quota_name: "CreateTopic Transactions per Second",
        value: 900.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of times the CreateTopic action can be called per \
                      second.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "SNS",
            api: "CreateTopic",
        },
        resource_plane: None,
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
        resource_plane: None,
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
        resource_plane: None,
    },
    Row {
        service_code: "sqs",
        quota_code: "L-E8A9C91E",
        quota_name: "Queue Name Length",
        value: 80.0,
        unit: "None",
        adjustable: false,
        description: "The number of characters in a queue name.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Row {
        service_code: "sso",
        quota_code: "L-B44C7A29",
        quota_name: "Number of permission sets allowed in IAM Identity Center",
        value: 3500.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Row {
        service_code: "sso",
        quota_code: "L-89954265",
        quota_name: "Number of permission sets allowed per AWS account",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Row {
        service_code: "storagegateway",
        quota_code: "L-14F83003",
        quota_name: "Cached volume gateway Cache Maximum in TiB",
        value: 64.0,
        unit: "None",
        adjustable: false,
        description: "Maximum cache size for Cached Volume Gateway",
        period: None,
        metric: Metric::None,
        resource_plane: None,
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
        resource_plane: None,
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
        resource_plane: None,
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
        resource_plane: None,
    },
    // The rule plane of ONE security group: metered by the count CloudWatch publishes for
    // it, applied to each security group, with an increase askable at the account level.
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
        resource_plane: Some(("AWS::EC2::SecurityGroup", "ACCOUNT")),
    },
    Row {
        service_code: "vpc",
        quota_code: "L-DF5E4CA3",
        quota_name: "Network interfaces per Region",
        value: 5000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of network interfaces per Availability Zone in a \
                      Region.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Row {
        service_code: "vpc-lattice",
        quota_code: "L-87FCA9B2",
        quota_name: "Auth policy size",
        value: 10.0,
        unit: "Kilobytes",
        adjustable: false,
        description: "The maximum size of a JSON file in an Auth policy.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
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
        resource_plane: None,
    },
    Row {
        service_code: "wafv2",
        quota_code: "L-5B2F9B11",
        quota_name: "Maximum number of AWS AppSync GraphQL API associations per web ACL in WAF \
                     for regional",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
        resource_plane: Some(("AWS::WAFv2::WebACL", "ALL")),
    },
    Row {
        service_code: "wafv2",
        quota_code: "L-D6DA96AE",
        quota_name: "Maximum IP sets per account in WAF for regional",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "AWS WAF",
            resource: "IpSetsPerAccountRegional",
        },
        resource_plane: None,
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
        resource_plane: None,
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
        resource_plane: None,
    },
    Row {
        service_code: "xray",
        quota_code: "L-8C0C998A",
        quota_name: "Custom sampling rules per region",
        value: 25.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Row {
        service_code: "xray",
        quota_code: "L-D781C0FD",
        quota_name: "Segment document size",
        value: 64.0,
        unit: "Kilobytes",
        adjustable: false,
        description: "The maximum size for segment documents.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
];

/// The `UsageMetric` a row's [`Metric`] publishes, as the wire reports it.
///
/// `pub(crate)` for the same reason [`Metric`] is.
pub(crate) fn metric_json(metric: &Metric) -> Value {
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
        Metric::ResourceRate {
            usage_service,
            resource,
        } => json!({
            "MetricNamespace": "AWS/Usage",
            "MetricName": "ResourceCount",
            "MetricStatisticRecommendation": "Sum",
            "MetricDimensions": {
                "Service": usage_service,
                "Class": "None",
                "Resource": resource,
                "Type": "Resource",
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
                // Every row carries the quota's own published sentence, except the rows
                // whose sentence no source available here states: those carry NO
                // `Description` (the member is optional, `@length 0..350`) rather than one
                // this emulator words itself. The exceptions are named, so a row that
                // quietly loses its description still fails.
                if DESCRIPTION_WITHHELD.contains(&(catalog.service_code, code)) {
                    assert!(
                        quota.get("Description").is_none_or(Value::is_null),
                        "case {row}: a row whose published description no source states \
                         carries none, got {quota}"
                    );
                } else {
                    assert!(
                        !quota["Description"].as_str().unwrap_or_default().is_empty(),
                        "case {row}: this row carries the quota's published description, got \
                         {quota}"
                    );
                }
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
        if row.description.is_empty() {
            assert!(
                quota.get("Description").is_none_or(Value::is_null),
                "case {case}: a quota whose published description no source states carries \
                 none, got {quota}"
            );
        } else {
            assert_eq!(
                quota["Description"],
                json!(row.description),
                "case {case}: a description is the service's own published text, got {quota}"
            );
        }
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
        // A quota published as a quota OF A RESOURCE describes that plane on every record
        // of it, with the RESOURCE scope the plane it is counted on gives it and the level
        // AWS publishes an increase as askable at; a quota managed for the account
        // describes no plane at all.
        match row.resource_plane {
            Some((scope_type, adjustable_at_level)) => assert_eq!(
                quota["QuotaContext"],
                json!({
                    "ContextScope": "RESOURCE",
                    "ContextScopeType": scope_type,
                    "AdjustableAtLevel": adjustable_at_level,
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

/// One `(ServiceCode, QuotaCode)` pair whose binding the recorded listing states, and the
/// quota published under it.
///
/// Where [`ROWS`] pins the facts of a row, this pins the BINDING: which quota a code
/// carries. The two are the same rule read from its two ends, so this table names the
/// codes whose quota the recording states, and the two tables below name the codes and the
/// quotas that are therefore published under nothing.
struct Binding {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    period: Option<(i32, &'static str)>,
    metric: Metric,
    resource_plane: Option<(&'static str, &'static str)>,
}

/// The bindings this suite pins, every one of the ten `ServiceCode`s represented.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of each `ServiceCode`,
/// which is the reference for which quota a code is published under. Within the ten
/// services the rows differ in every member class a binding can get wrong: a counted and
/// a measured quota (`None`, `Bytes`, `Kilobytes`, `Gigabytes`), adjustable and fixed
/// rows, no window beside a one-second rate window and the five-minute window a resource
/// count is published over, unmetered rows beside both `AWS/Usage` metric shapes under
/// three different `Service` dimensions, and a resource plane at each `AdjustableAtLevel`
/// the model declares for one.
const BINDINGS: &[Binding] = &[
    Binding {
        service_code: "sns",
        quota_code: "L-4738DDCE",
        quota_name: "CreatePlatformApplication Transactions per Second",
        value: 900.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "SNS",
            api: "CreatePlatformApplication",
        },
        resource_plane: None,
    },
    Binding {
        service_code: "sns",
        quota_code: "L-FF07E5EA",
        quota_name: "ConfirmSubscription Transactions per Second",
        value: 900.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "SNS",
            api: "ConfirmSubscription",
        },
        resource_plane: None,
    },
    // The same class, published as NOT adjustable: the flag is a fact of the code, not of
    // the class.
    Binding {
        service_code: "sns",
        quota_code: "L-390AADBE",
        quota_name: "ListSubscriptionsByTopic Transactions per Second",
        value: 30.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "SNS",
            api: "ListSubscriptionsByTopic",
        },
        resource_plane: None,
    },
    Binding {
        service_code: "sqs",
        quota_code: "L-98134C75",
        quota_name: "Attributes per Message",
        value: 10.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "sqs",
        quota_code: "L-81970B52",
        quota_name: "Message Size in S3 Bucket",
        value: 2.0,
        unit: "Gigabytes",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "sqs",
        quota_code: "L-BBEFA6CF",
        quota_name: "Queue Policy Size",
        value: 8192.0,
        unit: "Bytes",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "sqs",
        quota_code: "L-BF2A6161",
        quota_name: "UTF-8 Queue Tag Value Length",
        value: 256.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "sso",
        quota_code: "L-2FDFC908",
        quota_name: "Total number of groups that can be assigned to a permission set per AWS \
                     account, or to an application",
        value: 100.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "sso",
        quota_code: "L-D519A297",
        quota_name: "Total number of regions for IAM Identity Center",
        value: 6.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    // A service that names the unit IN the quota's own name and publishes `Unit` `None`
    // for it, with three minima of 150 GiB that differ only in the gateway type they
    // apply to — which is exactly what a positional catalog cannot tell apart.
    Binding {
        service_code: "storagegateway",
        quota_code: "L-311F8856",
        quota_name: "Max size of a virtual tape in TiB",
        value: 15.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "storagegateway",
        quota_code: "L-6D83F84B",
        quota_name: "Cached volume gateway Upload Buffer Minimum in GiB",
        value: 150.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "storagegateway",
        quota_code: "L-87640A7E",
        quota_name: "Tape gateway Upload Buffer Minimum in GiB",
        value: 150.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "storagegateway",
        quota_code: "L-F3EDDF8C",
        quota_name: "Tape gateway Cache Minimum in GiB",
        value: 150.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "timestream",
        quota_code: "L-798CB19D",
        quota_name: "CreateScheduledQuery request rate",
        value: 1.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "Timestream",
            api: "CreateScheduledQuery",
        },
        resource_plane: None,
    },
    Binding {
        service_code: "timestream",
        quota_code: "L-C49D398A",
        quota_name: "CreateBatchLoadTask request rate",
        value: 1.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "Timestream",
            api: "CreateBatchLoadTask",
        },
        resource_plane: None,
    },
    Binding {
        service_code: "vpc",
        quota_code: "L-5F53652F",
        quota_name: "Elastic IP address quota per NAT gateway",
        value: 2.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "vpc",
        quota_code: "L-42B9C2CA",
        quota_name: "VPC Block Public Access exclusions per account per Region",
        value: 50.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "vpc",
        quota_code: "L-CA6CC422",
        quota_name: "VPC endpoints of type 'resource' per VPC",
        value: 500.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "vpc",
        quota_code: "L-8312C5BB",
        quota_name: "VPC peering connection request expiry hours",
        value: 168.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    // The route plane of ONE route table: the value is applied to each route table, the
    // increase is asked for at the ACCOUNT level, and the plane is metered by the resource
    // count CloudWatch publishes for it.
    Binding {
        service_code: "vpc",
        quota_code: "L-93826ACB",
        quota_name: "Routes per route table",
        value: 50.0,
        unit: "None",
        adjustable: true,
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "EC2",
            resource: "RoutesPerRouteTable",
        },
        resource_plane: Some(("AWS::EC2::RouteTable", "ACCOUNT")),
    },
    Binding {
        service_code: "vpc-lattice",
        quota_code: "L-87FCA9B2",
        quota_name: "Auth policy size",
        value: 10.0,
        unit: "Kilobytes",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "vpc-lattice",
        quota_code: "L-5FF8F9B9",
        quota_name: "Resource configurations per AWS Region",
        value: 2000.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    // The same resource plane as the VPC row above, at the OTHER level an increase can be
    // asked for: here both the account and the individual web ACL.
    Binding {
        service_code: "wafv2",
        quota_code: "L-B32E8672",
        quota_name: "Maximum number of Amazon API Gateway REST API associations per web ACL \
                     in WAF for regional",
        value: 100.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: Metric::None,
        resource_plane: Some(("AWS::WAFv2::WebACL", "ALL")),
    },
    // A RESOURCE count where the catalog used to publish an API CALL count, under the
    // service dimension `AWS/Usage` publishes WAF's resource counts under.
    Binding {
        service_code: "wafv2",
        quota_code: "L-D6DA96AE",
        quota_name: "Maximum IP sets per account in WAF for regional",
        value: 100.0,
        unit: "None",
        adjustable: true,
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "AWS WAF",
            resource: "IpSetsPerAccountRegional",
        },
        resource_plane: None,
    },
    Binding {
        service_code: "workspaces",
        quota_code: "L-40A36AF9",
        quota_name: "Graphics.g6.4xlarge WorkSpaces",
        value: 0.0,
        unit: "None",
        adjustable: true,
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "WorkSpaces",
            resource: "GraphicsG64xlargeWorkSpace",
        },
        resource_plane: None,
    },
    Binding {
        service_code: "workspaces",
        quota_code: "L-BD784BE1",
        quota_name: "Graphics.g6.16xlarge WorkSpaces",
        value: 0.0,
        unit: "None",
        adjustable: true,
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "WorkSpaces",
            resource: "GraphicsG616xlargeWorkSpace",
        },
        resource_plane: None,
    },
    Binding {
        service_code: "xray",
        quota_code: "L-8C0C998A",
        quota_name: "Custom sampling rules per region",
        value: 25.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "xray",
        quota_code: "L-DB51D338",
        quota_name: "Tags per custom sampling rule",
        value: 50.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "xray",
        quota_code: "L-428FB8E0",
        quota_name: "Trace document size (dynamic upper limit)",
        value: 500.0,
        unit: "Kilobytes",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Binding {
        service_code: "xray",
        quota_code: "L-C6B6F05D",
        quota_name: "Indexed annotations per trace",
        value: 50.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
];

/// The quota NAMES no row of these catalogs carries: the rows the codes in [`BINDINGS`]
/// used to carry, whose own code no source available here states, and a name this
/// emulator used to word a quota with where the service words it otherwise.
///
/// Read from the quota's end rather than the code's: a name the service does not publish
/// is served by no row, and a quota whose code is unknown is never republished under a
/// neighbouring row's code once a source binds that code to its own quota.
const UNPUBLISHED: &[(&str, &str)] = &[
    // The notification catalog's own publish rate under the name this emulator used to
    // word it with: the service publishes that rate as a count of MESSAGES per second, so
    // no row carries a `Publish` call rate.
    ("sns", "Publish Transactions per Second"),
    ("sqs", "Message Size"),
    ("sqs", "Message Retention Period"),
    ("sqs", "Transactions per Second per API Action"),
    ("sqs", "FIFO Queue Transactions per Second"),
    ("sqs", "In Flight Messages per Standard Queue"),
    ("sqs", "In Flight Messages per FIFO Queue"),
    ("sqs", "Tags per Queue"),
    ("sqs", "Batch Entries per Batch Request"),
    ("sqs", "Message Deduplication ID Length"),
    ("sso", "AWS managed policies per permission set"),
    ("sso", "Trusted token issuers per instance"),
    ("sso", "Inline policy size per permission set"),
    ("sso", "Applications per instance"),
    ("sso", "Permission sets"),
    ("sso", "Groups per instance"),
    ("storagegateway", "Maximum size of a stored volume"),
    ("storagegateway", "Virtual tapes per gateway"),
    ("storagegateway", "Cache disks per gateway"),
    ("storagegateway", "Maximum size of a virtual tape"),
    ("storagegateway", "Volumes per snapshot schedule"),
    ("storagegateway", "Total cached volume storage per gateway"),
    ("storagegateway", "Automatic tape creation policies per gateway"),
    ("storagegateway", "Clients per NFS file share"),
    ("storagegateway", "Bandwidth rate limit schedule intervals per gateway"),
    ("storagegateway", "Maximum size of a file"),
    ("timestream", "Query timeout"),
    ("timestream", "Magnetic store retention"),
    ("vpc", "Entries per customer-managed prefix list"),
    ("vpc", "Customer-managed prefix lists per Region"),
    ("vpc", "Peered Network Address Usage"),
    ("vpc", "References to a prefix list per resource type"),
    ("vpc", "Traffic mirror filters per Region"),
    ("vpc-lattice", "Service networks per account"),
    ("vpc-lattice", "Header match conditions per rule"),
    ("vpc-lattice", "Auth policies per resource"),
    ("vpc-lattice", "Listeners per service"),
    ("vpc-lattice", "Target groups per account"),
    ("vpc-lattice", "Rules per listener"),
    ("vpc-lattice", "Service network associations per service"),
    ("vpc-lattice", "Services per service network"),
    ("wafv2", "Web ACL capacity units (WCU) per rule group"),
    ("wafv2", "Rate of GetSampledRequests requests"),
    ("wafv2", "IP sets per Region"),
    ("wafv2", "Web ACL capacity units (WCU) per web ACL"),
    ("wafv2", "Versions retained per managed rule group"),
    ("workspaces", "Connection alias associations per alias"),
    ("workspaces", "Rate of DescribeAccount requests"),
    ("workspaces", "Rate of CreateTags requests"),
    ("workspaces", "Certificate-based authentication certificates per directory"),
    ("workspaces", "Rate of StartWorkspaces requests"),
    ("xray", "Sampling rules"),
    ("xray", "Rate of BatchGetTraces requests"),
    ("xray", "Rate of PutTraceSegments requests"),
    ("xray", "Rate of GetTraceSummaries requests"),
    ("xray", "Rate of PutTelemetryRecords requests"),
    ("xray", "Trace retention"),
    ("xray", "Groups"),
    ("xray", "Rate of GetServiceGraph requests"),
];

/// Every member of the row published under a code is the one the service publishes for
/// THAT code, read in two Regions, through the single read and through the listing alike.
#[tokio::test]
async fn a_code_publishes_the_quota_the_service_binds_to_it() {
    let engine = emulator();
    // Two Regions, because the catalog is published per Region and a binding is a fact of
    // the catalog rather than of one Region's view of it.
    for region in ["us-east-2", "eu-west-3"] {
        let caller = client(&engine, region);

        for row in BINDINGS {
            let case = format!("{region} {}/{}", row.service_code, row.quota_code);
            let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
            let quota = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();

            assert_eq!(
                (
                    quota["QuotaName"].clone(),
                    quota["Value"].clone(),
                    quota["Unit"].clone(),
                    quota["Adjustable"].clone(),
                ),
                (
                    json!(row.quota_name),
                    json!(row.value),
                    json!(row.unit),
                    json!(row.adjustable),
                ),
                "case {case}: expected the name, value, unit and adjustability the service \
                 publishes under THIS code, got {quota}"
            );

            match row.period {
                Some((value, unit)) => assert_eq!(
                    quota["Period"],
                    json!({"PeriodValue": value, "PeriodUnit": unit}),
                    "case {case}: expected the window this quota is published over, got {quota}"
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
                    "case {case}: expected the usage metric of THIS quota's plane, got {quota}"
                );
            }

            match row.resource_plane {
                Some((scope_type, adjustable_at_level)) => assert_eq!(
                    quota["QuotaContext"],
                    json!({
                        "ContextScope": "RESOURCE",
                        "ContextScopeType": scope_type,
                        "AdjustableAtLevel": adjustable_at_level,
                    }),
                    "case {case}: expected the resource plane this quota is counted on, got \
                     {quota}"
                ),
                None => assert!(
                    quota.get("QuotaContext").is_none_or(Value::is_null),
                    "case {case}: a quota managed for the account carries no context, got \
                     {quota}"
                ),
            }

            // The listing carries exactly the record the single read returned, and the
            // applied plane resolves the same key at the same default.
            let listed = defaults(&caller, row.service_code).await;
            assert!(
                listed.contains(&quota),
                "case {case}: the listing must carry the record the single read returned, got \
                 {listed:?}"
            );
            let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
            assert_eq!(
                (
                    applied["QuotaName"].clone(),
                    applied["Value"].clone(),
                    applied["Unit"].clone()
                ),
                (json!(row.quota_name), json!(row.value), json!(row.unit)),
                "case {case}: the applied read resolves the same quota at its default, got \
                 {applied}"
            );
        }
    }
}

/// A quota whose own code no source states is published under no code at all — never
/// under the code of a neighbouring row.
#[tokio::test]
async fn a_quota_whose_code_no_source_states_is_published_under_no_code() {
    let engine = emulator();
    let caller = client(&engine, "us-west-2");

    for (service_code, name) in UNPUBLISHED {
        let listed = defaults(&caller, service_code).await;
        let carried: Vec<&str> = listed
            .iter()
            .filter(|quota| quota["QuotaName"] == json!(name))
            .filter_map(|quota| quota["QuotaCode"].as_str())
            .collect();
        assert!(
            carried.is_empty(),
            "case {service_code}/{name:?}: a quota whose own code no source states must be \
             published under none, got it under {carried:?}"
        );
    }
}
