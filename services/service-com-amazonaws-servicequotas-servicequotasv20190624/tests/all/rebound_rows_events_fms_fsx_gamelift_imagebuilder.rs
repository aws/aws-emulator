// SPDX-License-Identifier: CC0-1.0

//! The rows these five catalogs publish under the `QuotaCode`s a recorded
//! `ListAWSDefaultServiceQuotas` listing states the quota for.
//!
//! THE RULE: the row a service's default-quota catalog publishes under a `QuotaCode`
//! carries the `QuotaName`, `Value`, `Unit`, `Adjustable`, `GlobalQuota`, `Period` and
//! `Description` of the quota that service publishes UNDER THAT CODE — the sentence as the
//! service words it, never a paraphrase and never a neighbouring quota's — and every code
//! the service publishes answers a row, because `QuotaCode` is the only key
//! `GetAWSDefaultServiceQuota`, `GetServiceQuota`, `RequestServiceQuotaIncrease` and
//! `ListRequestedServiceQuotaChangeHistoryByQuota` take. Smithy fixes only the row's SHAPE
//! (`smithy_query(query="shape", shape="ServiceQuota")`), so which quota a code publishes
//! is recorded behaviour.
//!
//! The table spans what the rule generalises over rather than repeating one row: five
//! `ServiceCode`s; a per-second RATE row, counted resource planes, a capacity plane and
//! two SIZE rows whose `Unit` is `Bytes` and `Kilobytes`; adjustable rows beside
//! not-adjustable ones; limits from 1 to 500,000; rows whose published sentence a source
//! states beside rows no source states one for, which therefore carry none; and codes whose
//! row this catalog had omitted altogether. Every row is read through the DEFAULT listing,
//! through `GetAWSDefaultServiceQuota` and through the APPLIED single read, because the code
//! is what a client acts on.
//!
//! The harness (`client`, `emulator`, `default_quotas_by_code`) is the one
//! `quota_workflow.rs` owns.

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas_by_code, emulator};

/// One published row: the quota a `QuotaCode` of that service publishes.
struct Bound {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// The published sentence, where a source states its text; `None` pins that the row
    /// carries NO description rather than one this emulator words itself.
    description: Option<&'static str>,
    /// `(PeriodValue, PeriodUnit)` for a row published over a window.
    period: Option<(i32, &'static str)>,
}

/// The rows this suite pins.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of each of these service
/// codes, as the project's issue record quotes it, states which quota the code publishes and
/// the text of the members pinned here. Where it states the quota but not one of its
/// members, that member is taken from the class the service publishes the quota in (the
/// custom-managed-list pair of `fms`, the per-deployment capacity planes of `fsx`, the
/// data-field size planes of `imagebuilder`) and where no source states it at all the row
/// carries none.
const BOUND: &[Bound] = &[
    // events — the request rate of the whole CONTROL PLANE, published over the one-second
    // window and worded for the control plane rather than for one API.
    Bound {
        service_code: "events",
        quota_code: "L-3C47459F",
        quota_name: "Throttle limit in transactions per second for control plane APIs",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "Maximum number of requests per second for EventBridge control plane API \
             operations. Additional requests are throttled.",
        ),
        period: Some((1, "SECOND")),
    },
    // events — the CONNECTION plane, under the code the recording binds it to rather than
    // the one this catalog had published it under.
    Bound {
        service_code: "events",
        quota_code: "L-595D6D42",
        quota_name: "Connections",
        value: 3000.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of connections per account per Region."),
        period: None,
    },
    // events — a SIZE published in BYTES, with the sentence the recording states for it.
    Bound {
        service_code: "events",
        quota_code: "L-5D1D13E3",
        quota_name: "[EventsV2] Resource policy size per event bus",
        value: 20_480.0,
        unit: "Bytes",
        adjustable: true,
        description: Some(
            "The maximum size, in bytes, of the customer-managed (default) resource policy \
             of a V2 event bus. Policies that AWS Resource Access Manager writes for a \
             shared bus do not count against this quota.",
        ),
        period: None,
    },
    // events — the event-INGEST rate, whose sentence that listing words WITHOUT the leading
    // article the service's endpoint-API rates carry.
    Bound {
        service_code: "events",
        quota_code: "L-9B653E91",
        quota_name: "PutEvents throttle limit in transactions per second",
        value: 10_000.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "Maximum number of requests per second for PutEvents API. Additional requests \
             are throttled.",
        ),
        period: Some((1, "SECOND")),
    },
    // events — a version 2 RESOURCE plane metered by that plane's resource count over the
    // window those counts are published on, and a row whose own sentence no source states.
    Bound {
        service_code: "events",
        quota_code: "L-D082A4F6",
        quota_name: "[EventsV2] Event Sources",
        value: 200.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: Some((5, "MINUTE")),
    },
    // fms — a per-policy count published as NOT adjustable at a limit of 2, under a code
    // this catalog had published an API rate under.
    Bound {
        service_code: "fms",
        quota_code: "L-743D9B7E",
        quota_name: "AWS WAF Classic rule groups per AWS WAF Classic policy",
        value: 2.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of AWS WAF Classic rule groups that you can use in a \
             Firewall Manager AWS WAF Classic policy.",
        ),
        period: None,
    },
    // fms — a CAPACITY plane, whose sentence the service publishes with two spaces in it.
    Bound {
        service_code: "fms",
        quota_code: "L-5AB0AC42",
        quota_name: "Stateful rule group capacity per Network Firewall policy",
        value: 30_000.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of rules that can be included in a stateful  rule \
             group within a Firewall Manager Network Firewall policy.",
        ),
        period: None,
    },
    // fms — the two halves of the custom-managed PROTOCOL-LIST pair: the lists one setting
    // of a content-audit policy may use, and the lists an account may define.
    Bound {
        service_code: "fms",
        quota_code: "L-4E3D82C8",
        quota_name: "Custom managed protocol lists in any content audit security group \
                     policy setting",
        value: 1.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of custom managed protocol lists that you can use in any \
             setting in a Firewall Manager content audit security group policy.",
        ),
        period: None,
    },
    Bound {
        service_code: "fms",
        quota_code: "L-C6CF2DBB",
        quota_name: "Custom managed protocol lists per account",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of custom managed protocol lists that you can define for \
             an account.",
        ),
        period: None,
    },
    // fsx — the count planes whose sentence this service words by what an account may
    // CREATE, which is the half of those two rows the recording states.
    Bound {
        service_code: "fsx",
        quota_code: "L-C48231E5",
        quota_name: "Lustre Scratch file systems",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of Amazon FSx for Lustre scratch file systems that you can \
             create in this account.",
        ),
        period: None,
    },
    Bound {
        service_code: "fsx",
        quota_code: "L-C28C1403",
        quota_name: "ONTAP file systems",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of Amazon FSx for NetApp ONTAP file systems that you can \
             create in this account.",
        ),
        period: None,
    },
    // fsx — an account-wide IOPS ceiling of 500,000 where a per-file-system count had
    // stood, with the sentence the recording states for it.
    Bound {
        service_code: "fsx",
        quota_code: "L-901C77F5",
        quota_name: "Windows total SSD IOPS",
        value: 500_000.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The total amount of SSD IOPS allowed for all Amazon FSx for Windows File \
             Server file systems that you can have in this account.",
        ),
        period: None,
    },
    // fsx — the LUSTRE family's backups, where a per-volume count of another family had
    // stood.
    Bound {
        service_code: "fsx",
        quota_code: "L-CD5E0524",
        quota_name: "Lustre backups",
        value: 500.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of user-initiated backups that you can have for all Amazon \
             FSx for Lustre file systems in this account.",
        ),
        period: None,
    },
    // fsx — a code the recording carries a row for and this catalog had omitted: a
    // per-deployment storage capacity, published as adjustable with the catalog's own unit.
    Bound {
        service_code: "fsx",
        quota_code: "L-8F1B9C74",
        quota_name: "Lustre Persistent_2 storage capacity",
        value: 100_800.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum amount of storage capacity (in GiB) that you can configure for \
             all Amazon FSx for Lustre persistent_2 file systems in this account.",
        ),
        period: None,
    },
    // gamelift — a per-attribute count of 100 where a game-session API rate had stood.
    Bound {
        service_code: "gamelift",
        quota_code: "L-8EBC4E87",
        quota_name: "Strings per string list matchmaking player attribute",
        value: 100.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of strings in a string list (SL) matchmaking player \
             attribute.",
        ),
        period: None,
    },
    // gamelift — a per-fleet policy ceiling, with the sentence the recording states for it.
    Bound {
        service_code: "gamelift",
        quota_code: "L-3A43EF3C",
        quota_name: "Maximum NewGameSessionsPerCreator per fleet configuration",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum new game sessions per creator allowed in a fleet's resource policy \
             configuration.",
        ),
        period: None,
    },
    // gamelift — the QUEUE plane of a Region, worded as this service words its per-Region
    // planes.
    Bound {
        service_code: "gamelift",
        quota_code: "L-22451070",
        quota_name: "Game session queues per region",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of game session queues allowed per region."),
        period: None,
    },
    // gamelift — three sentences the recording states for rows whose name and limit this
    // catalog already published: the service words them with `allowed` in them.
    Bound {
        service_code: "gamelift",
        quota_code: "L-BB62CF1D",
        quota_name: "Queue destinations per game session queue",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of queue destinations allowed per game session queue.",
        ),
        period: None,
    },
    Bound {
        service_code: "gamelift",
        quota_code: "L-8AE49BBD",
        quota_name: "Matchmaking rule sets per region",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of matchmaking rule sets allowed per region.",
        ),
        period: None,
    },
    Bound {
        service_code: "gamelift",
        quota_code: "L-EC03D793",
        quota_name: "Player sessions per game session",
        value: 200.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of player sessions that can join a game session.",
        ),
        period: None,
    },
    // imagebuilder — the LIFECYCLE-POLICY plane at a limit of 50, where the workflow plane
    // had stood.
    Bound {
        service_code: "imagebuilder",
        quota_code: "L-6105A4EE",
        quota_name: "Lifecycle policies",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of lifecycle policies that you can create in an account in \
             the current Region.",
        ),
        period: None,
    },
    // imagebuilder — the workflow plane under the name the service publishes it with.
    Bound {
        service_code: "imagebuilder",
        quota_code: "L-0DF2752F",
        quota_name: "Image workflows",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of image workflows that you can create in an account in \
             the current Region.",
        ),
        period: None,
    },
    // imagebuilder — a SIZE published in KILOBYTES where a per-second API rate had stood,
    // so a client that formats the value reads a size and not a rate.
    Bound {
        service_code: "imagebuilder",
        quota_code: "L-84F5AB81",
        quota_name: "Docker template size",
        value: 64.0,
        unit: "Kilobytes",
        adjustable: true,
        description: Some("The maximum size of a Docker template data field."),
        period: None,
    },
    // imagebuilder — the account's COMPONENTS, a count of 1000 where a rate of 5 per
    // second had stood.
    Bound {
        service_code: "imagebuilder",
        quota_code: "L-9B183655",
        quota_name: "Components",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of components that you can create in an account in the \
             current Region.",
        ),
        period: None,
    },
    // imagebuilder — a character LENGTH where a count of pipelines had stood: the parameter
    // value a component may be given, measured in characters rather than in resources.
    Bound {
        service_code: "imagebuilder",
        quota_code: "L-10D22E0D",
        quota_name: "Component parameter length",
        value: 1024.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of characters that a component parameter value can contain.",
        ),
        period: None,
    },
];

/// Every pinned row is published under its own code, with the members the recording states
/// for it, identically in the listing and in the code-keyed default read.
#[tokio::test]
async fn a_rebound_row_carries_the_quota_its_code_publishes() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

    let mut catalogs: std::collections::BTreeMap<&str, _> = std::collections::BTreeMap::new();
    for row in BOUND {
        if !catalogs.contains_key(row.service_code) {
            catalogs.insert(
                row.service_code,
                default_quotas_by_code(&caller, row.service_code).await,
            );
        }
    }

    for row in BOUND {
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

        // The code-keyed single read returns exactly the record the listing carries, and
        // the applied plane resolves the same quota at its default value: the code is the
        // key a client acts on.
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

/// A quota these bindings publish is published under ONE code of its service: a plane a
/// sourced binding took to its own code is not left under a second one as well.
#[tokio::test]
async fn a_rebound_quota_is_published_under_one_code_only() {
    let engine = emulator();
    let caller = client(&engine, "us-east-2");

    for service_code in ["events", "fms", "fsx", "gamelift", "imagebuilder"] {
        let catalog = default_quotas_by_code(&caller, service_code).await;
        for row in BOUND.iter().filter(|row| row.service_code == service_code) {
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
