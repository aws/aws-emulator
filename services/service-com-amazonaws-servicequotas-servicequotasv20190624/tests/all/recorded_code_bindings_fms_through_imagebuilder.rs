// SPDX-License-Identifier: CC0-1.0

//! Which quota each of these default-quota catalogs publishes under which `QuotaCode`, for
//! the codes a recorded `ListAWSDefaultServiceQuotas` listing states the quota of.
//!
//! THE RULE: for a `ServiceCode`, the row published under a `QuotaCode` carries the
//! `QuotaName`, `Value`, `Unit`, `Adjustable`, `GlobalQuota`, `Period`, `UsageMetric`,
//! `QuotaContext` and `Description` of the quota that service publishes UNDER THAT CODE —
//! the sentence as the service words it, never a paraphrase and never a neighbouring
//! quota's — and a quota whose own code no source states is published under NO code rather
//! than under another quota's. `QuotaCode` is the only key `GetAWSDefaultServiceQuota`,
//! `GetServiceQuota`, `RequestServiceQuotaIncrease` and
//! `ListRequestedServiceQuotaChangeHistoryByQuota` take, so a code bound to a neighbour's
//! row hands a client another quota's limit, adjustability and usage metric. Smithy fixes
//! only the row's SHAPE (`smithy_query(query="shape", shape="ServiceQuota")`), so which
//! quota a code publishes is recorded behaviour.
//!
//! The table spans what the rule generalises over rather than repeating one row: five
//! `ServiceCode`s; counted account planes, a plane counted per POLICY, a plane counted per
//! REGION, capacity planes in GiB and in gigabytes, an IOPS total, and two character
//! LENGTHS; `Unit` `None` beside `Gigabytes`; adjustable rows beside a not-adjustable one;
//! a sentence the service publishes with two spaces in it, one it publishes without a
//! closing period, and rows whose own sentence no source states, which therefore carry
//! none; and limits from 3 to 500,000. Each row is read through the DEFAULT listing,
//! through `GetAWSDefaultServiceQuota` and through the APPLIED single read, because the
//! code is what a client acts on.
//!
//! The `events` rows this change rebound carry their names, limits and sentences in
//! `aws_quota_metadata_per_code.rs` and
//! `rebound_rows_events_fms_fsx_gamelift_imagebuilder.rs`, which own them; the members that
//! tell those version 2 planes apart from the version 1 plane the catalog had served under
//! the same code — the resource type they are COUNTED on and the `AWS/Usage` series they are
//! metered by — are asserted by the last test here, and the consequence asserted over WHOLE
//! catalogs below covers all six `ServiceCode`s this change touched.
//!
//! The harness (`client`, `emulator`, `default_quotas_by_code`, `resource_count`) is the
//! one `quota_workflow.rs` owns.

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas_by_code, emulator, resource_count, Metric};

/// One published row: the quota a `QuotaCode` of that service publishes.
struct Row {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// The published sentence, where the recorded listing states its text; `None` pins that
    /// the row carries NO description rather than one this emulator words itself.
    description: Option<&'static str>,
    /// `(PeriodValue, PeriodUnit)` for a row published over a window.
    period: Option<(i32, &'static str)>,
    /// The `AWS/Usage` series the row is metered by, for a metered plane.
    metric: Option<Metric>,
}

/// The rows this suite owns.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of each of these service
/// codes, as the project's issue record quotes it, states which quota the code publishes
/// and the text of the members pinned here. Where it states the quota but not one of its
/// members, that member is taken from the class the service publishes the quota in (the
/// per-policy counts of `fms`, the per-deployment capacity planes of `fsx`, the count
/// planes of `imagebuilder`) or — where the class does not reach it — kept as the row has
/// carried it, which is why two rows below carry a limit and none a sentence.
const ROWS: &[Row] = &[
    // fms — three policy-CAPACITY planes where per-API request rates had stood: the rules
    // one network ACL policy may use, the primary security groups one common security group
    // policy may use, and the VPCs one Network Firewall policy may remediate (published as
    // NOT adjustable, and the one row here whose sentence no source states).
    Row {
        service_code: "fms",
        quota_code: "L-7AB3A351",
        quota_name: "Inbound/outbound rules per network ACL policy",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of inbound/outbound rules that you can use in a Firewall \
             Manager Network ACL policy.",
        ),
        period: None,
        metric: None,
    },
    Row {
        service_code: "fms",
        quota_code: "L-72AC804F",
        quota_name: "Primary security groups per common security group policy",
        value: 3.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of primary security groups that you can use in a Firewall \
             Manager common security group policy.",
        ),
        period: None,
        metric: None,
    },
    Row {
        service_code: "fms",
        quota_code: "L-BCA8AD34",
        quota_name: "VPCs that a single Network Firewall policy can automatically remediate",
        value: 1000.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    // fsx — a COUNT of caches and a CAPACITY in GiB, each where a per-file-system or
    // per-volume count of another family had stood.
    Row {
        service_code: "fsx",
        quota_code: "L-60836D3E",
        quota_name: "Lustre Cache_1 caches",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of Amazon File Cache caches with cache type Lustre and \
             deployment type Cache_1 that you can create in this account.",
        ),
        period: None,
        metric: None,
    },
    Row {
        service_code: "fsx",
        quota_code: "L-AD2FC696",
        quota_name: "Lustre Scratch storage capacity",
        value: 100_800.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum amount of storage capacity (in GiB) that you can configure for all \
             Amazon FSx for Lustre scratch file systems in this account.",
        ),
        period: None,
        metric: None,
    },
    // gamelift — two per-Region planes where per-location and per-account hosting planes
    // had stood, a per-Region plane whose own sentence no source states, and a capacity
    // published in GIGABYTES and as not adjustable, worded by the uploaded game builds it
    // holds.
    Row {
        service_code: "gamelift",
        quota_code: "L-73F6E300",
        quota_name: "Matchmaking configurations per region",
        value: 300.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of matchmaking configurations allowed per region."),
        period: None,
        metric: None,
    },
    Row {
        service_code: "gamelift",
        quota_code: "L-293B0017",
        quota_name: "Scripts per region",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of game server scripts allowed per region."),
        period: None,
        metric: None,
    },
    Row {
        service_code: "gamelift",
        quota_code: "L-55650DB7",
        quota_name: "Locations in a fleet per region",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Row {
        service_code: "gamelift",
        quota_code: "L-11948650",
        quota_name: "Build capacity",
        value: 100.0,
        unit: "Gigabytes",
        adjustable: false,
        description: Some(
            "The maximum capacity (in gigabytes) available for all uploaded game builds \
             combined per region. To free storage space, delete unused builds as needed.",
        ),
        period: None,
        metric: None,
    },
    // glue — three counted account planes where per-API request rates had stood, and one
    // whose sentence the service publishes WITHOUT a closing period and in the singular.
    Row {
        service_code: "glue",
        quota_code: "L-C3952F6C",
        quota_name: "Max concurrent data quality ruleset evaluation runs per account",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of concurrent data quality ruleset evaluation runs in your \
             account.",
        ),
        period: None,
        metric: None,
    },
    Row {
        service_code: "glue",
        quota_code: "L-F953935E",
        quota_name: "Max databases per account",
        value: 10_000.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of databases in your account."),
        period: None,
        metric: None,
    },
    Row {
        service_code: "glue",
        quota_code: "L-EAC30D24",
        quota_name: "Max Named Tag Expressions",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of named tag expression allowed per account"),
        period: None,
        metric: None,
    },
    // glue — the code whose own quota no source states: it keeps the name and limit it has
    // carried, and publishes no sentence, because the sentence that row was served with
    // belongs to the quota `L-83192DBF` publishes.
    Row {
        service_code: "glue",
        quota_code: "L-4071B0E3",
        quota_name: "Security configurations per account",
        value: 200.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    // imagebuilder — a character LENGTH where a per-second API rate had stood, so a client
    // reading this code gets a limit on one parameter value and not a request rate; and a
    // per-configuration plane whose limit no source states, which therefore keeps the one
    // its code has carried.
    Row {
        service_code: "imagebuilder",
        quota_code: "L-A26B27A7",
        quota_name: "Image workflow parameter",
        value: 1024.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of characters that an image workflow parameter value can \
             contain.",
        ),
        period: None,
        metric: None,
    },
    Row {
        service_code: "imagebuilder",
        quota_code: "L-69312229",
        quota_name: "Concurrent AMI copies per distribution configuration",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of target accounts that can be defined for a single \
             distribution configuration.",
        ),
        period: None,
        metric: None,
    },
];

/// The quotas these bindings DISPLACED, per `ServiceCode`: each is a quota one of these
/// catalogs used to publish under a code the recorded listing binds to another quota, and no
/// source available here states the code its own service publishes it under — so it is
/// published under NO code at all rather than under a neighbour's. The `UpdateTable` rate
/// the `glue` database binding displaced is among them: the code that plane vacated
/// publishes the account's concurrent job runs, which the recorded listing states for it, so
/// that rate names no code at all.
const DISPLACED: &[(&str, &[&str])] = &[
    ("events", &["Rules per event bus", "Archives"]),
    (
        "fms",
        &[
            "Rate of GetProtectionStatus requests",
            "Rate of GetAppsList requests",
            "Rate of ListComplianceStatus requests",
        ],
    ),
    (
        "fsx",
        &[
            "Lustre data repository associations per file system",
            "ONTAP backups per volume",
        ],
    ),
    (
        "gamelift",
        &[
            "Instances per location",
            "Game server groups",
            "Matchmaking configurations",
        ],
    ),
    (
        "glue",
        &[
            "Rate of GetJob requests",
            "Rate of GetTableVersions requests",
            "Rate of UpdateTable requests",
            "Databases per account",
        ],
    ),
    (
        "imagebuilder",
        &[
            "Rate of CreateImage requests",
            "Rate of CreateComponent requests",
            "Component versions",
        ],
    ),
];

/// Every pinned row is published under its own code, with the members the recorded listing
/// states for it, identically in the listing and in the code-keyed default read.
#[tokio::test]
async fn a_rebound_row_carries_the_quota_published_under_its_code() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

    let mut catalogs: std::collections::BTreeMap<&str, _> = std::collections::BTreeMap::new();
    for row in ROWS {
        if !catalogs.contains_key(row.service_code) {
            catalogs.insert(
                row.service_code,
                default_quotas_by_code(&caller, row.service_code).await,
            );
        }
    }

    for row in ROWS {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let listed = catalogs[row.service_code]
            .get(row.quota_code)
            .unwrap_or_else(|| {
                panic!("case {case}: every code the service publishes answers a row, got none")
            });

        assert_eq!(
            listed["QuotaName"],
            json!(row.quota_name),
            "case {case}: expected the name published under this code, {:?}, got {:?}",
            row.quota_name,
            listed["QuotaName"]
        );
        assert_eq!(
            listed["Value"].as_f64(),
            Some(row.value),
            "case {case}: expected the limit published under this code, {}, got {:?}",
            row.value,
            listed["Value"]
        );
        assert_eq!(
            listed["Unit"],
            json!(row.unit),
            "case {case}: expected the unit published under this code, {:?}, got {:?}",
            row.unit,
            listed["Unit"]
        );
        assert_eq!(
            listed["Adjustable"],
            json!(row.adjustable),
            "case {case}: expected Adjustable {}, got {:?}",
            row.adjustable,
            listed["Adjustable"]
        );
        assert_eq!(
            listed["GlobalQuota"],
            json!(false),
            "case {case}: these quotas are counted per Region, got {:?}",
            listed["GlobalQuota"]
        );
        match row.description {
            Some(sentence) => assert_eq!(
                listed["Description"],
                json!(sentence),
                "case {case}: expected the sentence the service publishes, {sentence:?}, got \
                 {:?}",
                listed["Description"]
            ),
            None => assert_eq!(
                listed["Description"],
                Value::Null,
                "case {case}: a row whose published sentence no source states carries none, \
                 got {:?}",
                listed["Description"]
            ),
        }
        match row.period {
            Some((value, unit)) => assert_eq!(
                listed["Period"],
                json!({"PeriodValue": value, "PeriodUnit": unit}),
                "case {case}: expected the window this quota is published over, {value} \
                 {unit}, got {:?}",
                listed["Period"]
            ),
            None => assert!(
                listed.get("Period").is_none_or(Value::is_null),
                "case {case}: a quota published over no window carries no Period, got {:?}",
                listed["Period"]
            ),
        }
        match &row.metric {
            Some(metric) => assert_eq!(
                listed["UsageMetric"],
                metric.published(),
                "case {case}: expected the series usage of this quota is published under, \
                 got {:?}",
                listed["UsageMetric"]
            ),
            None => assert!(
                listed.get("UsageMetric").is_none_or(Value::is_null),
                "case {case}: this quota publishes no usage metric, got {:?}",
                listed["UsageMetric"]
            ),
        }

        // The code-keyed single read returns exactly the record the listing carries, and the
        // applied plane resolves the same quota at its default value: the code is the key a
        // client acts on.
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let read = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();
        assert_eq!(
            &read, listed,
            "case {case}: the listing carries the record the single read returns, got {read}"
        );
        let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
        assert_eq!(
            (applied["QuotaName"].clone(), applied["Value"].as_f64()),
            (json!(row.quota_name), Some(row.value)),
            "case {case}: the applied read resolves the same quota at its default value, got \
             {applied}"
        );
    }
}

/// A quota one of these bindings displaced is published under NO code of its service, and
/// every quota a binding took is published under exactly ONE — the code the recorded listing
/// binds it to.
#[tokio::test]
async fn a_displaced_quota_is_published_under_no_code_of_its_service() {
    let engine = emulator();
    let caller = client(&engine, "us-east-2");

    for (service_code, displaced) in DISPLACED {
        let catalog = default_quotas_by_code(&caller, service_code).await;
        for name in *displaced {
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

        // Every code this catalog publishes still resolves through the code-keyed read: a
        // rebinding replaces the quota a code publishes, never the code itself.
        for (code, listed) in &catalog {
            let read = caller
                .ok(
                    "GetAWSDefaultServiceQuota",
                    json!({"ServiceCode": service_code, "QuotaCode": code}),
                )
                .await["Quota"]
                .clone();
            assert_eq!(
                &read, listed,
                "case {service_code}/{code}: every published code resolves the row the \
                 listing carries, got {read}"
            );
        }

        for row in ROWS.iter().filter(|row| row.service_code == *service_code) {
            let carrying: Vec<&String> = catalog
                .iter()
                .filter(|(_, quota)| quota["QuotaName"] == json!(row.quota_name))
                .map(|(code, _)| code)
                .collect();
            assert_eq!(
                carrying,
                vec![&row.quota_code.to_string()],
                "case {service_code}/{}: this quota is published under one code, got it \
                 under {carrying:?}",
                row.quota_code
            );
        }
    }
}

/// An increase requested by a rebound code is minted against the quota that code now
/// publishes, at the name the catalog publishes under it: the rebinding reaches the write
/// path and not only the listing.
#[tokio::test]
async fn an_increase_by_a_rebound_code_targets_the_quota_its_code_publishes() {
    let engine = emulator();
    let caller = client(&engine, "ap-south-1");

    for row in ROWS.iter().filter(|row| row.adjustable) {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let desired = row.value * 2.0;

        let mut body = pair.clone();
        body["DesiredValue"] = json!(desired);
        let requested =
            caller.ok("RequestServiceQuotaIncrease", body).await["RequestedQuota"].clone();
        assert_eq!(
            (
                requested["QuotaCode"].clone(),
                requested["QuotaName"].clone(),
                requested["DesiredValue"].as_f64(),
            ),
            (
                json!(row.quota_code),
                json!(row.quota_name),
                Some(desired)
            ),
            "case {case}: an increase must be minted against the quota published under the \
             code it names, got {requested}"
        );
    }
}

/// The version 2 planes of `events` this change re-bound: the members a per-resource
/// counted plane publishes beyond its name and limit — the resource type it is COUNTED on
/// and the `AWS/Usage` series it is metered by.
///
/// The `events` rows' names, limits and sentences are pinned by
/// `aws_quota_metadata_per_code.rs` and
/// `rebound_rows_events_fms_fsx_gamelift_imagebuilder.rs`; the `QuotaContext` and the
/// metered series are asserted here, so the pair of members that tells a version 2 plane
/// from the version 1 plane the catalog had served under the same code is covered in one
/// place. The constructors are the shared ones, so a row pinned here cannot drift from the
/// series the catalog publishes.
#[tokio::test]
async fn a_version_2_plane_is_counted_on_its_own_resource_type_and_series() {
    let engine = emulator();
    let caller = client(&engine, "us-west-2");

    // The SUBSCRIBERS of one version 2 event bus: counted ON the version 2 event-bus
    // resource type, with an increase askable for every bus of the account, and metered by
    // that plane's own resource count — not by the version 1 `Rule` series.
    let subscribers = caller
        .ok(
            "GetAWSDefaultServiceQuota",
            json!({"ServiceCode": "events", "QuotaCode": "L-39263BDC"}),
        )
        .await["Quota"]
        .clone();
    assert_eq!(
        subscribers["QuotaContext"],
        json!({
            "ContextScope": "RESOURCE",
            "ContextScopeType": "AWS::EventsV2::EventBus",
            "AdjustableAtLevel": "ALL",
        }),
        "case events/L-39263BDC: a plane counted on a resource publishes that resource \
         type, got {subscribers}"
    );
    assert_eq!(
        subscribers["UsageMetric"],
        resource_count("EventBridge", "EventsV2/SubscribersPerEventBus").published(),
        "case events/L-39263BDC: a counted plane is metered by its own resource series, got \
         {subscribers}"
    );

    // The version 2 EVENT SOURCES of the account: a count of resources in use, so it is
    // metered by that plane's own resource count over the window those counts are published
    // on — never by an API's call count — and counted for the account, so it publishes no
    // resource context at all.
    let read = caller
        .ok(
            "GetAWSDefaultServiceQuota",
            json!({"ServiceCode": "events", "QuotaCode": "L-D082A4F6"}),
        )
        .await["Quota"]
        .clone();
    assert_eq!(
        read["UsageMetric"],
        resource_count("EventBridge", "EventsV2/EventSource").published(),
        "case events/L-D082A4F6: a counted plane is metered by its own resource series, got \
         {read}"
    );
    assert_eq!(
        read["Period"],
        json!({"PeriodValue": 5, "PeriodUnit": "MINUTE"}),
        "case events/L-D082A4F6: a resource count is published over the window those counts \
         are published on, got {read}"
    );
    assert!(
        read.get("QuotaContext").is_none_or(Value::is_null),
        "case events/L-D082A4F6: a quota counted for the account publishes no resource \
         context, got {read}"
    );
}
