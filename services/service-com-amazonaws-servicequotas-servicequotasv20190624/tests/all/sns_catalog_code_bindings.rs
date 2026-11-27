// SPDX-License-Identifier: CC0-1.0

//! WHICH quota each `QuotaCode` of the notification catalog publishes, and that every code
//! that catalog publishes answers a row.
//!
//! `QuotaCode` is the only stable key Service Quotas exposes — `GetAWSDefaultServiceQuota`,
//! `GetServiceQuota` and `RequestServiceQuotaIncrease` all take it — so a row is a claim
//! about the quota the service publishes UNDER THAT CODE. The rules asserted here:
//!
//! * every member of the row published under a code (`QuotaName`, `Description`, `Value`,
//!   `Unit`, `Adjustable`, `GlobalQuota`, `Period`, `UsageMetric`) belongs to the SAME
//!   quota — the one the service publishes under that code — so no row mixes one quota's
//!   name with another's limit, window or usage series;
//! * every code the catalog publishes answers exactly one row, through the listing and
//!   through both single reads, and no two rows publish the same quota: a code that
//!   resolves to nothing makes a quota that exists in AWS look nonexistent, and a quota
//!   published twice makes one of the two a duplicate of a limit the account does not
//!   have;
//! * and, for the request-rate class this catalog is mostly made of, the three members
//!   that name the ACTION a row throttles — the `QuotaName`, the `Description` and the
//!   `UsageMetric`'s `Resource` series — name the same action, which is the member-level
//!   form of the first rule and holds for every rate row rather than for the pinned ones
//!   only.
//!
//! The pinned table spans what a binding can get wrong: a rate published at ONE call per
//! second and as NOT adjustable beside rates published at 900 and 30 000, a counted plane
//! with no window and no usage metric, a metered plane over the five-minute window a
//! resource count is published on, a rate metered by a resource-usage series rather than by
//! an API call count, and a quota measured in `Bytes` beside the counted ones.
//!
//! The harness (`client`, `emulator`, `default_quotas`) is the one `quota_workflow.rs`
//! owns, and "is this a code AWS mints" is asked through `catalog_identity.rs`'s single
//! copy of that question.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::catalog_identity::is_aws_minted_shape;
use crate::quota_workflow::{client, default_quotas, emulator, Client};

/// The `ServiceCode` whose catalog this suite is about.
const SERVICE: &str = "sns";

/// How many quotas the notification service's published catalog holds.
///
/// SOURCE: AWS's published Service Quotas data for the `sns` service code (what
/// `aws service-quotas list-aws-default-service-quotas --service-code sns` reports).
const PUBLISHED_ROWS: usize = 48;

/// One pinned binding: the code, and the members of the quota published under it.
struct Bound {
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    description: &'static str,
    /// `(PeriodValue, PeriodUnit)`, for a quota published over a window.
    period: Option<(i32, &'static str)>,
    /// `(MetricName, MetricStatisticRecommendation, the `Resource` series or `""` where
    /// the listing states none)`, for a quota CloudWatch publishes usage for.
    metric: Option<(&'static str, &'static str, &'static str)>,
}

/// The bindings this suite pins.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of `sns`, which is the
/// reference for which quota a code publishes and for the wording of each row; the catalog
/// these are seeded from is `src/state/quota_rows/sns.rs`, which names the source of every
/// member beside it.
const BOUND: &[Bound] = &[
    // The per-API rate class, published at the three rates this catalog carries for it:
    // the window, the call count and the sentence are the class's, the ACTION, the rate
    // and the adjustability are the code's.
    Bound {
        quota_code: "L-AB928142",
        quota_name: "CreateTopic Transactions per Second",
        value: 900.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of times the CreateTopic action can be called per \
                      second.",
        period: Some((1, "SECOND")),
        metric: Some(("CallCount", "Sum", "CreateTopic")),
    },
    Bound {
        quota_code: "L-876E1222",
        quota_name: "GetSubscriptionAttributes Transactions per Second",
        value: 900.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of times the GetSubscriptionAttributes action can be \
                      called per second.",
        period: Some((1, "SECOND")),
        metric: Some(("CallCount", "Sum", "GetSubscriptionAttributes")),
    },
    // The same class at ONE call per second and published as NOT adjustable: both the rate
    // and the adjustability are facts of the code rather than of the class.
    Bound {
        quota_code: "L-154ECCA7",
        quota_name: "SetSMSAttributes Transactions per Second",
        value: 1.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of times the SetSMSAttributes action can be called per \
                      second.",
        period: Some((1, "SECOND")),
        metric: Some(("CallCount", "Sum", "SetSMSAttributes")),
    },
    Bound {
        quota_code: "L-390AADBE",
        quota_name: "ListSubscriptionsByTopic Transactions per Second",
        value: 30.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of times the ListSubscriptionsByTopic action can be \
                      called per second.",
        period: Some((1, "SECOND")),
        metric: Some(("CallCount", "Sum", "ListSubscriptionsByTopic")),
    },
    // The account's published-MESSAGE rate: a rate over the same one-second window, named
    // and described as a count of messages rather than of one action's calls, and metered
    // by a resource-usage series summed over the window rather than by an API call count —
    // the account's published-message series, which the listing names for it.
    Bound {
        quota_code: "L-F8E2BA85",
        quota_name: "Messages Published per Second",
        value: 9000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of messages published per second.",
        period: Some((1, "SECOND")),
        metric: Some((
            "ResourceCount",
            "Sum",
            "NumberOfMessagesPublishedPerAccount",
        )),
    },
    // The account's PENDING SUBSCRIPTION plane: a counted plane metered by the service's
    // resource count over the five-minute window those counts are published on, under the
    // series the listing names for it.
    Bound {
        quota_code: "L-1A43D3DB",
        quota_name: "Pending Subscriptions per Account",
        value: 5000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of pending subscriptions per AWS account, across all \
                      regions.",
        period: Some((5, "MINUTE")),
        metric: Some((
            "ResourceCount",
            "Maximum",
            "ApproximateNumberOfPendingSubscriptions",
        )),
    },
    // Two planes published over no window and with no usage metric at all: a counted one,
    // and the shape of one message — measured in `Bytes` and published as fixed.
    Bound {
        quota_code: "L-F514C636",
        quota_name: "Topics per Account",
        value: 100_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of topics in this account in the current Region.",
        period: None,
        metric: None,
    },
    Bound {
        quota_code: "L-A228E0EB",
        quota_name: "Message Size",
        value: 262_144.0,
        unit: "Bytes",
        adjustable: false,
        description: "The size, in bytes, of a published message.",
        period: None,
        metric: None,
    },
];

/// The suffix the per-API request-rate class of this catalog names a row with.
const RATE_SUFFIX: &str = " Transactions per Second";

/// The table has to span the member classes a binding can get wrong, or the rows below
/// prove nothing about a row of another shape.
#[test]
fn the_pinned_table_spans_the_classes_a_binding_can_get_wrong() {
    let units: BTreeSet<&str> = BOUND.iter().map(|bound| bound.unit).collect();
    let windows: BTreeSet<Option<&str>> = BOUND
        .iter()
        .map(|bound| bound.period.map(|(_, unit)| unit))
        .collect();
    let metrics: BTreeSet<Option<(&str, &str, bool)>> = BOUND
        .iter()
        .map(|bound| {
            bound
                .metric
                .map(|(name, statistic, series)| (name, statistic, series.is_empty()))
        })
        .collect();
    let values: BTreeSet<u64> = BOUND.iter().map(|bound| bound.value as u64).collect();
    assert_eq!(
        (
            units.len(),
            windows.len(),
            metrics.len(),
            values.len(),
            BOUND.iter().any(|bound| bound.adjustable),
            BOUND.iter().any(|bound| !bound.adjustable),
        ),
        (2, 3, 4, 7, true, true),
        "case table: expected two units {units:?}, three windows {windows:?}, four metric \
         shapes {metrics:?}, seven limits {values:?} and both adjustabilities"
    );
}

/// RULE: every member of the row published under a code belongs to the quota the service
/// publishes under THAT code, read through the listing and through both single reads.
#[tokio::test]
async fn a_code_publishes_every_member_of_its_own_quota() {
    let engine = emulator();
    // Two Regions: a binding is a fact of the published catalog rather than of one
    // Region's view of it.
    for region in ["us-east-1", "eu-central-1"] {
        let caller = client(&engine, region);
        let listed = by_code(&caller).await;

        for bound in BOUND {
            let case = format!("{region} {SERVICE}/{}", bound.quota_code);
            let pair = json!({"ServiceCode": SERVICE, "QuotaCode": bound.quota_code});
            let row = listed
                .get(bound.quota_code)
                .unwrap_or_else(|| panic!("case {case}: the listing must publish this code"));

            assert_eq!(
                (
                    row["QuotaName"].clone(),
                    row["Value"].clone(),
                    row["Unit"].clone(),
                    row["Adjustable"].clone(),
                    row["GlobalQuota"].clone(),
                    row["Description"].clone(),
                ),
                (
                    json!(bound.quota_name),
                    json!(bound.value),
                    json!(bound.unit),
                    json!(bound.adjustable),
                    json!(false),
                    json!(bound.description),
                ),
                "case {case}: expected the name, limit, unit, adjustability and sentence the \
                 service publishes under THIS code, got {row}"
            );

            match bound.period {
                Some((value, unit)) => assert_eq!(
                    row["Period"],
                    json!({"PeriodValue": value, "PeriodUnit": unit}),
                    "case {case}: expected the window this quota is published over, got {row}"
                ),
                None => assert!(
                    row.get("Period").is_none_or(Value::is_null),
                    "case {case}: this quota is published over no window, got {row}"
                ),
            }

            match bound.metric {
                Some((metric_name, statistic, series)) => {
                    let mut dimensions = BTreeMap::from([
                        ("Service", "SNS"),
                        ("Class", "None"),
                        (
                            "Type",
                            if metric_name == "CallCount" {
                                "API"
                            } else {
                                "Resource"
                            },
                        ),
                    ]);
                    if !series.is_empty() {
                        dimensions.insert("Resource", series);
                    }
                    assert_eq!(
                        row["UsageMetric"],
                        json!({
                            "MetricNamespace": "AWS/Usage",
                            "MetricName": metric_name,
                            "MetricStatisticRecommendation": statistic,
                            "MetricDimensions": dimensions,
                        }),
                        "case {case}: expected the usage metric of THIS quota's plane, got {row}"
                    );
                }
                None => assert!(
                    row.get("UsageMetric").is_none_or(Value::is_null),
                    "case {case}: this quota publishes no usage metric, got {row}"
                ),
            }

            // The default read and the applied read resolve the same key to the same row.
            let default = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"]
                .clone();
            assert_eq!(
                &default, row,
                "case {case}: the single default read must return the record the listing \
                 carries"
            );
            let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
            assert_eq!(
                (
                    applied["QuotaName"].clone(),
                    applied["Value"].clone(),
                    applied["Unit"].clone(),
                ),
                (
                    json!(bound.quota_name),
                    json!(bound.value),
                    json!(bound.unit),
                ),
                "case {case}: the applied read resolves the same quota at its default, got \
                 {applied}"
            );
        }
    }
}

/// RULE, over EVERY row rather than the pinned ones: the members of a request-rate row
/// that name the action it throttles — the name, the sentence and the usage series — name
/// the SAME action, so a row cannot carry one action's name over another's limit.
#[tokio::test]
async fn every_rate_row_names_one_action_in_all_of_its_members() {
    let engine = emulator();
    let caller = client(&engine, "us-west-2");
    let listed = by_code(&caller).await;

    let mut rates = 0;
    for (code, row) in &listed {
        let name = row["QuotaName"].as_str().unwrap_or_default();
        let Some(action) = name.strip_suffix(RATE_SUFFIX) else {
            continue;
        };
        rates += 1;
        let case = format!("{SERVICE}/{code}");

        assert_eq!(
            row["Description"],
            json!(format!(
                "The maximum number of times the {action} action can be called per second."
            )),
            "case {case}: a rate row's sentence must name the action its name does, got {row}"
        );
        assert_eq!(
            row["Period"],
            json!({"PeriodValue": 1, "PeriodUnit": "SECOND"}),
            "case {case}: a rate row is published over the one-second window, got {row}"
        );
        assert_eq!(
            row["UsageMetric"],
            json!({
                "MetricNamespace": "AWS/Usage",
                "MetricName": "CallCount",
                "MetricStatisticRecommendation": "Sum",
                "MetricDimensions": {
                    "Service": "SNS",
                    "Class": "None",
                    "Resource": action,
                    "Type": "API",
                },
            }),
            "case {case}: a rate row's usage metric must count calls of the action its name \
             states, got {row}"
        );
        assert!(
            row["Value"].as_f64().is_some_and(|value| value > 0.0),
            "case {case}: a published rate is a positive number of calls per second, got {row}"
        );
    }

    assert!(
        rates > 30,
        "case {SERVICE}: this catalog is mostly the per-API rate class, got {rates} rate rows \
         of {} published",
        listed.len()
    );
}

/// RULE: every code the catalog publishes answers exactly one row — in the listing and in
/// both single reads — and no quota is published twice.
#[tokio::test]
async fn every_published_code_answers_one_row_and_every_quota_once() {
    let engine = emulator();
    let caller = client(&engine, "ap-northeast-1");
    let quotas = default_quotas(&caller, SERVICE).await;

    assert_eq!(
        quotas.len(),
        PUBLISHED_ROWS,
        "case {SERVICE}: expected the whole published catalog ({PUBLISHED_ROWS} quotas), got {} \
         ({:?})",
        quotas.len(),
        quotas
            .iter()
            .filter_map(|quota| quota["QuotaCode"].as_str())
            .collect::<Vec<_>>()
    );

    let mut codes: BTreeSet<&str> = BTreeSet::new();
    let mut names: BTreeSet<&str> = BTreeSet::new();
    for quota in &quotas {
        let code = quota["QuotaCode"].as_str().unwrap_or_default();
        let name = quota["QuotaName"].as_str().unwrap_or_default();
        let case = format!("{SERVICE}/{code}");

        assert!(
            is_aws_minted_shape(code),
            "case {case}: every published row is keyed on the code AWS assigns the quota, got \
             {quota}"
        );
        assert!(
            codes.insert(code),
            "case {case}: a code answers exactly one row, got a second one: {quota}"
        );
        assert!(
            names.insert(name),
            "case {case}: a quota is published under one code only, got {name:?} twice"
        );
        assert_eq!(
            (quota["ServiceCode"].clone(), quota["QuotaArn"].clone()),
            (
                json!(SERVICE),
                json!(format!(
                    "arn:aws:servicequotas:ap-northeast-1::{SERVICE}/{code}"
                ))
            ),
            "case {case}: every row names this service and its own code in its ARN, got {quota}"
        );

        // The same key resolves through both single reads: a published code that answered
        // nothing would make a quota that exists in AWS look nonexistent.
        let pair = json!({"ServiceCode": SERVICE, "QuotaCode": code});
        for operation in ["GetAWSDefaultServiceQuota", "GetServiceQuota"] {
            let resolved = caller.ok(operation, pair.clone()).await["Quota"].clone();
            assert_eq!(
                (resolved["QuotaCode"].clone(), resolved["QuotaName"].clone()),
                (json!(code), json!(name)),
                "case {case}: {operation} must resolve the code the listing publishes"
            );
        }
    }
}

/// Every default row of this catalog, keyed by its `QuotaCode`.
async fn by_code(caller: &Client) -> BTreeMap<String, Value> {
    default_quotas(caller, SERVICE)
        .await
        .into_iter()
        .filter_map(|quota| {
            Some((quota["QuotaCode"].as_str()?.to_string(), quota))
        })
        .collect()
}
