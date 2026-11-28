// SPDX-License-Identifier: CC0-1.0

//! Which quota the default-quota catalogs of `apigateway`, `appconfig`,
//! `application-autoscaling`, `athena` and `glue` publish under which `QuotaCode`.
//!
//! THE RULE, in the two parts the catalog's row tables implement (see the module header of
//! `src/state/quota_rows.rs`): the row published under a `QuotaCode` carries the
//! `QuotaName`, `Value`, `Unit`, `Adjustable`, `Period`, `UsageMetric` and `Description` of
//! the quota that service publishes UNDER THAT CODE — worded as the service words it, never
//! a paraphrase and never a neighbouring quota's — and a plane whose own code no source
//! states is published under NO code of its own: either under a code whose quota no source
//! states, when a sourced binding displaced it onto one that code vacated, or under no row
//! at all. Both halves are asserted here, beside the rule that a code publishes exactly one
//! row, which resolves through every read that takes a `QuotaCode`.
//!
//! Two rows of the table are of that second kind — `apigateway` `L-A966AB5C` and `athena`
//! `L-D0D8C912`, the codes the WebSocket-payload and `ListNamedQueries` bindings vacated.
//! Pinning them is not a claim that AWS publishes those planes under those codes (the row
//! tables mark them as exactly that): it pins that the displaced plane stays reachable under
//! one code rather than vanishing or appearing twice, and it is replaced the moment the
//! quota either code publishes can be sourced.
//!
//! SOURCE for every expectation: AWS's published Service Quotas data for that service code,
//! and the recorded `ListAWSDefaultServiceQuotas` response of that code for which
//! `QuotaCode` publishes which quota and for the text, window, metric and adjustability of
//! the rows it states. The rows below span what the rule generalises over rather than
//! repeating one recorded response: five service codes, counted planes beside per-API rates
//! and a size, `Unit` `None` beside kilobytes, adjustable rows beside fixed ones, values
//! from 10 to 1,000,000, rows published over a one-second window beside rows published over
//! none, an `AWS/Usage` `ResourceCount` series beside `CallCount` series of two different
//! APIs, a row metered with NO window of its own, and rows published with a sentence beside
//! rows whose published sentence no source available here states, which carry none.
//!
//! Each row is read BOTH through `ListAWSDefaultServiceQuotas` and through
//! `GetAWSDefaultServiceQuota`, because the code is the only key a single read takes. The
//! harness (`client`, `emulator`, `default_quotas`) is the one `quota_workflow.rs` owns.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::quota_workflow::{
    call_count, client, default_quotas, emulator, resource_count, Client, Metric,
};

/// The `ServiceCode`s whose whole catalogs the per-catalog invariants are walked over.
const SERVICE_CODES: &[&str] = &[
    "apigateway",
    "appconfig",
    "application-autoscaling",
    "athena",
    "glue",
];

/// One published default-quota row.
struct Row {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// The published sentence, where a source states its text; `None` pins that the row
    /// publishes none, which is what a row whose own sentence no source states carries.
    description: Option<&'static str>,
    /// `(PeriodValue, PeriodUnit)` of a quota published over a window.
    period: Option<(i32, &'static str)>,
    /// The series usage of the quota is published under, for a metered row.
    metric: Option<Metric>,
}

/// The sentence Application Auto Scaling publishes for a scalable-target row, which names
/// the ONE namespace that row's targets are registered under.
macro_rules! scalable_targets_sentence {
    ($namespace:literal) => {
        concat!(
            "The maximum number of scalable targets that you can register for the ",
            $namespace,
            " namespace in this account in the current Region. A scalable target \
             identifies the resource that Application Auto Scaling can scale."
        )
    };
}

/// The rows this suite pins.
const ROWS: &[Row] = &[
    // apigateway — the size a resource policy may reach, the developer portals of an
    // account, the subnets of a V2 VPC link and the WebSocket message payload size, under
    // the codes the recorded listing binds them to, beside the per-API tag plane the last of
    // those bindings displaced onto the code it vacated.
    Row {
        service_code: "apigateway",
        quota_code: "L-8B81B02C",
        quota_name: "Maximum resource policy size in bytes",
        value: 8192.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum resource policy size in bytes you can have in this account in the \
             current region.",
        ),
        period: None,
        metric: None,
    },
    Row {
        service_code: "apigateway",
        quota_code: "L-F8BD84D3",
        quota_name: "Portals per account",
        value: 15.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Row {
        service_code: "apigateway",
        quota_code: "L-668C9B28",
        quota_name: "Subnets per VPC link(V2)",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Row {
        service_code: "apigateway",
        quota_code: "L-FD0EB744",
        quota_name: "WebSocket message payload size",
        value: 128.0,
        unit: "Kilobytes",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    Row {
        service_code: "apigateway",
        quota_code: "L-A966AB5C",
        quota_name: "Tags per API",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    // appconfig — the size limit of ONE deployment, at the limit the recorded listing states
    // for the code and in the unit this service publishes its size planes in.
    Row {
        service_code: "appconfig",
        quota_code: "L-A5FC0339",
        quota_name: "Deployment size limit",
        value: 2048.0,
        unit: "Kilobytes",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    // application-autoscaling — the scalable-target planes whose scaled service the
    // recorded listing names, each described for the namespace ITS targets are registered
    // under (which AWS publishes under a different name for two of them), and the two planes
    // it publishes above the class's limit.
    Row {
        service_code: "application-autoscaling",
        quota_code: "L-800F6F7B",
        quota_name: "Scalable targets for RDS",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: Some(scalable_targets_sentence!("RDS")),
        period: None,
        metric: None,
    },
    Row {
        service_code: "application-autoscaling",
        quota_code: "L-1A11EB4B",
        quota_name: "Scalable targets for Amazon MSK",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: Some(scalable_targets_sentence!("Kafka")),
        period: None,
        metric: None,
    },
    Row {
        service_code: "application-autoscaling",
        quota_code: "L-60E4E5E2",
        quota_name: "Scalable targets for Amazon ElastiCache",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: Some(scalable_targets_sentence!("ElastiCache")),
        period: None,
        metric: None,
    },
    Row {
        service_code: "application-autoscaling",
        quota_code: "L-782A3EE2",
        quota_name: "Scalable targets for ECS",
        value: 3000.0,
        unit: "None",
        adjustable: true,
        description: Some(scalable_targets_sentence!("ECS")),
        period: None,
        metric: None,
    },
    Row {
        service_code: "application-autoscaling",
        quota_code: "L-A1D42901",
        quota_name: "Scalable targets for DynamoDB",
        value: 5000.0,
        unit: "None",
        adjustable: true,
        description: Some(scalable_targets_sentence!("DynamoDB")),
        period: None,
        metric: None,
    },
    // athena — the WORKGROUP plane of an account, metered by the service's resource count
    // over no window of its own, beside the three per-API replenishment rates: the one the
    // recorded listing binds to the code the workgroup plane's neighbour had carried, the
    // rate displaced onto the code that binding vacated, and the rate whose own burst
    // multiplier is now published under no code.
    Row {
        service_code: "athena",
        quota_code: "L-FD9D80C2",
        quota_name: "Maximum number of workgroups per account",
        value: 1000.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: Some(resource_count("Athena", "WorkGroup")),
    },
    Row {
        service_code: "athena",
        quota_code: "L-82E47245",
        quota_name: "Replenishment rate of ListNamedQueries API calls",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: Some((1, "SECOND")),
        metric: Some(call_count("Athena", "ListNamedQueries")),
    },
    Row {
        service_code: "athena",
        quota_code: "L-D0D8C912",
        quota_name: "Replenishment rate of GetQueryExecution API calls",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: Some((1, "SECOND")),
        metric: Some(call_count("Athena", "GetQueryExecution")),
    },
    Row {
        service_code: "athena",
        quota_code: "L-FC5F6546",
        quota_name: "Replenishment rate of StopQueryExecution API calls",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: Some((1, "SECOND")),
        metric: Some(call_count("Athena", "StopQueryExecution")),
    },
    // glue — the security configurations and queued job runs of an account and the Data
    // Catalog's connection types, under the codes the recorded listing binds them to, beside
    // the two rows whose sentence that listing words for the Region and the account rather
    // than in this catalog's possessive voice.
    Row {
        service_code: "glue",
        quota_code: "L-83192DBF",
        quota_name: "Max security configurations per account",
        value: 250.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of security configurations in your account."),
        period: None,
        metric: None,
    },
    Row {
        service_code: "glue",
        quota_code: "L-A457CBAB",
        quota_name: "Maximum number of queued job runs",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: Some("Orchestration: maximum number of queued job runs"),
        period: None,
        metric: None,
    },
    Row {
        service_code: "glue",
        quota_code: "L-F545C262",
        quota_name: "Number of connection types",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of connection types per AWS Region for this account.",
        ),
        period: None,
        metric: None,
    },
    Row {
        service_code: "glue",
        quota_code: "L-F1AAB534",
        quota_name: "Number of integrations",
        value: 40.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of integrations per AWS Region for this account."),
        period: None,
        metric: None,
    },
    Row {
        service_code: "glue",
        quota_code: "L-87F5991C",
        quota_name: "Max data quality rulesets per account",
        value: 1_000_000.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of data quality rulesets per account."),
        period: None,
        metric: None,
    },
];

/// The planes DISPLACED by the bindings above that no source available here states a
/// `QuotaCode` for: each is published under no code of its service at all, by the
/// `QuotaName` it had been served as, rather than minted a code AWS never assigned.
const PLANES_UNDER_NO_CODE: &[(&str, &str)] = &[
    ("apigateway", "Throttle rate"),
    ("apigateway", "Integrations per API"),
    ("apigateway", "Custom domain names per account per Region"),
    ("application-autoscaling", "Scalable targets for Neptune"),
    ("application-autoscaling", "Scalable targets for Aurora"),
    ("athena", "Burst multiplier quota for StopQueryExecution API"),
    ("glue", "Rate of CreateTable requests"),
    ("glue", "Rate of StartWorkflowRun requests"),
];

/// One service's catalog, keyed by `QuotaCode`, asserting each code is published once.
async fn catalog_by_code(caller: &Client, service_code: &str) -> BTreeMap<String, Value> {
    let listed = default_quotas(caller, service_code).await;
    assert!(
        !listed.is_empty(),
        "case {service_code}: the service publishes a default-quota catalog, got none"
    );
    let mut by_code: BTreeMap<String, Value> = BTreeMap::new();
    for quota in listed {
        let code = quota["QuotaCode"]
            .as_str()
            .unwrap_or_else(|| panic!("case {service_code}: a row without a code: {quota}"))
            .to_string();
        if let Some(first) = by_code.insert(code.clone(), quota.clone()) {
            panic!("case {service_code}/{code}: a code is published once, got {first} and {quota}");
        }
    }
    by_code
}

/// Every pinned row carries the facts its service publishes under its own code, through the
/// listing and through the single read alike.
#[tokio::test]
async fn a_rebound_row_carries_the_facts_published_under_its_code() {
    let engine = emulator();
    let region = "eu-west-1";
    let caller = client(&engine, region);

    let mut catalogs: BTreeMap<&str, BTreeMap<String, Value>> = BTreeMap::new();
    for service_code in SERVICE_CODES {
        catalogs.insert(service_code, catalog_by_code(&caller, service_code).await);
    }

    for row in ROWS {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let listed = catalogs[row.service_code]
            .get(row.quota_code)
            .unwrap_or_else(|| panic!("case {case}: expected the code to be published"));
        let read = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();
        assert_eq!(
            &read, listed,
            "case {case}: the listing carries the record the single read returns, got {read} \
             and {listed}"
        );

        assert_eq!(
            (
                read["QuotaName"].clone(),
                read["Value"].clone(),
                read["Unit"].clone(),
                read["Adjustable"].clone(),
            ),
            (
                json!(row.quota_name),
                json!(row.value),
                json!(row.unit),
                json!(row.adjustable),
            ),
            "case {case}: expected the row this catalog publishes under this code, got {read}"
        );
        assert_eq!(
            read["Description"],
            match row.description {
                Some(sentence) => json!(sentence),
                None => Value::Null,
            },
            "case {case}: expected the sentence published under this code, got {read}"
        );
        match row.period {
            Some((value, unit)) => assert_eq!(
                read["Period"],
                json!({"PeriodValue": value, "PeriodUnit": unit}),
                "case {case}: expected the published window, got {read}"
            ),
            None => assert!(
                read.get("Period").is_none_or(Value::is_null),
                "case {case}: this quota is published over no window, got {read}"
            ),
        }
        match &row.metric {
            Some(metric) => assert_eq!(
                read["UsageMetric"],
                metric.published(),
                "case {case}: expected the series usage of this quota is published under, got \
                 {read}"
            ),
            None => assert!(
                read.get("UsageMetric").is_none_or(Value::is_null),
                "case {case}: this quota publishes no usage metric, got {read}"
            ),
        }

        // The applied plane is keyed on the same code, so a client that read a code out of
        // the default catalog resolves its value in force under the same quota.
        let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
        assert_eq!(
            (
                applied["QuotaName"].clone(),
                applied["Value"].clone(),
                applied["Description"].clone(),
            ),
            (
                json!(row.quota_name),
                json!(row.value),
                read["Description"].clone(),
            ),
            "case {case}: the applied read resolves the same quota at its default value, got \
             {applied}"
        );
    }
}

/// A plane whose own `QuotaCode` no source states is published under NO code: its name
/// appears nowhere in its service's catalog, and no two rows of one catalog share a name.
#[tokio::test]
async fn a_plane_with_no_published_code_is_published_under_no_row() {
    let engine = emulator();
    let caller = client(&engine, "us-east-2");

    for service_code in SERVICE_CODES {
        let catalog = catalog_by_code(&caller, service_code).await;

        for (plane_service, name) in PLANES_UNDER_NO_CODE {
            if plane_service != service_code {
                continue;
            }
            let carried: Vec<&String> = catalog
                .iter()
                .filter(|(_, quota)| quota["QuotaName"] == json!(name))
                .map(|(code, _)| code)
                .collect();
            assert!(
                carried.is_empty(),
                "case {service_code}/{name:?}: a quota whose own code no source states is \
                 published under no code, got it under {carried:?}"
            );
        }

        // A name belongs to one quota of a service, so a row that borrowed a neighbour's
        // would show up here.
        let mut by_name: BTreeMap<&str, &String> = BTreeMap::new();
        for (code, quota) in &catalog {
            let name = quota["QuotaName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {service_code}/{code}: a row without a name"));
            if let Some(first) = by_name.insert(name, code) {
                panic!(
                    "case {service_code}/{code}: a published name belongs to one quota, got \
                     {name:?} under {code} and under {first}"
                );
            }
        }
    }
}

/// Every code of these catalogs is a key a client can act on: it resolves the same record on
/// the default plane and the applied plane, and an adjustable row is accepted by the
/// increase request that names it.
#[tokio::test]
async fn every_published_code_of_these_catalogs_resolves_its_own_row() {
    let engine = emulator();
    let caller = client(&engine, "ap-south-1");

    let mut covered: BTreeSet<&str> = BTreeSet::new();
    for service_code in SERVICE_CODES {
        let catalog = catalog_by_code(&caller, service_code).await;
        for (code, quota) in &catalog {
            let case = format!("{service_code}/{code}");
            let pair = json!({"ServiceCode": service_code, "QuotaCode": code});

            let default = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"]
                .clone();
            assert_eq!(
                &default, quota,
                "case {case}: the published code must resolve the listed row, got {default}"
            );

            let applied = caller.ok("GetServiceQuota", pair.clone()).await["Quota"].clone();
            assert_eq!(
                (
                    applied["QuotaName"].clone(),
                    applied["Value"].clone(),
                    applied["Description"].clone(),
                ),
                (
                    quota["QuotaName"].clone(),
                    quota["Value"].clone(),
                    quota["Description"].clone(),
                ),
                "case {case}: the applied plane must report the same catalog row, got {applied}"
            );

            let pinned = ROWS
                .iter()
                .any(|row| row.service_code == *service_code && row.quota_code == code.as_str());

            // The increase path is driven for the PINNED rows only: a code is a key the
            // write takes, which the rows this change rebinds are what has to show, and
            // asking for an increase on every code of five whole catalogs would spend
            // hundreds of round trips on an invariant the per-catalog suites already hold.
            if pinned && quota["Adjustable"] == json!(true) {
                let mut body = pair;
                body["DesiredValue"] = json!(quota["Value"].as_f64().unwrap_or_default() + 1.0);
                let requested = caller.ok("RequestServiceQuotaIncrease", body).await
                    ["RequestedQuota"]
                    .clone();
                assert_eq!(
                    (requested["QuotaCode"].clone(), requested["QuotaName"].clone()),
                    (json!(code), quota["QuotaName"].clone()),
                    "case {case}: the request must be recorded against the quota asked for, got \
                     {requested}"
                );
            }

            if pinned {
                covered.insert(service_code);
            }
        }
    }

    // Each of the five catalogs carries at least one of the pinned rows, so the table above
    // spans the services this rule was asserted over rather than one of them.
    assert_eq!(
        covered.len(),
        SERVICE_CODES.len(),
        "case catalog: expected a pinned row in each covered catalog, got {covered:?}"
    );
}
