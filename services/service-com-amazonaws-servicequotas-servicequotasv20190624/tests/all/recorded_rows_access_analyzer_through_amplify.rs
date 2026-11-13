// SPDX-License-Identifier: CC0-1.0

//! The default-quota catalogs of the service codes whose recorded listings bind every
//! row to an AWS-minted `QuotaCode`, over the wire.
//!
//! A `QuotaCode` is the identity a client resolves a limit by, and AWS alone mints into
//! the `L-XXXXXXXX` namespace: a row served under a code of that shape is a claim about
//! the quota AWS publishes under THAT code, and a row served under a code this emulator
//! made up is an identifier AWS rejects. The rules asserted here, each over a TABLE of
//! inputs rather than one quota:
//!
//! * a published row carries the quota's own published facts — `QuotaName`, `Value`,
//!   `Unit`, `Adjustable`, `GlobalQuota`, the `Description` where the quota's text is
//!   known, the `Period` it is measured over and the `UsageMetric` its usage is
//!   published in — reported identically by the single read and by the listing, with a
//!   `QuotaArn` of the form `arn:aws:servicequotas:<region>::<serviceCode>/<quotaCode>`;
//! * every row these service codes publish sits under an AWS-minted code, so a client
//!   that reads a code out of one of these listings holds a code AWS would accept;
//! * a code one of these services publishes NOTHING under resolves nothing — neither a
//!   descriptive stand-in this catalog once minted nor an AWS code whose quota it does
//!   not carry — while the service code itself keeps resolving its listing;
//! * `Adjustable` is backed: a quota published as fixed refuses
//!   `RequestServiceQuotaIncrease`, an adjustable one takes it.
//!
//! The rows span what the rules generalise over: a plain count, a count with a unit of
//! its own, a per-API rate with a one-second window, a quota metered over an hour, a
//! quota metered over five minutes, adjustable and fixed rows, values from 1 to 100, and
//! names in prose, in slug and in sentence form. The harness (`client`, `emulator`,
//! `default_quotas`) is the one `quota_workflow.rs` owns.

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas as defaults, emulator};

/// One published default-quota row, with the members AWS publishes for it.
struct Row {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// The published description, where the evidence states its text; `None` pins none,
    /// because a wrong assertion would be worse than no assertion.
    description: Option<&'static str>,
    /// `(PeriodValue, PeriodUnit)` for a quota AWS publishes over a window.
    period: Option<(i32, &'static str)>,
    /// `(MetricName, Service, Resource, Type)` of the `AWS/Usage` series the quota's
    /// usage is published in, for a metered quota.
    metric: Option<(&'static str, &'static str, &'static str, &'static str)>,
}

/// The rows this suite pins.
///
/// SOURCE: AWS's published Service Quotas data for each `ServiceCode` (what `aws
/// service-quotas list-aws-default-service-quotas --service-code <code>` reports), and
/// the recorded default-quota listing of that code for which `QuotaCode` names which
/// quota — which is where `src/state.rs::seed_quota_catalog` and
/// `src/state/quota_rows/` take each row from.
fn rows() -> Vec<Row> {
    vec![
        // A count AWS publishes as fixed, with its own published sentence.
        Row {
            service_code: "access-analyzer",
            quota_code: "L-4599575C",
            quota_name: "Concurrent policy generations",
            value: 1.0,
            unit: "None",
            adjustable: false,
            description: Some("The maximum number of concurrent policy generations."),
            period: None,
            metric: None,
        },
        // The analyzer count's AWS code is published with a name and a description this
        // evidence withholds, so no row is served under it (see
        // `UNPUBLISHED_QUOTA_CODES`).
        // The per-API request-rate class of AWS Private CA: one-second window, the
        // class's own sentence, and a per-operation limit.
        Row {
            service_code: "acm-pca",
            quota_code: "L-506CC328",
            quota_name: "Rate of PutPolicy requests",
            value: 5.0,
            unit: "None",
            adjustable: false,
            description: Some(
                "The maximum number of PutPolicy requests that can be performed in an account \
                 in the current Region each second.",
            ),
            period: Some((1, "SECOND")),
            metric: None,
        },
        Row {
            service_code: "acm-pca",
            quota_code: "L-4065FDCB",
            quota_name: "Rate of DeleteCertificateAuthority requests",
            value: 10.0,
            unit: "None",
            adjustable: false,
            description: Some(
                "The maximum number of DeleteCertificateAuthority requests that can be \
                 performed in an account in the current Region each second.",
            ),
            period: Some((1, "SECOND")),
            metric: None,
        },
        Row {
            service_code: "acm-pca",
            quota_code: "L-5A3CF806",
            quota_name: "Rate of ListCertificateAuthorities requests",
            value: 20.0,
            unit: "None",
            adjustable: false,
            description: Some(
                "The maximum number of ListCertificateAuthorities requests that can be \
                 performed in an account in the current Region each second.",
            ),
            period: Some((1, "SECOND")),
            metric: None,
        },
        Row {
            service_code: "acm-pca",
            quota_code: "L-773738D3",
            quota_name: "Rate of CreatePermission requests",
            value: 1.0,
            unit: "None",
            adjustable: false,
            description: Some(
                "The maximum number of CreatePermission requests that can be performed in an \
                 account in the current Region each second.",
            ),
            period: Some((1, "SECOND")),
            metric: None,
        },
        // The agent-registry rate row, whose published sentence names the account and
        // the Region the rate is counted on.
        Row {
            service_code: "agent-registry",
            quota_code: "L-7F384C37",
            quota_name: "InvokeRegistryMcp requests per second",
            value: 10.0,
            unit: "Count/Second",
            adjustable: true,
            description: Some(
                "The maximum number of InvokeRegistryMcp API requests per second per account \
                 per Region",
            ),
            period: Some((1, "SECOND")),
            metric: Some(("CallCount", "Agent Registry", "InvokeRegistryMcp", "API")),
        },
        Row {
            service_code: "agent-registry",
            quota_code: "L-4BB62FF5",
            quota_name: "DeleteRegistryRecord requests per second",
            value: 10.0,
            unit: "Count/Second",
            adjustable: false,
            description: Some(
                "The maximum number of DeleteRegistryRecord API requests per second per \
                 account per Region",
            ),
            period: Some((1, "SECOND")),
            metric: Some(("CallCount", "Agent Registry", "DeleteRegistryRecord", "API")),
        },
        // A throttle-rate row of a service whose class AWS publishes as fixed, at the
        // per-operation limit its recorded listing states for it.
        Row {
            service_code: "aco-automation",
            quota_code: "L-1501440B",
            quota_name: "Throttle rate limit for ListAutomationRules",
            value: 5.0,
            unit: "None",
            adjustable: false,
            description: Some(
                "Maximum number of ListAutomationRules requests you can make per second per \
                 account in the current region.",
            ),
            period: Some((1, "SECOND")),
            metric: None,
        },
        // A quota named with the slug of what it counts.
        Row {
            service_code: "aidevops",
            quota_code: "L-510AF4A9",
            quota_name: "agent-spaces-per-account-per-region",
            value: 100.0,
            unit: "None",
            adjustable: true,
            description: None,
            period: None,
            metric: None,
        },
        // A metered quota published over an HOUR, against the five-minute window a
        // resource count is published over.
        Row {
            service_code: "aiops",
            quota_code: "L-4CD81CD5",
            quota_name: "Monthly investigations",
            value: 150.0,
            unit: "None",
            adjustable: false,
            description: None,
            period: Some((1, "HOUR")),
            metric: Some((
                "ResourceCount",
                "Amazon AI Operations",
                "MonthlyAIAssistedInvestigations",
                "Resource",
            )),
        },
        Row {
            service_code: "aiops",
            quota_code: "L-5EC19707",
            quota_name: "Investigation groups",
            value: 1.0,
            unit: "None",
            adjustable: false,
            description: None,
            period: None,
            metric: None,
        },
        Row {
            service_code: "airflow",
            quota_code: "L-78AC1883",
            quota_name: "Workers per environment",
            value: 25.0,
            unit: "None",
            adjustable: true,
            description: Some("The maximum number of workers per environment."),
            period: None,
            metric: None,
        },
        Row {
            service_code: "amplify",
            quota_code: "L-2FC3A2FA",
            quota_name: "Maximum app creations per hour",
            value: 25.0,
            unit: "None",
            adjustable: false,
            description: None,
            period: None,
            metric: None,
        },
        // The branch plane of an Amplify app, re-keyed off a stand-in code onto AWS's
        // own: same published quota, the code AWS resolves it by.
        Row {
            service_code: "amplify",
            quota_code: "L-A6716586",
            quota_name: "Branches per app",
            value: 50.0,
            unit: "None",
            adjustable: true,
            description: None,
            period: None,
            metric: None,
        },
    ]
}

/// The service codes whose recorded default-quota listings publish every row under an
/// AWS-minted code, so no row of theirs may carry a code this emulator minted.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of each code, which
/// reports a descriptive stand-in as a surplus entry.
const MINTED_CODE_SERVICES: &[&str] = &[
    "access-analyzer",
    "acm",
    "acm-pca",
    "aco-automation",
    "agent-registry",
    "aidevops",
    "aiops",
    "airflow",
    "amplify",
];

/// `(ServiceCode, QuotaCode, why)` triples these services publish NOTHING under: the
/// descriptive stand-ins their recorded listings report as surplus entries, and AWS
/// codes of their own namespace whose quota this catalog carries no published facts for.
const UNPUBLISHED_QUOTA_CODES: &[(&str, &str, &str)] = &[
    (
        "access-analyzer",
        "AnalyzersPerRegion",
        "a stand-in code the recorded listing reports as a surplus entry",
    ),
    ("acm", "ACMCertificates", "a surplus stand-in code"),
    ("acm", "ImportedCertificates", "a surplus stand-in code"),
    ("acm-pca", "CertificateAuthorities", "a surplus stand-in code"),
    ("amplify", "AppsPerRegion", "a surplus stand-in code"),
    (
        "amplify",
        "BranchesPerApp",
        "a stand-in re-keyed onto the AWS code for the same quota",
    ),
    (
        "access-analyzer",
        "L-2F63646F",
        "an AWS code whose published quota name this evidence withholds",
    ),
    (
        "access-analyzer",
        "L-FD4CD927",
        "an AWS code whose published quota this catalog carries no facts for",
    ),
    (
        "acm",
        "L-8849A1A8",
        "an AWS code whose published per-operation rate this catalog carries no facts for",
    ),
    (
        "aco-automation",
        "L-D0AFC926",
        "an AWS code whose published throttle limit this catalog carries no facts for",
    ),
];

/// RULE: a published row reports the facts AWS publishes under its own code, through the
/// single read and the listing alike, at the Region it was read at.
#[tokio::test]
async fn a_published_row_carries_the_facts_aws_publishes_under_its_code() {
    let engine = emulator();
    let region = "us-west-2";
    let caller = client(&engine, region);

    for row in rows() {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let quota = caller
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code}),
            )
            .await["Quota"]
            .clone();

        assert_eq!(
            (
                quota["ServiceCode"].as_str(),
                quota["QuotaCode"].as_str(),
                quota["QuotaName"].as_str(),
                quota["Value"].as_f64(),
                quota["Unit"].as_str(),
                quota["Adjustable"].as_bool(),
                quota["GlobalQuota"].as_bool(),
            ),
            (
                Some(row.service_code),
                Some(row.quota_code),
                Some(row.quota_name),
                Some(row.value),
                Some(row.unit),
                Some(row.adjustable),
                Some(false),
            ),
            "case {case}: expected the facts published under this code, got {quota}"
        );
        assert_eq!(
            quota["QuotaArn"],
            json!(format!(
                "arn:aws:servicequotas:{region}::{}/{}",
                row.service_code, row.quota_code
            )),
            "case {case}: a default quota's ARN names the Region and no account, got {quota}"
        );
        if let Some(description) = row.description {
            assert_eq!(
                quota["Description"],
                json!(description),
                "case {case}: a description is the quota's own published text, got {quota}"
            );
        }
        match row.period {
            Some((value, unit)) => assert_eq!(
                quota["Period"],
                json!({"PeriodValue": value, "PeriodUnit": unit}),
                "case {case}: expected the window AWS publishes the quota over, got {quota}"
            ),
            None => assert_eq!(
                quota["Period"],
                Value::Null,
                "case {case}: a quota AWS publishes no window for reports none, got {quota}"
            ),
        }
        match row.metric {
            Some((metric_name, service, resource, usage_type)) => assert_eq!(
                (
                    quota["UsageMetric"]["MetricNamespace"].as_str(),
                    quota["UsageMetric"]["MetricName"].as_str(),
                    quota["UsageMetric"]["MetricDimensions"]["Service"].as_str(),
                    quota["UsageMetric"]["MetricDimensions"]["Resource"].as_str(),
                    quota["UsageMetric"]["MetricDimensions"]["Type"].as_str(),
                ),
                (
                    Some("AWS/Usage"),
                    Some(metric_name),
                    Some(service),
                    Some(resource),
                    Some(usage_type),
                ),
                "case {case}: expected the series the quota's usage is published in, got {quota}"
            ),
            None => assert_eq!(
                quota["UsageMetric"],
                Value::Null,
                "case {case}: a quota CloudWatch publishes no usage for reports no metric, got \
                 {quota}"
            ),
        }

        // The listing carries exactly the record the single read returned.
        let listed = defaults(&caller, row.service_code).await;
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );
    }
}

/// RULE: every row these services publish sits in AWS's minted `L-` namespace, so a
/// client never reads a code AWS would reject out of one of their listings.
#[tokio::test]
async fn every_row_of_these_services_is_published_under_an_aws_minted_code() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

    for service_code in MINTED_CODE_SERVICES {
        let listed = defaults(&caller, service_code).await;
        assert!(
            !listed.is_empty(),
            "case {service_code}: expected the service's published catalog, got no rows"
        );
        for quota in &listed {
            let code = quota["QuotaCode"].as_str().unwrap_or_default();
            let case = format!("{service_code}/{code}");
            let minted = code.len() == 10
                && code.starts_with("L-")
                && code[2..]
                    .chars()
                    .all(|char| char.is_ascii_digit() || char.is_ascii_uppercase());
            assert!(
                minted,
                "case {case}: every row of this service is published under an AWS-minted \
                 L-XXXXXXXX code, got {code:?} for {quota}"
            );
        }
    }
}

/// RULE: a code one of these services publishes nothing under resolves nothing, while
/// the service code itself keeps resolving its listing.
#[tokio::test]
async fn a_code_these_services_publish_nothing_under_resolves_nothing() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    for (service_code, quota_code, why) in UNPUBLISHED_QUOTA_CODES {
        let case = format!("{service_code}/{quota_code}");
        let listed = defaults(&caller, service_code).await;
        let codes: Vec<&str> = listed
            .iter()
            .filter_map(|quota| quota["QuotaCode"].as_str())
            .collect();
        assert!(
            !codes.contains(quota_code),
            "case {case} ({why}): an unpublished code must not be listed, got {codes:?}"
        );

        for operation in ["GetAWSDefaultServiceQuota", "GetServiceQuota"] {
            let (status, error) = caller
                .refused(
                    operation,
                    json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
                )
                .await;
            assert_eq!(
                (status, error.as_str()),
                (400, "NoSuchResourceException"),
                "case {case} ({why}): {operation} must refuse a quota this service does not \
                 publish"
            );
        }

        // The service code still resolves: it is the QUOTA that is unpublished.
        assert!(
            !listed.is_empty(),
            "case {case}: the service must still serve its published catalog, got no rows"
        );
    }
}

// The per-API rate CLASS rule of these services — every row of a class carries that
// class's window, unit, usage metric and sentence with the operation its own `QuotaName`
// states named in them — is asserted by `published_row_class_wording.rs`, which owns that
// rule and now carries the four classes this catalog publishes for `acm`, `acm-pca`,
// `aco-automation` and `agent-registry` as rows of its own `CLASSES` table.

/// RULE: `Adjustable` decides whether the service takes an increase request — a row
/// published as fixed refuses one, an adjustable row accepts it.
#[tokio::test]
async fn adjustability_decides_whether_an_increase_is_accepted() {
    let engine = emulator();
    let caller = client(&engine, "ap-northeast-1");

    for row in rows() {
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

        let (status, error) = caller.refused("RequestServiceQuotaIncrease", body).await;
        assert_eq!(
            (status, error.as_str()),
            (400, "IllegalArgumentException"),
            "case {case}: a quota AWS publishes as not adjustable refuses an increase"
        );
    }
}
