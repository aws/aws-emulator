// SPDX-License-Identifier: CC0-1.0

//! WHICH quota each `QuotaCode` of the notification, queueing, sign-on, gateway,
//! time-series, networking, firewall, desktop and tracing catalogs publishes.
//!
//! `QuotaCode` is the only stable key Service Quotas exposes, so the row served under one
//! is a claim about the quota the service publishes UNDER THAT CODE. The rules driven here
//! over a TABLE of the codes whose quota a source available here states:
//!
//! * the row published under a code carries that quota's own `QuotaName`, `Value`, `Unit`,
//!   `Adjustable`, `GlobalQuota`, `Period`, `UsageMetric`, `QuotaContext` and
//!   `Description` — the text as the service words it, never a paraphrase and never a
//!   neighbouring quota's — identically through the listing, through
//!   `GetAWSDefaultServiceQuota` and through the applied read;
//! * a member no source available here states is not published rather than invented: a
//!   row whose sentence is unknown carries no `Description`, and a metered plane whose
//!   `Resource` series is unknown publishes the dimensions its class states and no series;
//! * and `Adjustable` is a fact of the CODE rather than of the quota's class: the row
//!   published under a code the service publishes as fixed refuses
//!   `RequestServiceQuotaIncrease`, while an adjustable one takes it — so a misbound row
//!   sends a client down a path the service does not serve.
//!
//! The table spans what the rules generalise over: ten `ServiceCode`s; counted and
//! measured quotas (`None`, `Bytes`, `Kilobytes`, `Gigabytes`, `Seconds`); adjustable and
//! fixed rows; no window, the one-second window a request rate is published over and the
//! five-minute window a resource count is published over; unmetered rows beside both
//! `AWS/Usage` metric shapes and a resource count with no series; a row published as a
//! quota OF A RESOURCE; and a limit published at zero beside one published in the hundreds
//! of thousands.
//!
//! The harness (`client`, `emulator`, `default_quotas`, `service_inventory`) is the one
//! `quota_workflow.rs` owns, and a metric is described through the single `Metric` shape
//! `default_catalogs_sns_through_xray.rs` owns.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::default_catalogs_sns_through_xray::{metric_json, Metric};
use crate::quota_workflow::{
    client, default_quotas as defaults, emulator, service_inventory as inventory, Client,
};

/// One binding: the code, and every member of the quota the service publishes under it.
struct Bound {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// The sentence the service publishes for the quota, or `""` where no source
    /// available here states it: `Description` is optional (`@length 0..350`), so such a
    /// row publishes none rather than one this emulator words itself.
    description: &'static str,
    /// `(PeriodValue, PeriodUnit)`, for a row published over a window.
    period: Option<(i32, &'static str)>,
    metric: Metric,
    /// `(ContextScopeType, AdjustableAtLevel)`, for a row published as a quota OF A
    /// RESOURCE.
    resource_plane: Option<(&'static str, &'static str)>,
}

/// The bindings this suite pins.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of each `ServiceCode`,
/// which is the reference for which quota a code publishes and for the wording of each
/// row's `Description`; the catalog these are seeded from is
/// `src/state/quota_rows/<service>.rs`, which names the source of every member beside it.
const BOUND: &[Bound] = &[
    // The notification service's per-API rate class: the window, the call count and the
    // sentence are the class's, the ACTION and the rate are the code's.
    Bound {
        service_code: "sns",
        quota_code: "L-B6107771",
        quota_name: "SetSubscriptionAttributes Transactions per Second",
        value: 900.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of times the SetSubscriptionAttributes action can be \
                      called per second.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "SNS",
            api: "SetSubscriptionAttributes",
        },
        resource_plane: None,
    },
    Bound {
        service_code: "sns",
        quota_code: "L-876E1222",
        quota_name: "GetSubscriptionAttributes Transactions per Second",
        value: 900.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of times the GetSubscriptionAttributes action can be \
                      called per second.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "SNS",
            api: "GetSubscriptionAttributes",
        },
        resource_plane: None,
    },
    // A COUNTED plane published under a code the catalog used to serve a rate under: a
    // resource count over the five-minute window those counts are published on, under the
    // series the recorded listing names for it.
    Bound {
        service_code: "sns",
        quota_code: "L-1A43D3DB",
        quota_name: "Pending Subscriptions per Account",
        value: 5000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of pending subscriptions per AWS account, across all \
                      regions.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "SNS",
            resource: "ApproximateNumberOfPendingSubscriptions",
        },
        resource_plane: None,
    },
    // The rate published under the code the recorded listing binds the CREATE-TOPIC rate
    // to, beside the one SMS-attribute rate that listing publishes as ONE call per second
    // and as NOT adjustable: the action, the limit and the adjustability are the code's.
    Bound {
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
    Bound {
        service_code: "sns",
        quota_code: "L-154ECCA7",
        quota_name: "SetSMSAttributes Transactions per Second",
        value: 1.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of times the SetSMSAttributes action can be called per \
                      second.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "SNS",
            api: "SetSMSAttributes",
        },
        resource_plane: None,
    },
    // The one rate of the notification catalog published as a count of MESSAGES over the
    // second rather than as a count of one action's calls: the recorded listing names and
    // describes it that way and meters it with a resource-usage series summed over the
    // window — the account's published-message count.
    Bound {
        service_code: "sns",
        quota_code: "L-F8E2BA85",
        quota_name: "Messages Published per Second",
        value: 9000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of messages published per second.",
        period: Some((1, "SECOND")),
        metric: Metric::ResourceRate {
            usage_service: "SNS",
            resource: "NumberOfMessagesPublishedPerAccount",
        },
        resource_plane: None,
    },
    Bound {
        service_code: "sqs",
        quota_code: "L-9F628B95",
        quota_name: "Statements per Queue Policy",
        value: 20.0,
        unit: "None",
        adjustable: false,
        description: "The number of statements in a queue policy.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "sqs",
        quota_code: "L-4FF3DAAF",
        quota_name: "Batched Message ID Length",
        value: 80.0,
        unit: "None",
        adjustable: false,
        description: "The length of a batched message ID.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    // A quota published at ZERO, measured in seconds.
    Bound {
        service_code: "sqs",
        quota_code: "L-B2A3B9D5",
        quota_name: "Message Invisibility Period",
        value: 0.0,
        unit: "Seconds",
        adjustable: false,
        description: "The length of time, in seconds, for which a message consumed from a queue \
                      remains invisible to other consumers.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "sqs",
        quota_code: "L-2DA3E3B2",
        quota_name: "Message Retention Time",
        value: 345_600.0,
        unit: "Seconds",
        adjustable: false,
        description: "The length of time, in seconds, for which Amazon SQS retains a message if \
                      it isn't deleted. The maximum is 14 days (1,209,600 seconds).",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "sqs",
        quota_code: "L-81970B52",
        quota_name: "Message Size in S3 Bucket",
        value: 2.0,
        unit: "Gigabytes",
        adjustable: false,
        description: "The size of a message, in gigabytes, in an Amazon S3 bucket.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "sqs",
        quota_code: "L-BF2A6161",
        quota_name: "UTF-8 Queue Tag Value Length",
        value: 256.0,
        unit: "None",
        adjustable: false,
        description: "The length of a UTF-8 queue tag value.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    // The sign-on service publishes a FILE SIZE under the code the catalog used to serve
    // an adjustable permission-set count under, and that count under the code it used to
    // serve a fixed policy size under: both the unit and the adjustability are the code's.
    Bound {
        service_code: "sso",
        quota_code: "L-C4FCA052",
        quota_name: "File size of service provider SAML 2.0 certificates (in PEM format)",
        value: 2.0,
        unit: "Kilobytes",
        adjustable: false,
        description: "The maximum file size (in KB) of service provider SAML 2.0 certificates \
                      (in PEM format).",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
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
    Bound {
        service_code: "sso",
        quota_code: "L-2FDFC908",
        quota_name: "Total number of groups that can be assigned to a permission set per AWS \
                     account, or to an application",
        value: 100.0,
        unit: "None",
        adjustable: false,
        description: "Total number of groups that can be assigned to a permission set per AWS \
                      account, or to an application.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "sso",
        quota_code: "L-D519A297",
        quota_name: "Total number of regions for IAM Identity Center",
        value: 6.0,
        unit: "None",
        adjustable: true,
        description: "The total number of regions that can be enabled for IAM Identity Center, \
                      including the primary region.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    // The gateway service names the unit IN the quota's own name for two of these and in
    // `Unit` for the third, and words every one of them in its own terse form.
    Bound {
        service_code: "storagegateway",
        quota_code: "L-F5915598",
        quota_name: "File gateway Cache Maximum in TiB",
        value: 64.0,
        unit: "None",
        adjustable: false,
        description: "Maximum cache size for File Gateway",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "storagegateway",
        quota_code: "L-4951D254",
        quota_name: "Total size of tapes in a virtual tape library in PiB",
        value: 1.0,
        unit: "None",
        adjustable: false,
        description: "Total size of all tapes in a virtual tape library (VTL)",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "storagegateway",
        quota_code: "L-C86341FE",
        quota_name: "Path length",
        value: 1024.0,
        unit: "Bytes",
        adjustable: false,
        description: "Maximum path length",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "storagegateway",
        quota_code: "L-311F8856",
        quota_name: "Max size of a virtual tape in TiB",
        value: 15.0,
        unit: "None",
        adjustable: false,
        description: "Maximum size of a virtual tape",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    // Three minima of 150 GiB that differ only in the gateway type they apply to, which is
    // exactly what a row borrowing a neighbour's text cannot tell apart.
    Bound {
        service_code: "storagegateway",
        quota_code: "L-6D83F84B",
        quota_name: "Cached volume gateway Upload Buffer Minimum in GiB",
        value: 150.0,
        unit: "None",
        adjustable: false,
        description: "Minimum upload buffer size for Cached Volume Gateway",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "storagegateway",
        quota_code: "L-87640A7E",
        quota_name: "Tape gateway Upload Buffer Minimum in GiB",
        value: 150.0,
        unit: "None",
        adjustable: false,
        description: "Minimum upload buffer size for Tape Gateway",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "storagegateway",
        quota_code: "L-F3EDDF8C",
        quota_name: "Tape gateway Cache Minimum in GiB",
        value: 150.0,
        unit: "None",
        adjustable: false,
        description: "Minimum cache size for Tape Gateway",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    // The time-series service publishes API RATES under two codes the catalog used to
    // serve a timeout and a retention under, so the row's whole CLASS is the code's: the
    // window and the call count come with it.
    Bound {
        service_code: "timestream",
        quota_code: "L-C0E9F2ED",
        quota_name: "ExecuteScheduledQuery request rate",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: "Maximum number of ExecuteScheduledQuery requests per second.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "Timestream",
            api: "ExecuteScheduledQuery",
        },
        resource_plane: None,
    },
    Bound {
        service_code: "timestream",
        quota_code: "L-8DFD99E7",
        quota_name: "UpdateAccountSettings request rate",
        value: 1.0,
        unit: "None",
        adjustable: false,
        description: "Maximum number of UpdateAccountSettings requests per second.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "Timestream",
            api: "UpdateAccountSettings",
        },
        resource_plane: None,
    },
    Bound {
        service_code: "vpc",
        quota_code: "L-8312C5BB",
        quota_name: "VPC peering connection request expiry hours",
        value: 168.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of hours after which an unaccepted VPC peering \
                      connection request expires. The default value is 168 hours (one week).",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    // Four rows whose SCOPE is in the sentence the service words them with — per Region
    // against per account, per Availability Zone against per Region, one VPC against the
    // VPCs it is peered with — so a re-worded description changes the denominator a client
    // computes headroom against.
    Bound {
        service_code: "vpc",
        quota_code: "L-1B52E74A",
        quota_name: "Gateway VPC endpoints per Region",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of gateway VPC endpoints per Region. The maximum is 255 \
                      gateway endpoints per VPC.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "vpc",
        quota_code: "L-BB24F6E5",
        quota_name: "Network Address Usage",
        value: 64_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum Network Address Usage for a single VPC.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "vpc",
        quota_code: "L-DF5E4CA3",
        quota_name: "Network interfaces per Region",
        value: 5000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of network interfaces per Availability Zone in a Region.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "vpc",
        quota_code: "L-E79EC296",
        quota_name: "VPC security groups per Region",
        value: 2500.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of VPC security groups per Region.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    // The service-network catalog gets both `Adjustable` and `Value` from the code: one
    // code publishes an adjustable 500 where the catalog served a fixed 1, another a fixed
    // 5 where it served an adjustable 1000.
    Bound {
        service_code: "vpc-lattice",
        quota_code: "L-0DCA4434",
        quota_name: "Resource gateways per VPC",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of resource gateways in a VPC. For additional capacity \
                      and limit increases, contact AWS Support.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "vpc-lattice",
        quota_code: "L-6095700C",
        quota_name: "Resource Configurations per service network",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "vpc-lattice",
        quota_code: "L-CA6A1CC5",
        quota_name: "Security groups per association",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: "",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
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
    // A quota OF A RESOURCE: the firewall service publishes the association plane of one
    // web ACL as raisable for an individual web ACL as well as account-wide, which is the
    // `QuotaContext` a client tells a per-resource limit from an account-wide one by.
    Bound {
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
    Bound {
        service_code: "wafv2",
        quota_code: "L-D95DED7E",
        quota_name: "Maximum size in kilobytes of a web request body that can be inspected for \
                     CloudFront, API Gateway, Amazon Cognito, App Runner, and Verified Access \
                     protections",
        value: 64.0,
        unit: "None",
        adjustable: false,
        description: "",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    // The desktop service publishes INSTANCE planes under two codes the catalog served an
    // API rate and a certificate count under: a resource count over five minutes rather
    // than a call count over one second.
    Bound {
        service_code: "workspaces",
        quota_code: "L-8C70C791",
        quota_name: "General Purpose Value streaming instances for WorkSpaces Pools",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "WorkSpaces",
            resource: "GeneralPurposeValueStreamingInstancesForWorkSpacesPool",
        },
        resource_plane: None,
    },
    Bound {
        service_code: "workspaces",
        quota_code: "L-63F14199",
        quota_name: "Graphics.g7.2xlarge WorkSpaces",
        value: 0.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "WorkSpaces",
            resource: "GraphicsG72xlargeWorkSpace",
        },
        resource_plane: None,
    },
    Bound {
        service_code: "xray",
        quota_code: "L-C6B6F05D",
        quota_name: "Indexed annotations per trace",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of annotations that can be indexed within a single \
                      trace.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
        service_code: "xray",
        quota_code: "L-E2DD2778",
        quota_name: "Tags per group",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of tags per group.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    // An INGESTION rate the service publishes as a plain count rather than over a window
    // of its own, where the catalog served a retention measured in days.
    Bound {
        service_code: "xray",
        quota_code: "L-E75A2BBF",
        quota_name: "Segments per second",
        value: 2600.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of segments per second you can send to X-Ray.",
        period: None,
        metric: Metric::None,
        resource_plane: None,
    },
    Bound {
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

/// The `Period` a row publishes, as the wire reports it.
fn period_json(period: Option<(i32, &str)>) -> Value {
    match period {
        Some((value, unit)) => json!({"PeriodValue": value, "PeriodUnit": unit}),
        None => Value::Null,
    }
}

/// The `QuotaContext` a row publishes, as the wire reports it: a row managed for the
/// account describes no plane at all, and one published as a quota of a resource names
/// the resource type and the level an increase is askable at.
///
/// `pub(crate)` so the other suites in this target describe the context shape through ONE
/// construction instead of each growing a copy of it.
pub(crate) fn context_json(resource_plane: Option<(&str, &str)>) -> Value {
    match resource_plane {
        Some((scope_type, level)) => json!({
            "ContextScope": "RESOURCE",
            "ContextScopeType": scope_type,
            "AdjustableAtLevel": level,
        }),
        None => Value::Null,
    }
}

/// The record the service publishes under a binding's code, as the wire renders it.
fn published(bound: &Bound, region: &str, service_name: &str) -> Value {
    json!({
        "ServiceCode": bound.service_code,
        "ServiceName": service_name,
        "QuotaCode": bound.quota_code,
        "QuotaArn": format!(
            "arn:aws:servicequotas:{region}::{}/{}",
            bound.service_code, bound.quota_code
        ),
        "QuotaName": bound.quota_name,
        "Value": bound.value,
        "Unit": bound.unit,
        "Adjustable": bound.adjustable,
        "GlobalQuota": false,
        "Description": if bound.description.is_empty() {
            Value::Null
        } else {
            json!(bound.description)
        },
        "Period": period_json(bound.period),
        "UsageMetric": metric_json(&bound.metric),
        "QuotaContext": context_json(bound.resource_plane),
    })
}

/// The record served under a code, projected onto the members a binding fixes.
fn served(quota: &Value) -> Value {
    json!({
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
        "QuotaContext": quota["QuotaContext"],
    })
}

/// `ServiceCode -> ServiceName`, read from the inventory rather than restated here.
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

/// The table has to span the classes the rules generalise over, or the rows below prove
/// nothing about rows of another shape.
#[test]
fn the_table_spans_the_classes_a_binding_can_get_wrong() {
    let services: BTreeSet<&str> = BOUND.iter().map(|bound| bound.service_code).collect();
    let units: BTreeSet<&str> = BOUND.iter().map(|bound| bound.unit).collect();
    let windows: BTreeSet<Option<&str>> = BOUND
        .iter()
        .map(|bound| bound.period.map(|(_, unit)| unit))
        .collect();
    let metric_shapes: BTreeSet<&str> = BOUND
        .iter()
        .map(|bound| match bound.metric {
            Metric::None => "none",
            Metric::Calls { .. } => "calls",
            Metric::Resources { .. } => "resources",
            Metric::ResourceRate { .. } => "resource-rate",
        })
        .collect();
    assert_eq!(
        (
            services.len(),
            units.len() > 4,
            windows.len(),
            metric_shapes.len(),
            BOUND.iter().any(|bound| bound.adjustable),
            BOUND.iter().any(|bound| !bound.adjustable),
            BOUND.iter().any(|bound| bound.resource_plane.is_some()),
            BOUND.iter().any(|bound| bound.description.is_empty()),
            BOUND.iter().any(|bound| bound.value == 0.0),
        ),
        (10, true, 3, 4, true, true, true, true, true),
        "case table: expected ten services, five units {units:?}, three windows {windows:?}, \
         every metric shape {metric_shapes:?}, both adjustabilities, a resource plane, a \
         withheld sentence and a limit of zero"
    );
}

/// RULE: the row published under a code carries the published members of the quota the
/// service publishes UNDER THAT CODE, and the listing, the default read and the applied
/// read all report it.
#[tokio::test]
async fn a_code_publishes_the_members_of_its_own_quota() {
    let engine = emulator();
    // Two Regions: a binding is a fact of the published catalog rather than of one
    // Region's view of it, and the ARN of a default quota names the Region it is read at.
    for region in ["us-east-2", "ap-southeast-2"] {
        let caller = client(&engine, region);
        let labels = labels(&caller).await;
        let mut listings: BTreeMap<&str, Vec<Value>> = BTreeMap::new();

        for bound in BOUND {
            let case = format!("{region} {}/{}", bound.service_code, bound.quota_code);
            let service_name = labels
                .get(bound.service_code)
                .unwrap_or_else(|| panic!("case {case}: the inventory must list the service"));
            let pair =
                json!({"ServiceCode": bound.service_code, "QuotaCode": bound.quota_code});

            let quota =
                caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();
            assert_eq!(
                served(&quota),
                published(bound, region, service_name),
                "case {case}: expected the members the service publishes under this code, got \
                 {quota}"
            );

            // The listing publishes the identical record, under that code and no other.
            let listed = match listings.get(bound.service_code) {
                Some(listed) => listed,
                None => {
                    let listed = defaults(&caller, bound.service_code).await;
                    listings.entry(bound.service_code).or_insert(listed)
                }
            };
            let under_code: Vec<&Value> = listed
                .iter()
                .filter(|row| row["QuotaCode"] == json!(bound.quota_code))
                .collect();
            assert_eq!(
                under_code,
                vec![&quota],
                "case {case}: the listing must carry exactly the record the single read \
                 returned"
            );

            // The APPLIED plane resolves the same key to the same quota while nothing is
            // applied over the default, so a client that resolved a code once reads one
            // quota through either plane.
            let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
            for member in [
                "QuotaName",
                "Value",
                "Unit",
                "Adjustable",
                "GlobalQuota",
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
}

/// RULE: `Adjustable` is a fact of the code — the service takes an increase on the row it
/// publishes as adjustable and refuses one on the row it publishes as fixed.
#[tokio::test]
async fn adjustability_decides_whether_an_increase_is_accepted() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-3");

    for bound in BOUND {
        let case = format!("{}/{}", bound.service_code, bound.quota_code);
        // Above the value in force, so the only thing left to refuse the request for is
        // the quota's published adjustability.
        let desired = bound.value.max(1.0) * 2.0;
        let body = json!({
            "ServiceCode": bound.service_code,
            "QuotaCode": bound.quota_code,
            "DesiredValue": desired,
        });

        if bound.adjustable {
            let accepted = caller.ok("RequestServiceQuotaIncrease", body).await;
            let requested = &accepted["RequestedQuota"];
            assert_eq!(
                (
                    requested["QuotaCode"].as_str(),
                    requested["QuotaName"].as_str(),
                    requested["DesiredValue"].as_f64(),
                    requested["Status"].as_str(),
                ),
                (
                    Some(bound.quota_code),
                    Some(bound.quota_name),
                    Some(desired),
                    Some("PENDING"),
                ),
                "case {case}: an adjustable quota takes the increase, got {accepted}"
            );
            continue;
        }

        let (status, error) = caller.refused("RequestServiceQuotaIncrease", body).await;
        assert_eq!(
            (status, error.as_str()),
            (400, "IllegalArgumentException"),
            "case {case}: a quota the service publishes as fixed refuses an increase"
        );
    }
}
