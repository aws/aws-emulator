// SPDX-License-Identifier: CC0-1.0

//! The bindings the notification, queueing, sign-on, gateway, time-series, networking,
//! firewall, desktop and tracing catalogs had keyed on a NEIGHBOURING quota, read back
//! under the code each quota is actually published under.
//!
//! `QuotaCode` is the only key Service Quotas exposes — `GetAWSDefaultServiceQuota`,
//! `GetServiceQuota` and `RequestServiceQuotaIncrease` all take it and nothing else — so
//! the row served under a code is a claim about the quota that code IDENTIFIES. The rules
//! driven here over a table of the codes whose quota a source available here states:
//!
//! * the row published under a code carries that quota's own `QuotaName`, `Value`, `Unit`,
//!   `Adjustable`, `GlobalQuota`, `Period`, `UsageMetric` and `Description` — never the
//!   members of the quota next to it in the listing — identically through the listing,
//!   through the single default read and through the applied read;
//! * a member no source available here states is withheld rather than invented: a row
//!   whose published sentence is unknown carries no `Description` at all;
//! * every code a catalog publishes answers a row, so a code read out of AWS's catalog
//!   resolves here instead of making a quota that exists in AWS look nonexistent;
//! * and the quota a binding DISPLACED is not republished under a second code, so one
//!   plane is served under one key.
//!
//! The table spans what the rules generalise over: the ten `ServiceCode`s of this cluster;
//! a per-API rate class beside counted planes and measured capacities; `Unit` `None`,
//! `Kilobytes` and `Minutes`; adjustable and fixed rows; no window beside the one-second
//! window a rate is published over and the five-minute window a resource count is
//! published over; unmetered rows beside all three `AWS/Usage` metric shapes; limits from
//! zero to 120000; and rows whose sentence is published beside rows whose sentence is not.
//!
//! The harness (`client`, `emulator`, `default_quotas`) is the one `quota_workflow.rs`
//! owns; a metric is described through the single `Metric` shape
//! `default_catalogs_sns_through_xray.rs` owns.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::default_catalogs_sns_through_xray::{metric_json, Metric};
use crate::quota_workflow::{client, default_quotas as defaults, emulator, Client};

/// One binding: the code, and the members of the quota published under it.
struct Bound {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// The sentence the service publishes for the quota, or `""` where no source available
    /// here states it: `Description` is optional (`@length 0..350`), so such a row
    /// publishes none rather than one this emulator words itself.
    description: &'static str,
    /// `(PeriodValue, PeriodUnit)`, for a row published over a window.
    period: Option<(i32, &'static str)>,
    metric: Metric,
}

/// The bindings this suite pins.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of each of these ten
/// `ServiceCode`s, which is the reference for WHICH quota a code publishes; the catalog
/// they are seeded from is `src/state/quota_rows/<service>.rs`, which names the source of
/// every member beside it. Every row here is a code this catalog used to publish a
/// neighbouring quota's members under.
const BOUND: &[Bound] = &[
    // The notification catalog's per-API rate class: the window, the call count and the
    // sentence are the class's, while the ACTION the row throttles, its rate and its
    // adjustability are the code's.
    Bound {
        service_code: "sns",
        quota_code: "L-93308085",
        quota_name: "Unsubscribe Transactions per Second",
        value: 100.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of times the Unsubscribe action can be called per \
                      second.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "SNS",
            api: "Unsubscribe",
        },
    },
    Bound {
        service_code: "sns",
        quota_code: "L-4EE37BC0",
        quota_name: "SetEndpointAttributes Transactions per Second",
        value: 900.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of times the SetEndpointAttributes action can be \
                      called per second.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "SNS",
            api: "SetEndpointAttributes",
        },
    },
    Bound {
        service_code: "sns",
        quota_code: "L-B98ECD3E",
        quota_name: "DeleteTopic Transactions per Second",
        value: 30.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of times the DeleteTopic action can be called per \
                      second.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "SNS",
            api: "DeleteTopic",
        },
    },
    // The account's published-MESSAGE rate: metered by the account's published-message
    // series summed over the window rather than by any API's call count.
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
    },
    // The queueing catalog: four codes that had each carried the members of another queue
    // property, and the delay plane the catalog publishes in MINUTES.
    Bound {
        service_code: "sqs",
        quota_code: "L-A01B4DF0",
        quota_name: "UTF-8 Queue Tag Key Length",
        value: 128.0,
        unit: "None",
        adjustable: false,
        description: "The length of a UTF-8 queue tag key.",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "sqs",
        quota_code: "L-F115E65A",
        quota_name: "Messages per Batch",
        value: 10.0,
        unit: "None",
        adjustable: false,
        description: "",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "sqs",
        quota_code: "L-C491D5A4",
        quota_name: "In-Flight Messages per Standard Queue",
        value: 120_000.0,
        unit: "None",
        adjustable: false,
        description: "",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "sqs",
        quota_code: "L-E8A9C91E",
        quota_name: "Queue Name Length",
        value: 80.0,
        unit: "None",
        adjustable: false,
        description: "The number of characters in a queue name.",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "sqs",
        quota_code: "L-A7816957",
        quota_name: "Delay Queue Delay",
        value: 15.0,
        unit: "Minutes",
        adjustable: false,
        description: "",
        period: None,
        metric: Metric::None,
    },
    // The sign-on catalog: the file size whose sentence the listing states, and the three
    // instance planes the service words in its own voice.
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
    },
    Bound {
        service_code: "sso",
        quota_code: "L-0299121C",
        quota_name: "Total number of AWS accounts or applications that can be configured",
        value: 7000.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "sso",
        quota_code: "L-B44C7A29",
        quota_name: "Number of permission sets allowed in IAM Identity Center",
        value: 3500.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "sso",
        quota_code: "L-7E1D4E33",
        quota_name: "Number of groups supported in IAM Identity Center",
        value: 100_000.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
    },
    // The gateway catalog's capacity class, whose unit is named IN the quota's own name:
    // the gateway TYPE and the bound are the code's, the sentence is the class's.
    Bound {
        service_code: "storagegateway",
        quota_code: "L-E0AEEBAC",
        quota_name: "Stored volume gateway Upload Buffer Minimum in GiB",
        value: 150.0,
        unit: "None",
        adjustable: false,
        description: "Minimum upload buffer size for Stored Volume Gateway",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "storagegateway",
        quota_code: "L-14F83003",
        quota_name: "Cached volume gateway Cache Maximum in TiB",
        value: 64.0,
        unit: "None",
        adjustable: false,
        description: "Maximum cache size for Cached Volume Gateway",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "storagegateway",
        quota_code: "L-59D49F15",
        quota_name: "Tape gateway Cache Maximum in TiB",
        value: 64.0,
        unit: "None",
        adjustable: false,
        description: "Maximum cache size for Tape Gateway",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "storagegateway",
        quota_code: "L-81A6E497",
        quota_name: "Stored volume size in TiB",
        value: 16.0,
        unit: "None",
        adjustable: false,
        description: "Maximum size of a stored volume",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "storagegateway",
        quota_code: "L-CEFD39A0",
        quota_name: "File shares per S3 bucket",
        value: 1.0,
        unit: "None",
        adjustable: false,
        description: "",
        period: None,
        metric: Metric::None,
    },
    // The time-series catalog: a rate row and, under the code that had carried another
    // rate, a counted plane metered by a RESOURCE count over the window those counts are
    // published over.
    Bound {
        service_code: "timestream",
        quota_code: "L-AE604CE2",
        quota_name: "ListTables request rate",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: "Maximum number of ListTables requests per second.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "Timestream",
            api: "ListTables",
        },
    },
    Bound {
        service_code: "timestream",
        quota_code: "L-00158752",
        quota_name: "ListScheduledQueries request rate",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: "Maximum number of ListScheduledQueries requests per second.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "Timestream",
            api: "ListScheduledQueries",
        },
    },
    Bound {
        service_code: "timestream",
        quota_code: "L-1222731D",
        quota_name: "Tables per account",
        value: 50_000.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "Timestream",
            resource: "Table",
        },
    },
    // The networking catalog: two planes of one resource, and the peering row whose
    // published sentence names the maximum an increase may take it to.
    Bound {
        service_code: "vpc",
        quota_code: "L-A272D574",
        quota_name: "VPC associations per security group",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "vpc",
        quota_code: "L-DFA99DE7",
        quota_name: "Private IP address quota per NAT gateway",
        value: 8.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "vpc",
        quota_code: "L-7E9ECCDB",
        quota_name: "Active VPC peering connections per VPC",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of active VPC peering connections per VPC. This quota \
                      can be increased up to a maximum of 125.",
        period: None,
        metric: Metric::None,
    },
    // The service-network catalog: the association plane and its INVERSE are two quotas,
    // and each is published under its own code.
    Bound {
        service_code: "vpc-lattice",
        quota_code: "L-75D4A19E",
        quota_name: "Service associations per service network",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of services associated with one service network.",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "vpc-lattice",
        quota_code: "L-73D0F278",
        quota_name: "Domain Verifications per AWS Region",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: None,
        metric: Metric::None,
    },
    // The firewall catalog: the account's regional regex pattern sets, metered by that
    // plane's own series, under the code that had carried a per-web-ACL reference count.
    Bound {
        service_code: "wafv2",
        quota_code: "L-BF0029B0",
        quota_name: "Maximum regex pattern sets per account in WAF for regional",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "AWS WAF",
            resource: "RegexPatternSetsPerAccountRegional",
        },
    },
    // The desktop catalog: a hardware-bundle plane, which starts at ZERO for a new
    // account, under the code that had carried an API request rate.
    Bound {
        service_code: "workspaces",
        quota_code: "L-F64A92CF",
        quota_name: "Graphics.g7.4xlarge WorkSpaces",
        value: 0.0,
        unit: "None",
        adjustable: true,
        description: "",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "WorkSpaces",
            resource: "GraphicsG74xlargeWorkSpace",
        },
    },
    // The tracing catalog: two retention planes measured in days and the account's group
    // plane, every one of them published as NOT adjustable where the catalog served an
    // adjustable API rate.
    Bound {
        service_code: "xray",
        quota_code: "L-998BFF16",
        quota_name: "Trace and service graph retention in days",
        value: 30.0,
        unit: "None",
        adjustable: false,
        description: "",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "xray",
        quota_code: "L-5DB17969",
        quota_name: "Trace data modification period in days",
        value: 7.0,
        unit: "None",
        adjustable: false,
        description: "",
        period: None,
        metric: Metric::None,
    },
    Bound {
        service_code: "xray",
        quota_code: "L-7F259013",
        quota_name: "Groups in an account",
        value: 25.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of groups in this account in the current Region.",
        period: None,
        metric: Metric::None,
    },
];

/// The codes these catalogs publish that this emulator served NO row for, each of which
/// must answer one: a code AWS publishes a quota under that resolves nowhere makes a quota
/// that exists in AWS look nonexistent.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of each `ServiceCode`,
/// which carries a row under every one of these codes.
const PUBLISHED_CODES: &[(&str, &str)] = &[
    ("sqs", "L-6A03DCE9"),
    ("sqs", "L-A7816957"),
    ("storagegateway", "L-6F75AC83"),
    ("timestream", "L-0C6E4BCB"),
    ("timestream", "L-16B72159"),
    ("vpc", "L-2C462E13"),
    ("wafv2", "L-08788A4E"),
    ("xray", "L-3B8EFF88"),
];

/// The quota NAMES a binding above displaced, which are therefore published under no code
/// at all: a plane left with no code of its own is served by no row rather than by a
/// neighbouring row's key.
const DISPLACED_NAMES: &[(&str, &str)] = &[
    ("sqs", "Message Deduplication ID Length"),
    ("sso", "Permission sets"),
    ("storagegateway", "Clients per NFS file share"),
    ("storagegateway", "Snapshot schedules per volume"),
    ("vpc", "Traffic mirror filters per Region"),
    ("vpc-lattice", "Rules per listener"),
    ("workspaces", "Rate of StartWorkspaces requests"),
    ("xray", "Rate of GetServiceGraph requests"),
];

/// The record a binding publishes, projected onto the members it fixes.
fn expected(bound: &Bound) -> Value {
    json!({
        "QuotaCode": bound.quota_code,
        "ServiceCode": bound.service_code,
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
        "Period": match bound.period {
            Some((value, unit)) => json!({"PeriodValue": value, "PeriodUnit": unit}),
            None => Value::Null,
        },
        "UsageMetric": metric_json(&bound.metric),
    })
}

/// The same members, read off a served record.
fn served(quota: &Value) -> Value {
    json!({
        "QuotaCode": quota["QuotaCode"],
        "ServiceCode": quota["ServiceCode"],
        "QuotaName": quota["QuotaName"],
        "Value": quota["Value"],
        "Unit": quota["Unit"],
        "Adjustable": quota["Adjustable"],
        "GlobalQuota": quota["GlobalQuota"],
        "Description": quota["Description"],
        "Period": quota["Period"],
        "UsageMetric": quota["UsageMetric"],
    })
}

/// Every default quota of the services the tables name, read once per service.
async fn catalogs(caller: &Client) -> BTreeMap<&'static str, Vec<Value>> {
    let mut catalogs: BTreeMap<&'static str, Vec<Value>> = BTreeMap::new();
    for service_code in BOUND
        .iter()
        .map(|bound| bound.service_code)
        .chain(PUBLISHED_CODES.iter().map(|(service_code, _)| *service_code))
        .chain(DISPLACED_NAMES.iter().map(|(service_code, _)| *service_code))
    {
        if !catalogs.contains_key(service_code) {
            catalogs.insert(service_code, defaults(caller, service_code).await);
        }
    }
    catalogs
}

/// The table has to span the classes the rule generalises over, or the rows below prove
/// nothing about a row of another shape.
#[test]
fn the_table_spans_the_classes_a_misbinding_can_get_wrong() {
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
    let actual = (
        services.len(),
        units.len(),
        windows.len(),
        metric_shapes.len(),
        BOUND.iter().any(|bound| bound.adjustable),
        BOUND.iter().any(|bound| !bound.adjustable),
        BOUND.iter().any(|bound| bound.description.is_empty()),
        BOUND.iter().any(|bound| bound.value == 0.0),
        BOUND.iter().any(|bound| bound.value >= 100_000.0),
    );
    assert_eq!(
        actual,
        (10, 3, 3, 4, true, true, true, true, true),
        "case table: expected the ten services {services:?}, three units {units:?}, three \
         windows {windows:?}, every metric shape {metric_shapes:?}, both adjustabilities, a \
         withheld sentence, a limit of zero and a limit in the hundreds of thousands, got \
         {actual:?}"
    );
}

/// RULE: the row published under a `QuotaCode` carries the members of the quota THAT code
/// identifies, through the listing, the single default read and the applied read alike.
#[tokio::test]
async fn a_rekeyed_code_publishes_the_members_of_its_own_quota() {
    let engine = emulator();
    // Two Regions: a binding is a fact of the published catalog rather than of one
    // Region's view of it.
    for region in ["us-east-1", "eu-west-2"] {
        let caller = client(&engine, region);
        let catalogs = catalogs(&caller).await;

        for bound in BOUND {
            let case = format!("{region} {}/{}", bound.service_code, bound.quota_code);
            let pair = json!({"ServiceCode": bound.service_code, "QuotaCode": bound.quota_code});

            let quota =
                caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();
            let wanted = expected(bound);
            assert_eq!(
                served(&quota),
                wanted,
                "case {case}: expected {wanted}, got {quota}"
            );

            // The listing carries exactly that record, under that code and no other.
            let under_code: Vec<&Value> = catalogs[bound.service_code]
                .iter()
                .filter(|row| row["QuotaCode"] == json!(bound.quota_code))
                .collect();
            assert_eq!(
                under_code,
                vec![&quota],
                "case {case}: the listing must carry exactly the record the single read \
                 returned"
            );

            // The applied plane resolves the same key to the same quota while nothing is
            // applied over the default.
            let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
            for member in ["QuotaName", "Value", "Unit", "Adjustable", "Description"] {
                assert_eq!(
                    applied[member], quota[member],
                    "case {case}: the applied read must report the same {member}, got {applied}"
                );
            }
        }
    }
}

/// RULE: every code one of these catalogs publishes answers a row of that service, and the
/// quota a binding displaced is published under no code at all.
#[tokio::test]
async fn every_published_code_answers_a_row_and_no_displaced_plane_is_republished() {
    let engine = emulator();
    let caller = client(&engine, "us-west-2");
    let catalogs = catalogs(&caller).await;

    for (service_code, quota_code) in PUBLISHED_CODES {
        let case = format!("{service_code}/{quota_code}");
        let listed: Vec<&Value> = catalogs[service_code]
            .iter()
            .filter(|row| row["QuotaCode"] == json!(quota_code))
            .collect();
        assert_eq!(
            listed.len(),
            1,
            "case {case}: a code the catalog publishes answers exactly one row, got {listed:?}"
        );
        // Reachable by that key through the single read too, so a client that read the
        // code out of the listing resolves it.
        let quota = caller
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            )
            .await["Quota"]
            .clone();
        assert_eq!(
            quota["QuotaCode"],
            json!(quota_code),
            "case {case}: the single read resolves the code it was asked for, got {quota}"
        );
    }

    for (service_code, quota_name) in DISPLACED_NAMES {
        let carried: Vec<&str> = catalogs[service_code]
            .iter()
            .filter(|row| row["QuotaName"] == json!(quota_name))
            .filter_map(|row| row["QuotaCode"].as_str())
            .collect();
        assert!(
            carried.is_empty(),
            "case {service_code}/{quota_name:?}: a plane a binding displaced is published \
             under no code, got it under {carried:?}"
        );
    }
}
