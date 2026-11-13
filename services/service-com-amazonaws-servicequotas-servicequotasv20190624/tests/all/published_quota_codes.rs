// SPDX-License-Identifier: CC0-1.0

//! Which `QuotaCode` a service's default-quota catalog may publish a quota under,
//! over the wire.
//!
//! `QuotaCode` is the only stable key Service Quotas exposes, and it is the input to
//! `GetServiceQuota`, `GetAWSDefaultServiceQuota` and `RequestServiceQuotaIncrease`,
//! so the rule this suite asserts is about the identity of a row rather than about
//! one recorded listing:
//!
//! * a quota is served ONLY under the code the service itself publishes it under —
//!   AWS's minted `L-…` code where this catalog cites it from AWS's published data —
//!   so for a service whose published rows all carry minted codes, every row this
//!   catalog serves carries one;
//! * a code the service publishes nothing under is served by nothing: it is absent
//!   from both listings and refused by both single reads, which is what keeps a
//!   client holding a documented code from reading back a quota this emulator made
//!   up;
//! * the SERVICE still resolves either way — it is the quota that is unpublished, so
//!   the inventory still lists the code under its display name and the listing still
//!   answers with its (possibly empty) `Quotas`;
//! * a published row reports every member AWS publishes with it: the window a rate
//!   quota is measured over, and the usage metric in the namespace the service
//!   publishes its usage in.
//!
//! Each rule is driven over a TABLE: ten `ServiceCode`s whose catalogs differ in how
//! many rows they publish (none, one, two) and in how many withdrawn codes they
//! carry (one, two, three), and the published rows span both a one-second and a
//! 24-hour window, a metered and an unmetered row, and both usage-metric namespaces.
//! The harness (`client`, `walk_pages`) is the one `quota_workflow.rs` owns.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::catalog_identity::is_aws_minted_shape;
use crate::quota_workflow::{client, default_quotas as defaults, emulator, walk_pages, Client};

/// One service of the catalog: its display name, the codes it publishes nothing
/// under, and every code it does publish.
struct Service {
    /// `ServiceCode`.
    code: &'static str,
    /// The display name `ListServices` reports for the code, which every quota of
    /// the service repeats as its `ServiceName`.
    name: &'static str,
    /// Codes no quota of this service is published under.
    ///
    /// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response for the service,
    /// which reports a row under any of these as a surplus entry. They are the
    /// descriptive stand-ins a quota name yields
    /// (`src/state.rs::descriptive_quota_code`), which is why a catalog row that
    /// grows back without AWS's code for it fails here.
    withdrawn: &'static [&'static str],
    /// Every code the catalog publishes for the service, in listing order.
    published: &'static [&'static str],
}

/// The services this suite spans.
fn services() -> Vec<Service> {
    vec![
        Service {
            code: "memorydb",
            name: "Amazon MemoryDB",
            withdrawn: &["NodesPerRegion"],
            published: &[],
        },
        Service {
            code: "mgn",
            name: "AWS Application Migration Service",
            withdrawn: &["SourceServersPerAccount"],
            published: &[],
        },
        Service {
            code: "monitoring",
            name: "Amazon CloudWatch",
            withdrawn: &[
                "AlarmsPerRegion",
                "DashboardsPerAccount",
                "MetricStreamsPerAccount",
            ],
            published: &["L-6FCAAA2E", "L-69C44FFD", "L-6300B446", "L-722EAE58"],
        },
        Service {
            code: "mq",
            name: "Amazon MQ",
            withdrawn: &["BrokersPerRegion"],
            published: &[],
        },
        Service {
            code: "neptune",
            name: "Amazon Neptune",
            withdrawn: &["Clusters"],
            published: &[],
        },
        Service {
            code: "personalize",
            name: "Amazon Personalize",
            withdrawn: &["DatasetGroupsPerAccount"],
            published: &[],
        },
        Service {
            code: "securityhub",
            name: "AWS Security Hub",
            withdrawn: &["MemberAccountsPerAdministratorAccount"],
            published: &[],
        },
        Service {
            code: "servicecatalog",
            name: "AWS Service Catalog",
            withdrawn: &["Portfolios"],
            published: &[],
        },
        Service {
            code: "ses",
            name: "Amazon Simple Email Service(Amazon SES)",
            withdrawn: &["DailySendingQuota", "SendingRate"],
            published: &["L-CDEF9B6B", "L-804C8AE8", "L-E7F21B4C", "L-10E24536"],
        },
        Service {
            code: "signer",
            name: "AWS Signer",
            withdrawn: &["SigningProfilesPerAccount"],
            published: &[],
        },
    ]
}

/// One published row, with every member AWS publishes for it.
struct Row {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    /// `(PeriodValue, PeriodUnit)` — the window the quota is measured over.
    period: (i32, &'static str),
    /// The description AWS publishes, or `None` where the evidence states none: a
    /// wrong assertion would be worse than no assertion.
    description: Option<&'static str>,
    /// The whole `UsageMetric`, as the service publishes its usage.
    metric: Value,
}

/// The rows the catalog publishes for the services above.
///
/// SOURCE: AWS's published default-quota data for each `ServiceCode` (what
/// `aws service-quotas list-aws-default-service-quotas --service-code <code>`
/// reports). The rows span the class: a per-API request rate measured in `AWS/Usage`
/// with the call count of the API it throttles, a sending rate measured in the
/// service's OWN namespace with no dimensions, and a quota published over a 24-hour
/// window with no usage metric at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            service_code: "monitoring",
            quota_code: "L-6FCAAA2E",
            quota_name: "Rate of GetMetricWidgetImage requests",
            value: 20.0,
            period: (1, "SECOND"),
            description: Some(
                "The maximum number of GetMetricWidgetImage requests that you can make, per \
                 second, in this account in the current region.",
            ),
            metric: json!({
                "MetricNamespace": "AWS/Usage",
                "MetricName": "CallCount",
                "MetricStatisticRecommendation": "Sum",
                "MetricDimensions": {
                    "Service": "CloudWatch",
                    "Class": "None",
                    "Resource": "GetMetricWidgetImage",
                    "Type": "API",
                },
            }),
        },
        Row {
            service_code: "ses",
            quota_code: "L-CDEF9B6B",
            quota_name: "Sending rate",
            value: 1.0,
            period: (1, "SECOND"),
            description: Some(
                "The maximum number of emails that Amazon SES can accept each second for \
                 this account in the current Region.",
            ),
            metric: json!({
                "MetricNamespace": "AWS/SES",
                "MetricName": "Send",
                "MetricStatisticRecommendation": "Sum",
                "MetricDimensions": {},
            }),
        },
        Row {
            service_code: "ses",
            quota_code: "L-804C8AE8",
            quota_name: "Sending quota",
            value: 200.0,
            period: (24, "HOUR"),
            description: Some(
                "The maximum number of emails that you can send in a 24-hour period for \
                 this account in the current Region.",
            ),
            metric: json!({
                "MetricNamespace": "AWS/Usage",
                "MetricName": "CallCount",
                "MetricStatisticRecommendation": "Maximum",
                "MetricDimensions": {
                    "Class": "None",
                    "Resource": "SendLast24Hours",
                    "Service": "SES",
                    "Type": "API",
                },
            }),
        },
    ]
}

/// Every `QuotaCode` a listing of `service_code` reports, in listing order.
async fn listed_codes(caller: &Client, operation: &str, service_code: &str) -> Vec<String> {
    let listed = walk_pages(
        caller,
        operation,
        &json!({"ServiceCode": service_code}),
        "Quotas",
        None,
    )
    .await;
    listed
        .iter()
        .map(|quota| {
            quota["QuotaCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case {service_code}: a quota without a code: {quota}"))
                .to_string()
        })
        .collect()
}

/// A service publishes exactly the codes it publishes: each withdrawn code is served
/// by nothing, while the service itself still resolves.
#[tokio::test]
async fn a_code_the_service_publishes_nothing_under_is_served_by_nothing() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

    let inventory: BTreeMap<String, String> = crate::quota_workflow::service_inventory(&caller)
        .await
        .iter()
        .filter_map(|service| {
            Some((
                service["ServiceCode"].as_str()?.to_string(),
                service["ServiceName"].as_str()?.to_string(),
            ))
        })
        .collect();

    for service in services() {
        let case = service.code;

        // The service resolves: the inventory lists it under its display name, which
        // is a different string from its code.
        assert_eq!(
            inventory.get(case).map(String::as_str),
            Some(service.name),
            "case {case}: expected the display name AWS lists the code under, got {:?}",
            inventory.get(case)
        );

        // Both listings report exactly the published codes — no withdrawn code, and
        // nothing beyond the published set, whichever listing is walked.
        for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
            let codes = listed_codes(&caller, operation, case).await;
            assert_eq!(
                codes,
                service.published,
                "case {case}: {operation} must report exactly the codes the service publishes, \
                 got {codes:?}"
            );
            for code in &codes {
                assert!(
                    is_aws_minted_shape(code),
                    "case {case}: {operation} published {code:?}, which is not a code cited \
                     from AWS's minted namespace"
                );
            }
        }

        // Every withdrawn code is refused by both single reads: a client holding one
        // reads no quota back from it.
        for code in service.withdrawn {
            for operation in ["GetAWSDefaultServiceQuota", "GetServiceQuota"] {
                let (status, error) = caller
                    .refused(
                        operation,
                        json!({"ServiceCode": case, "QuotaCode": code}),
                    )
                    .await;
                assert_eq!(
                    (status, error.as_str()),
                    (400, "NoSuchResourceException"),
                    "case {case}/{code}: {operation} must refuse a code the service publishes \
                     nothing under"
                );
            }
        }

        // The listing still answers for a service with no published row at all: an
        // empty `Quotas` list, never a refusal.
        let response = caller
            .ok(
                "ListAWSDefaultServiceQuotas",
                json!({"ServiceCode": case}),
            )
            .await;
        assert!(
            response["Quotas"].is_array(),
            "case {case}: the service code must still resolve its listing, got {response}"
        );
    }
}

/// Each published row reports the members AWS publishes with it, identically through
/// the listing and both single reads.
#[tokio::test]
async fn a_published_row_reports_every_member_aws_publishes_with_it() {
    let engine = emulator();
    let region = "ap-southeast-2";
    let caller = client(&engine, region);
    let names: BTreeMap<&str, &str> = services()
        .iter()
        .map(|service| (service.code, service.name))
        .collect();

    for row in rows() {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let quota = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();

        assert_eq!(
            quota["QuotaName"],
            json!(row.quota_name),
            "case {case}: expected the published quota name, got {quota}"
        );
        assert_eq!(
            quota["ServiceName"],
            json!(names.get(row.service_code).copied().unwrap_or_default()),
            "case {case}: a row repeats the display name of its service, got {quota}"
        );
        assert_eq!(
            quota["Value"],
            json!(row.value),
            "case {case}: expected the published default value, got {quota}"
        );
        assert_eq!(
            quota["Unit"],
            json!("None"),
            "case {case}: expected the published unit, got {quota}"
        );
        assert_eq!(
            quota["Adjustable"],
            json!(true),
            "case {case}: AWS publishes this quota as adjustable, got {quota}"
        );
        assert_eq!(
            quota["GlobalQuota"],
            json!(false),
            "case {case}: AWS counts this quota per Region, got {quota}"
        );
        assert_eq!(
            quota["Period"],
            json!({"PeriodValue": row.period.0, "PeriodUnit": row.period.1}),
            "case {case}: expected the window AWS measures the quota over, got {quota}"
        );
        assert_eq!(
            quota["UsageMetric"], row.metric,
            "case {case}: expected the usage metric the service publishes, got {quota}"
        );
        if let Some(description) = row.description {
            assert_eq!(
                quota["Description"],
                json!(description),
                "case {case}: a description is the service's own published text, got {quota}"
            );
        }
        assert_eq!(
            quota["QuotaArn"],
            json!(format!(
                "arn:aws:servicequotas:{region}::{}/{}",
                row.service_code, row.quota_code
            )),
            "case {case}: a default quota's ARN names the Region and no account, got {quota}"
        );

        // The listing carries exactly the record the single read returned, and the
        // applied read answers the same record while nothing is applied over it.
        let listed = defaults(&caller, row.service_code).await;
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );
        let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
        for member in ["QuotaCode", "QuotaName", "Value", "Unit", "Period"] {
            assert_eq!(
                applied[member], quota[member],
                "case {case}: with nothing applied the value in force must agree on {member}, \
                 got {applied}"
            );
        }
    }
}
