// SPDX-License-Identifier: CC0-1.0

//! What a service's DEFAULT-QUOTA SET may be published as, over the wire.
//!
//! `ListAWSDefaultServiceQuotas(ServiceCode)` reports the service's published
//! default-quota catalog, and `QuotaCode` is the only key Service Quotas exposes: it is
//! what `GetAWSDefaultServiceQuota`, `GetServiceQuota` and `RequestServiceQuotaIncrease`
//! take. So a short hand-written table keyed on codes derived from quota NAMES hands a
//! client keys no other API accepts and values that are not the service's defaults. The
//! rules asserted here, each over a TABLE of services rather than one case:
//!
//! * a service's whole published catalog is served — every row keyed on the
//!   `L-XXXXXXXX` code AWS mints, none keyed on a code derived from a quota name, and
//!   the catalog is as long as the service's published one;
//! * every row reports that service's published `ServiceName`, the same string
//!   `ListServices` reports for the code;
//! * a published row reports the `QuotaName`, `Value`, `Unit`, `Adjustable`,
//!   `GlobalQuota`, `Period` and `UsageMetric` AWS publishes for the quota, with a
//!   `QuotaArn` naming the Region it was read at and no account, identically through the
//!   listing and through `GetAWSDefaultServiceQuota`, and the same code resolves in the
//!   APPLIED listing;
//! * a code this catalog used to derive from a quota name is published nowhere and
//!   resolves nowhere.
//!
//! The harness (`client`, `emulator`, `default_quotas`, `walk_pages`) is the one
//! `quota_workflow.rs` owns, and the question "is this a code AWS mints" is asked
//! through `catalog_identity.rs`'s single copy of it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::catalog_identity::is_aws_minted_shape;
use crate::quota_workflow::{
    client, default_quotas as defaults, emulator, service_inventory, walk_pages, Client,
};

/// One service whose published default-quota catalog is keyed entirely on AWS's own
/// codes.
struct CatalogService {
    service_code: &'static str,
    /// The display name AWS's own Service Quotas data reports for the code.
    service_name: &'static str,
    /// How many quotas the service's published catalog holds.
    quota_count: usize,
    /// The `QuotaCode`s of that published catalog this emulator does NOT serve: the
    /// codes whose quota no source available here states, which the catalog leaves to
    /// the default-quota-set gap rather than serving another quota's row under (see the
    /// row tables in `src/state/quota_rows.rs`). Named here so the shortfall against
    /// AWS's own count is reported by a check rather than absorbed into it.
    gap_codes: &'static [&'static str],
}

/// The services this suite covers.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response for each of these
/// service codes — every row of it carries an `L-XXXXXXXX` `QuotaCode`, and the count
/// is the number of rows that response holds. The rows span the shapes the rule has to
/// hold over: a catalog of 7 rows and one of 47, a service whose display name is its
/// current product name and services whose display name is a former one, and services
/// whose catalogs are resource planes, request rates, or both.
const SERVICES: &[CatalogService] = &[
    CatalogService {
        service_code: "events",
        service_name: "Amazon EventBridge (CloudWatch Events)",
        quota_count: 21,
        gap_codes: &[],
    },
    CatalogService {
        service_code: "firehose",
        service_name: "Amazon Kinesis Data Firehose",
        quota_count: 15,
        gap_codes: &[],
    },
    CatalogService {
        service_code: "fms",
        service_name: "AWS Firewall Manager",
        quota_count: 30,
        gap_codes: &[],
    },
    CatalogService {
        service_code: "fsx",
        service_name: "Amazon FSx",
        quota_count: 30,
        gap_codes: &[],
    },
    CatalogService {
        service_code: "gamelift",
        service_name: "Amazon GameLift Servers",
        quota_count: 27,
        gap_codes: &[],
    },
    CatalogService {
        service_code: "glacier",
        service_name: "Amazon Glacier",
        quota_count: 8,
        gap_codes: &[],
    },
    CatalogService {
        service_code: "glue",
        service_name: "AWS Glue",
        quota_count: 47,
        gap_codes: &[],
    },
    CatalogService {
        service_code: "groundstation",
        service_name: "AWS Ground Station",
        quota_count: 10,
        gap_codes: &[],
    },
    CatalogService {
        service_code: "guardduty",
        service_name: "Amazon GuardDuty",
        quota_count: 7,
        gap_codes: &[],
    },
    CatalogService {
        service_code: "imagebuilder",
        service_name: "EC2 Image Builder",
        quota_count: 23,
        gap_codes: &[],
    },
];

/// How many `(ServiceCode, QuotaCode)` pairs of these catalogs are published under no row
/// at all — the whole of the `gap_codes` above.
///
/// Pinned as ONE number so that leaving a further code to the default-quota-set gap is a
/// deliberate, visible edit rather than one more entry in a list nothing counts. It is ZERO
/// now that every code these recorded listings carry a row for answers one here: the codes
/// whose own quota no source states carry the plane a sourced binding displaced instead (see
/// the row tables in `src/state/quota_rows.rs`).
const GAP_CODE_COUNT: usize = 0;

/// A code this catalog used to derive from a quota NAME for one of these services, which
/// the service answers to nothing under.
///
/// SOURCE: the recorded default-quota listing of each service reports the row the
/// emulator published under this code as a surplus entry, so the service publishes
/// nothing under it.
const DERIVED_CODES: &[(&str, &str)] = &[
    ("events", "EventBusesPerAccount"),
    ("events", "RulesPerEventBus"),
    ("events", "PutEventsThrottleLimitInTransactionsPerSecond"),
    ("firehose", "DeliveryStreamsPerRegion"),
    ("fms", "PoliciesPerRegion"),
    ("fsx", "FileSystemsPerAccount"),
    ("gamelift", "Fleets"),
    ("glacier", "VaultsPerRegion"),
    ("glue", "ConcurrentJobRunsPerAccount"),
    ("glue", "DatabasesPerAccount"),
    ("glue", "TablesPerDatabase"),
    ("groundstation", "MissionProfilesPerAccount"),
    ("guardduty", "DetectorsPerRegion"),
    ("guardduty", "ThreatIntelSetsPerDetector"),
    ("imagebuilder", "ImagePipelines"),
];

/// One published quota, as the service's default-quota data reports it.
struct Probe {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// `(PeriodValue, PeriodUnit)` for a quota AWS publishes over a window.
    period: Option<(i32, &'static str)>,
    /// The API whose `AWS/Usage` `CallCount` meters the quota, with the `Service`
    /// dimension that count is published under, for a request-rate row.
    call_count: Option<(&'static str, &'static str)>,
}

/// The rows this suite pins, at most two per service so the table spans services rather
/// than repeating one.
///
/// SOURCE: the published default-quota data of each service code, and the recorded
/// default-quota listing of each service for which code names which quota. The rows
/// differ along everything the rule generalises over: a counted resource plane and a
/// request rate, an
/// adjustable row and a not-adjustable one, `Unit` `None` and a size unit, a row
/// with no `Period` and rows published over a one-second window, metered and unmetered
/// rows, and values from 5 to 10,000,000.
const PROBES: &[Probe] = &[
    Probe {
        service_code: "events",
        quota_code: "L-5540C5E3",
        quota_name: "Invocations throttle limit in transactions per second",
        value: 18_750.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "SECOND")),
        call_count: Some(("EventBridge", "Invocations")),
    },
    Probe {
        service_code: "events",
        quota_code: "L-755FD01C",
        quota_name: "Rate of invocations per API destination",
        value: 300.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "SECOND")),
        call_count: None,
    },
    Probe {
        service_code: "firehose",
        quota_code: "L-CC1FDDAA",
        quota_name: "Rate of UpdateDestination requests",
        value: 5.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "SECOND")),
        call_count: Some(("Kinesis Firehose", "UpdateDestination")),
    },
    Probe {
        service_code: "firehose",
        quota_code: "L-ED7DA7FC",
        quota_name: "Rate of TagDeliveryStream requests",
        value: 5.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "SECOND")),
        call_count: Some(("Kinesis Firehose", "TagDeliveryStream")),
    },
    Probe {
        service_code: "fms",
        quota_code: "L-0B28E140",
        quota_name: "Protocols lists per organization",
        value: 20.0,
        unit: "None",
        adjustable: true,
        period: None,
        call_count: None,
    },
    Probe {
        service_code: "fsx",
        quota_code: "L-901C77F5",
        quota_name: "Windows total SSD IOPS",
        value: 500_000.0,
        unit: "None",
        adjustable: true,
        period: None,
        call_count: None,
    },
    Probe {
        service_code: "fsx",
        quota_code: "L-FD89CA8A",
        quota_name: "Windows throughput capacity",
        value: 10_240.0,
        unit: "None",
        adjustable: true,
        period: None,
        call_count: None,
    },
    Probe {
        service_code: "gamelift",
        quota_code: "L-22451070",
        quota_name: "Game session queues per region",
        value: 100.0,
        unit: "None",
        adjustable: true,
        period: None,
        call_count: None,
    },
    Probe {
        service_code: "glacier",
        quota_code: "L-D1C67346",
        quota_name: "Vaults per account",
        value: 1000.0,
        unit: "None",
        adjustable: false,
        period: None,
        call_count: None,
    },
    Probe {
        service_code: "glacier",
        quota_code: "L-E96017CA",
        quota_name: "Archive size in GB.",
        value: 40_000.0,
        unit: "Gigabytes",
        adjustable: false,
        period: None,
        call_count: None,
    },
    Probe {
        service_code: "glue",
        quota_code: "L-08F3B322",
        quota_name: "Max task DPUs per account",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        period: None,
        call_count: None,
    },
    Probe {
        service_code: "glue",
        quota_code: "L-E15CE20A",
        quota_name: "Concurrent machine learning task runs per transform",
        value: 3.0,
        unit: "None",
        adjustable: true,
        period: None,
        call_count: None,
    },
    Probe {
        service_code: "groundstation",
        quota_code: "L-DF7B6DEC",
        quota_name: "Scheduled Contacts Limit",
        value: 100.0,
        unit: "None",
        adjustable: true,
        period: None,
        call_count: None,
    },
    Probe {
        service_code: "guardduty",
        quota_code: "L-8B0D7135",
        quota_name: "Member accounts through AWS Organizations",
        value: 50_000.0,
        unit: "None",
        adjustable: false,
        period: None,
        call_count: None,
    },
    Probe {
        service_code: "imagebuilder",
        quota_code: "L-6105A4EE",
        quota_name: "Lifecycle policies",
        value: 50.0,
        unit: "None",
        adjustable: true,
        period: None,
        call_count: None,
    },
    Probe {
        service_code: "imagebuilder",
        quota_code: "L-7A0E01E3",
        quota_name: "Image pipelines",
        value: 75.0,
        unit: "None",
        adjustable: true,
        period: None,
        call_count: None,
    },
];

/// The `QuotaCode`s a listing of `service_code` reports.
async fn listed_codes(client: &Client, operation: &str, service_code: &str) -> BTreeSet<String> {
    walk_pages(
        client,
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
/// mints, with its published display name on every row.
#[tokio::test]
async fn every_covered_service_serves_its_published_catalog() {
    let engine = emulator();

    // Two endpoints, because the catalog is published per Region and the rule is about
    // the catalog rather than about one Region's view of it.
    for region in ["us-west-2", "eu-central-1"] {
        let client = client(&engine, region);
        let inventory: BTreeMap<String, String> = service_inventory(&client)
            .await
            .iter()
            .filter_map(|service| {
                Some((
                    service["ServiceCode"].as_str()?.to_string(),
                    service["ServiceName"].as_str()?.to_string(),
                ))
            })
            .collect();

        for service in SERVICES {
            let case = format!("{region} {}", service.service_code);
            let quotas = defaults(&client, service.service_code).await;

            // The published catalog, less the codes whose quota no source available here
            // states: those are named per service, so a row that went missing for any
            // other reason is still reported here, and AWS's own count stays pinned.
            assert_eq!(
                quotas.len() + service.gap_codes.len(),
                service.quota_count,
                "case {case}: expected the whole published catalog ({} quotas) less the {} \
                 code(s) left to the default-quota-set gap, got {} ({:?})",
                service.quota_count,
                service.gap_codes.len(),
                quotas.len(),
                quotas
                    .iter()
                    .filter_map(|quota| quota["QuotaCode"].as_str())
                    .collect::<Vec<_>>()
            );
            for gap_code in service.gap_codes {
                assert!(
                    !quotas
                        .iter()
                        .any(|quota| quota["QuotaCode"] == json!(gap_code)),
                    "case {case}: a code whose quota no source states is published under no \
                     row, got one for {gap_code:?}"
                );
            }

            for quota in &quotas {
                let code = quota["QuotaCode"].as_str().unwrap_or_default();
                assert!(
                    is_aws_minted_shape(code),
                    "case {case}: every published quota code is one AWS mints, got {code:?} \
                     in {quota}"
                );
                assert_eq!(
                    quota["ServiceName"],
                    json!(service.service_name),
                    "case {case}: expected the published service name, got {quota}"
                );
                assert_eq!(
                    quota["ServiceCode"],
                    json!(service.service_code),
                    "case {case}: a row reports the service it was read for, got {quota}"
                );
            }

            // Every code is distinct, so each quota is reachable by its own key.
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
                inventory.get(service.service_code).map(String::as_str),
                Some(service.service_name),
                "case {case}: ListServices must report the same display name the quotas \
                 carry, got {:?}",
                inventory.get(service.service_code)
            );
        }

        // The whole of the default-quota-set gap across these catalogs, pinned as one
        // number. This one compares the table above against a constant rather than
        // against a served response — an EDIT TRIPWIRE, so that leaving a further code to
        // the gap is a deliberate change; what the served catalogs are held to is the
        // count equation and the absence of each gap code above.
        assert_eq!(
            SERVICES
                .iter()
                .map(|service| service.gap_codes.len())
                .sum::<usize>(),
            GAP_CODE_COUNT,
            "case {region}: expected {GAP_CODE_COUNT} code(s) published under no row across \
             these catalogs"
        );
    }
}

/// A code this catalog used to derive from a quota name is published nowhere, and
/// resolves nowhere: not in either listing, and not through any single read that takes a
/// `QuotaCode`.
#[tokio::test]
async fn a_code_derived_from_a_quota_name_resolves_nowhere() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    for (service_code, derived) in DERIVED_CODES {
        let case = format!("{service_code}/{derived}");

        for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
            let listed = listed_codes(&client, operation, service_code).await;
            assert!(
                !listed.contains(*derived),
                "case {case}: {operation} must not publish a code derived from a quota name, \
                 got {listed:?}"
            );
        }

        for operation in [
            "GetAWSDefaultServiceQuota",
            "GetServiceQuota",
            "RequestServiceQuotaIncrease",
        ] {
            let mut body = json!({"ServiceCode": service_code, "QuotaCode": derived});
            if operation == "RequestServiceQuotaIncrease" {
                body["DesiredValue"] = json!(1_000.0);
            }
            let (status, error) = client.refused(operation, body).await;
            assert_eq!(
                (status, error.as_str()),
                (400, "NoSuchResourceException"),
                "case {case}: {operation} must refuse a code this service publishes nothing \
                 under"
            );
        }
    }
}

/// A published row reports the facts AWS publishes for that quota, identically through
/// the listing and through the single read, and the applied plane resolves the same key.
#[tokio::test]
async fn a_published_row_reports_the_facts_aws_publishes() {
    let engine = emulator();
    let region = "ap-northeast-1";
    let client = client(&engine, region);

    for probe in PROBES {
        let case = format!("{}/{}", probe.service_code, probe.quota_code);
        let pair = json!({"ServiceCode": probe.service_code, "QuotaCode": probe.quota_code});
        let quota = client.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();

        assert_eq!(
            quota["QuotaName"],
            json!(probe.quota_name),
            "case {case}: expected the published quota name, got {quota}"
        );
        assert_eq!(
            quota["Value"],
            json!(probe.value),
            "case {case}: expected the published default value, got {quota}"
        );
        assert_eq!(
            quota["Unit"],
            json!(probe.unit),
            "case {case}: expected the published unit, got {quota}"
        );
        assert_eq!(
            quota["Adjustable"],
            json!(probe.adjustable),
            "case {case}: expected the published adjustability, got {quota}"
        );
        assert_eq!(
            quota["GlobalQuota"],
            json!(false),
            "case {case}: these quotas are counted per Region, got {quota}"
        );
        assert_eq!(
            quota["QuotaArn"],
            json!(format!(
                "arn:aws:servicequotas:{region}::{}/{}",
                probe.service_code, probe.quota_code
            )),
            "case {case}: an AWS default quota's ARN names the Region and no account, got \
             {quota}"
        );
        match probe.period {
            Some((value, unit)) => assert_eq!(
                quota["Period"],
                json!({"PeriodValue": value, "PeriodUnit": unit}),
                "case {case}: expected the published window, got {quota}"
            ),
            None => assert!(
                quota.get("Period").is_none_or(Value::is_null),
                "case {case}: this quota is published over no window, got {quota}"
            ),
        }
        match probe.call_count {
            Some((usage_service, api)) => assert_eq!(
                quota["UsageMetric"],
                json!({
                    "MetricNamespace": "AWS/Usage",
                    "MetricName": "CallCount",
                    "MetricStatisticRecommendation": "Sum",
                    "MetricDimensions": {
                        "Service": usage_service,
                        "Resource": api,
                        "Type": "API",
                        "Class": "None",
                    },
                }),
                "case {case}: a request rate is metered by that API's call count, got {quota}"
            ),
            None => assert!(
                quota.get("UsageMetric").is_none_or(Value::is_null),
                "case {case}: this quota publishes no usage metric, got {quota}"
            ),
        }

        let listed = defaults(&client, probe.service_code).await;
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );

        // The applied plane is keyed on the same codes, so a client that read a code out
        // of the default catalog resolves its value in force too.
        let applied = client.ok("GetServiceQuota", pair).await["Quota"].clone();
        assert_eq!(
            (
                applied["QuotaCode"].clone(),
                applied["QuotaName"].clone(),
                applied["Value"].clone()
            ),
            (
                json!(probe.quota_code),
                json!(probe.quota_name),
                json!(probe.value)
            ),
            "case {case}: the applied read resolves the same quota at its default value, got \
             {applied}"
        );
    }
}
