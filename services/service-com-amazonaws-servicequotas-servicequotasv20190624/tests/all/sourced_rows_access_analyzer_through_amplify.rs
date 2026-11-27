// SPDX-License-Identifier: CC0-1.0

//! The default-quota rows of `access-analyzer` through `amplify` whose published facts
//! the recorded listings of those `ServiceCode`s state, over the wire.
//!
//! THE RULE: `ListAWSDefaultServiceQuotas` reports ONE row per quota the service
//! publishes for the code asked for, and the row served under a `QuotaCode` carries that
//! quota's own published `QuotaName`, `Value`, `Unit`, `Adjustable`, `GlobalQuota`,
//! `Period`, `UsageMetric`, `QuotaContext` and `Description` — the `Description`
//! SERIALIZED whenever the service publishes one, and never a neighbouring quota's text.
//! A quota the service publishes is therefore resolvable by its code, through the listing
//! and through the single read alike.
//!
//! The rows below span what the rule generalises over rather than one recorded response:
//! eight service codes; a plain count, a count with a unit of its own, two sizes in
//! gigabytes, a per-API rate with a one-second window, a resource count metered over five
//! minutes, a count of six figures and values from 3 to 1,000,000; adjustable rows beside
//! fixed ones; names in prose, in slug and in sentence form; a row counted on a RESOURCE
//! beside account-managed rows; rows carrying a usage metric beside rows AWS publishes
//! none for; and rows whose published sentence the evidence states beside rows it does
//! not, which publish no `Description` at all.
//!
//! SOURCE for every expectation: AWS's published Service Quotas data for that service
//! code, and the recorded `ListAWSDefaultServiceQuotas` response of that code for which
//! `QuotaCode` publishes which quota and for the name, value, unit, adjustability,
//! window, metric, context and text it states — which is where
//! `src/state/quota_rows/<service>.rs` takes each row from.
//! `smithy_query(query="shape", shape="ServiceQuota")` for the modelled members and for
//! the `Description` length asserted below.
//!
//! The harness (`client`, `emulator`, `default_quotas`, `context_json`) is the one
//! `quota_workflow.rs` and `recorded_bindings_sns_through_xray.rs` own.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::LEVEL_ALL;

use crate::quota_workflow::{client, default_quotas as defaults, emulator, Client};
use crate::recorded_bindings_sns_through_xray::context_json;

/// The modelled maximum length of `ServiceQuota$Description` (`@length 0..350`).
const DESCRIPTION_MAX_LENGTH: usize = 350;

/// One published default-quota row, with the members AWS publishes for it.
struct Row {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// The published sentence, where a source available here states its text; `None`
    /// pins nothing, because a wrong assertion would be worse than no assertion.
    description: Option<&'static str>,
    /// `(PeriodValue, PeriodUnit)` of a quota published over a window.
    period: Option<(i32, &'static str)>,
    /// `(MetricName, Service, Resource, Type)` of the `AWS/Usage` series the quota's
    /// usage is published in, for a metered quota.
    metric: Option<(&'static str, &'static str, &'static str, &'static str)>,
    /// `(ContextScopeType, AdjustableAtLevel)` of a quota published as a quota OF A
    /// RESOURCE; `None` for one managed for the account, which publishes no context.
    resource_plane: Option<(&'static str, &'static str)>,
}

/// A row managed for the account, counted per Region, with no context of its own: the
/// shape most of these rows have, so only the rows that differ say so.
const fn row(
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
) -> Row {
    Row {
        service_code,
        quota_code,
        quota_name,
        value,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
        resource_plane: None,
    }
}

/// The rows this suite pins: the quota each of these codes publishes.
fn rows() -> Vec<Row> {
    vec![
        // access-analyzer — the CloudTrail history one policy generation may read: a
        // SIZE in gigabytes, published as fixed, with the sentence its own code carries.
        Row {
            unit: "Gigabytes",
            adjustable: false,
            description: Some("The maximum size of CloudTrail data per policy generation."),
            ..row(
                "access-analyzer",
                "L-94099EDD",
                "Policy generation CloudTrail data size",
                25.0,
            )
        },
        // A row of the same catalog this change did not touch, so the rule is asserted
        // past the rows that motivated it: a quota measured in DAYS.
        Row {
            adjustable: false,
            description: Some(
                "The maximum CloudTrail time range that you can select in days when you \
                 generate a policy.",
            ),
            ..row(
                "access-analyzer",
                "L-96F55078",
                "Policy generation CloudTrail time range",
                90.0,
            )
        },
        // acm — the ACME domain-validation rate: the per-API class, metered with that
        // one operation's call count over the window the rate is published over.
        Row {
            description: Some(
                "The maximum number of CreateAcmeDomainValidation API requests per second.",
            ),
            period: Some((1, "SECOND")),
            metric: Some((
                "CallCount",
                "Certificate Manager",
                "CreateAcmeDomainValidation",
                "API",
            )),
            ..row(
                "acm",
                "L-74BB4A99",
                "Rate of CreateAcmeDomainValidation API requests",
                50.0,
            )
        },
        // acm-pca — the one row of this service's rate class AWS publishes as
        // ADJUSTABLE, beside a fixed row of the same class at a rate of its own.
        Row {
            description: Some(
                "The maximum number of IssueCertificate requests that can be performed in an \
                 account in the current Region each second.",
            ),
            period: Some((1, "SECOND")),
            ..row("acm-pca", "L-CAFB4993", "Rate of IssueCertificate requests", 25.0)
        },
        Row {
            adjustable: false,
            description: Some(
                "The maximum number of ImportCertificateAuthorityCertificate requests that can \
                 be performed in an account in the current Region each second.",
            ),
            period: Some((1, "SECOND")),
            ..row(
                "acm-pca",
                "L-D0C21409",
                "Rate of ImportCertificateAuthorityCertificate requests",
                10.0,
            )
        },
        // The revoked certificates of ONE CA with complete CRLs enabled: a COUNT of
        // six figures rather than a rate, so no window and no rate sentence — and a row
        // whose own published text this evidence does not state, so it carries none.
        Row {
            adjustable: false,
            ..row(
                "acm-pca",
                "L-DFF54388",
                "Number of revoked private certificates for each CA with complete CRLs enabled",
                1_000_000.0,
            )
        },
        // aco-automation — the throttle class, whose phrase precedes the operation it
        // throttles, at the rate the class is published at.
        Row {
            adjustable: false,
            description: Some(
                "Maximum number of DeleteAutomationRule requests you can make per second per \
                 account in the current region.",
            ),
            period: Some((1, "SECOND")),
            ..row(
                "aco-automation",
                "L-AD6FFAA3",
                "Throttle rate limit for DeleteAutomationRule",
                5.0,
            )
        },
        Row {
            adjustable: false,
            description: Some(
                "Maximum number of ListAutomationEventSummaries requests you can make per \
                 second per account in the current region.",
            ),
            period: Some((1, "SECOND")),
            ..row(
                "aco-automation",
                "L-7EC2BBE4",
                "Throttle rate limit for ListAutomationEventSummaries",
                5.0,
            )
        },
        Row {
            adjustable: false,
            description: Some(
                "Maximum number of UpdateEnrollmentConfiguration requests you can make per \
                 second per account in the current region.",
            ),
            period: Some((1, "SECOND")),
            ..row(
                "aco-automation",
                "L-AD63CC14",
                "Throttle rate limit for UpdateEnrollmentConfiguration",
                5.0,
            )
        },
        Row {
            adjustable: false,
            description: Some(
                "Maximum number of ListRecommendedActionSummaries requests you can make per \
                 second per account in the current region.",
            ),
            period: Some((1, "SECOND")),
            ..row(
                "aco-automation",
                "L-B314103C",
                "Throttle rate limit for ListRecommendedActionSummaries",
                5.0,
            )
        },
        // agent-registry — the REGISTRY plane of an account: a resource count, so it is
        // metered by that plane's own series over the five-minute window those counts
        // are published over, with the sentence its code carries.
        Row {
            description: Some(
                "The maximum number of registries that can be created per account per Region",
            ),
            period: Some((5, "MINUTE")),
            metric: Some(("ResourceCount", "Agent Registry", "Registry", "Resource")),
            ..row("agent-registry", "L-197AC8CB", "Registries per Account", 10.0)
        },
        // A rate row of the same service: counted in `Count/Second`, over the one-second
        // window, metered with the operation's call count, and published as fixed.
        Row {
            unit: "Count/Second",
            adjustable: false,
            description: Some(
                "The maximum number of ListRegistryRecords API requests per second per account \
                 per Region",
            ),
            period: Some((1, "SECOND")),
            metric: Some((
                "CallCount",
                "Agent Registry",
                "ListRegistryRecords",
                "API",
            )),
            ..row(
                "agent-registry",
                "L-2ECDF0F2",
                "ListRegistryRecords requests per second",
                10.0,
            )
        },
        // aidevops — the planes this service names with the slug of what it counts: the
        // investigations one agent space may run at once, and the memory planes, each
        // with the sentence its own code carries.
        Row {
            unit: "Count",
            ..row(
                "aidevops",
                "L-1553F789",
                "concurrent-investigations-per-agent-space",
                3.0,
            )
        },
        Row {
            unit: "Count",
            description: Some("Concurrent on-demand agent invocations per agent space"),
            ..row(
                "aidevops",
                "L-3D8A06B8",
                "concurrent-on-demand-invocations-per-agent-space",
                10.0,
            )
        },
        Row {
            unit: "Count",
            adjustable: false,
            description: Some(
                "The maximum number of memory stores that can exist in a single agent space.",
            ),
            ..row("aidevops", "L-D774F842", "memory-stores-per-agent-space", 50.0)
        },
        Row {
            unit: "Count",
            adjustable: false,
            description: Some(
                "The maximum number of individual memory files within a single memory store.",
            ),
            ..row("aidevops", "L-F1DF5F94", "memories-per-memory-store", 200.0)
        },
        // airflow — the LARGER-environment plane, whose sentence names the environment
        // classes it counts rather than reading the plane's own name as a phrase.
        Row {
            description: Some(
                "The maximum number of mw1.xlarge and mw1.2xlarge environments per account per \
                 Region.",
            ),
            ..row(
                "airflow",
                "L-4BA0CB82",
                "Larger environments per account per Region (xlarge, 2xlarge)",
                5.0,
            )
        },
        // amplify — the hourly app-creation rate and the cache size, each with the
        // sentence its own code carries, beside the subdomain plane of a domain (the
        // counted-plane class's sentence around its own name).
        Row {
            adjustable: false,
            description: Some(
                "The maximum number of apps that you can create in AWS Amplify Console per hour \
                 in this account in the current Region.",
            ),
            ..row("amplify", "L-2FC3A2FA", "Maximum app creations per hour", 25.0)
        },
        Row {
            unit: "Gigabytes",
            adjustable: false,
            description: Some("The maximum size (in GB) of the environment cache artifact."),
            ..row("amplify", "L-A0FC4951", "Environment cache artifact size", 5.0)
        },
        Row {
            adjustable: false,
            description: Some(
                "The maximum number of subdomains per domain that you can create in this \
                 account in the current Region.",
            ),
            ..row("amplify", "L-85685B2E", "Subdomains per domain", 50.0)
        },
        // The request tokens ONE APP serves per second: published as a quota OF the app,
        // so its records name that plane and the resource type it applies to.
        Row {
            resource_plane: Some(("AWS::Amplify::App", LEVEL_ALL)),
            ..row("amplify", "L-CE88B60E", "Request tokens per second", 20_000.0)
        },
        Row {
            unit: "Gigabytes",
            adjustable: false,
            ..row("amplify", "L-D95E5509", "Manual deploy ZIP file size", 5.0)
        },
        // A row of the same catalog this change did not touch: the branch plane, fixed
        // and with its whole class's sentence.
        Row {
            adjustable: false,
            description: Some(
                "The maximum number of branches per app that you can create in this account in \
                 the current Region.",
            ),
            ..row("amplify", "L-A6716586", "Branches per app", 50.0)
        },
    ]
}

/// The service codes these rows belong to, each read once.
async fn catalogs(caller: &Client) -> BTreeMap<&'static str, Vec<Value>> {
    let mut catalogs = BTreeMap::new();
    for row in rows() {
        if !catalogs.contains_key(row.service_code) {
            let listed = defaults(caller, row.service_code).await;
            assert!(
                !listed.is_empty(),
                "case {}: the service publishes a default-quota catalog, got none",
                row.service_code
            );
            catalogs.insert(row.service_code, listed);
        }
    }
    catalogs
}

/// The table spans the classes the rule generalises over, so a row cannot be dropped
/// from it without the suite saying so.
#[test]
fn the_table_spans_the_published_row_classes() {
    let table = rows();
    let services: Vec<&str> = {
        let mut seen: Vec<&str> = table.iter().map(|row| row.service_code).collect();
        seen.sort_unstable();
        seen.dedup();
        seen
    };
    let units: Vec<&str> = {
        let mut seen: Vec<&str> = table.iter().map(|row| row.unit).collect();
        seen.sort_unstable();
        seen.dedup();
        seen
    };

    assert!(
        services.len() >= 8
            && units.len() >= 3
            && table.iter().any(|row| row.adjustable)
            && table.iter().any(|row| !row.adjustable)
            && table.iter().any(|row| row.description.is_some())
            && table.iter().any(|row| row.description.is_none())
            && table.iter().any(|row| row.period == Some((1, "SECOND")))
            && table.iter().any(|row| row.period == Some((5, "MINUTE")))
            && table.iter().any(|row| row.period.is_none())
            && table
                .iter()
                .any(|row| matches!(row.metric, Some(("CallCount", ..))))
            && table
                .iter()
                .any(|row| matches!(row.metric, Some(("ResourceCount", ..))))
            && table.iter().any(|row| row.resource_plane.is_some())
            && table.iter().any(|row| row.value >= 1_000_000.0)
            && table.iter().any(|row| row.value <= 5.0),
        "case table: expected the table to span the row classes, got {} services and {} units \
         over {} rows",
        services.len(),
        units.len(),
        table.len()
    );
}

/// RULE: the row served under a code carries that quota's own published facts — the
/// `Description` and the `QuotaContext` included — identically through the single read
/// and through the listing.
#[tokio::test]
async fn a_code_is_served_with_the_facts_aws_publishes_under_it() {
    let engine = emulator();
    let region = "us-west-2";
    let caller = client(&engine, region);
    let catalogs = catalogs(&caller).await;

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
        match row.description {
            Some(description) => assert_eq!(
                quota["Description"],
                json!(description),
                "case {case}: a description is the quota's own published text, serialized on \
                 the row, got {quota}"
            ),
            None => assert!(
                quota.get("Description").is_none_or(Value::is_null),
                "case {case}: a row whose published text no source states carries none, got \
                 {quota}"
            ),
        }
        match row.period {
            Some((value, unit)) => assert_eq!(
                quota["Period"],
                json!({"PeriodValue": value, "PeriodUnit": unit}),
                "case {case}: expected the window AWS publishes the quota over, got {quota}"
            ),
            None => assert!(
                quota.get("Period").is_none_or(Value::is_null),
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
            None => assert!(
                quota.get("UsageMetric").is_none_or(Value::is_null),
                "case {case}: a quota CloudWatch publishes no usage for reports no metric, got \
                 {quota}"
            ),
        }
        assert_eq!(
            quota
                .get("QuotaContext")
                .cloned()
                .unwrap_or(Value::Null),
            context_json(row.resource_plane),
            "case {case}: a quota counted on a resource names that plane, and one managed for \
             the account names none, got {quota}"
        );

        // The listing carries exactly the record the single read returned.
        let listed = &catalogs[row.service_code];
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );
    }
}

/// RULE: a `Description` is the sentence the service publishes for ONE of its quotas —
/// serialized on the row, within the modelled length, and carried by no second row of
/// the same catalog (a sentence appearing twice would mean one row describes the other's
/// quota).
#[tokio::test]
async fn a_published_description_belongs_to_one_row_of_its_catalog() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

    let mut described = 0_usize;
    for (service_code, listed) in catalogs(&caller).await {
        let mut by_description: BTreeMap<String, String> = BTreeMap::new();
        for quota in &listed {
            let code = quota["QuotaCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case {service_code}: a row without a code: {quota}"))
                .to_string();
            let case = format!("{service_code}/{code}");
            let Some(description) = quota["Description"].as_str() else {
                continue;
            };
            assert!(
                (1..=DESCRIPTION_MAX_LENGTH).contains(&description.chars().count()),
                "case {case}: a published sentence sits within the modelled length, got \
                 {description:?}"
            );
            if let Some(first) = by_description.insert(description.to_string(), code.clone()) {
                panic!(
                    "case {case}: a published description belongs to one quota, got \
                     {description:?} under {code} and under {first}"
                );
            }
            described += 1;
        }
    }

    // The described rows of these catalogs, so a change that drops the member again
    // fails here rather than passing a vacuous walk.
    assert!(
        described >= rows().iter().filter(|row| row.description.is_some()).count(),
        "case catalog: expected every described row of these catalogs to carry its sentence, \
         got {described}"
    );
}

/// RULE: `Adjustable` is backed by the write path — a quota published as fixed refuses
/// an increase request, an adjustable one takes it, at a value above the one in force.
#[tokio::test]
async fn adjustability_decides_whether_an_increase_is_accepted() {
    let engine = emulator();
    let caller = client(&engine, "ap-northeast-1");

    for row in rows() {
        let case = format!("{}/{}", row.service_code, row.quota_code);
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
