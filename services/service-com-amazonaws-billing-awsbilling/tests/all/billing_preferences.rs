// SPDX-License-Identifier: CC0-1.0

//! Billing preferences end to end over the awsJson1_0 wire.
//!
//! Every assertion here is about ONE rule: `GetBillingPreferences` reports, for the
//! single feature a request names, exactly what this service persists for that
//! feature — the keys `UpdateBillingPreferences` set, or, for a history feature, the
//! recorded changes to the feature behind it — narrowed by the `PREFERENCE_KEY`
//! filter and paged by `maxResults`/`nextToken`. The tables span what the rule
//! generalises over: which feature, how many keys, the key's shape (feature-level,
//! account-list, `credit/{creditId}/{option}`), the value, the page size (including
//! the modelled `maxResults` bounds and one past each) and the filter's value count.

use std::sync::Arc;
use std::time::SystemTime;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::credits::{credit, seed};

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`),
/// which is the management account of this emulator's consolidated billing family.
const ACCOUNT_ID: &str = "000000000000";

/// The display name that account carries in the family
/// (`crate::state::MANAGEMENT_ACCOUNT_NAME`).
const ACCOUNT_NAME: &str = "Management Account";

/// A pattern-valid 12-digit account id that is in no consolidated billing family
/// this emulator holds.
const FOREIGN_ACCOUNT_ID: &str = "999999999999";

/// The credits this suite seeds into the store before any call, so that a
/// `credit/{creditId}/{option}` key names a credit the family actually holds —
/// which is what `UpdateBillingPreferences` requires of one. The ids are
/// modelled `CreditId`s: 1..32 characters of `^[0-9]+$`
/// (`smithy_query(query="shape", shape="CreditId")`).
const SEEDED_CREDIT_IDS: &[&str] = &["1", "7", "42", "9001"];

/// Every `BillingFeature` the model declares (`BILLING_FEATURE_VALUES`).
const ALL_FEATURES: &[&str] = &[
    "BILLING_ALERTS",
    "CREDIT_LEVEL_SHARING",
    "CREDIT_PREFERENCE_OPTIONS",
    "CREDIT_SHARING",
    "CREDIT_SHARING_HISTORY",
    "RI_SHARING",
    "RI_SHARING_HISTORY",
];

/// The `BillingFeature` values `UpdateBillingPreferences` accepts: the `feature`
/// doc's "Valid values: BILLING_ALERTS, RI_SHARING, CREDIT_SHARING,
/// CREDIT_LEVEL_SHARING, CREDIT_PREFERENCE_OPTIONS".
const UPDATABLE_FEATURES: &[&str] = &[
    "BILLING_ALERTS",
    "RI_SHARING",
    "CREDIT_SHARING",
    "CREDIT_LEVEL_SHARING",
    "CREDIT_PREFERENCE_OPTIONS",
];

/// One router over ONE shared engine — what `core/router` builds in production, so
/// every call in a test observes the same state.
///
/// Every `BillingFeature` is ACTIVATED on that engine
/// (`ComAmazonawsBillingAwsbillingState::feature_access`): this suite is about what
/// the preference store reports for a feature the caller can reach, so it opens the
/// per-feature access gate that the default state leaves closed for the features
/// behind the Billing-console access activation. The gate itself — which features
/// are refused, with what code, on a read and on a write — is pinned in
/// `tests/all/billing_feature_access.rs`.
/// The credits of [`SEEDED_CREDIT_IDS`] are put in the store too, owned by the
/// account every request here authenticates as: a credit preference key names a
/// credit, and this suite is about what the preference store reports for one,
/// not about the credit lookup (pinned in `tests/all/credit_preference_keys.rs`).
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<service_com_amazonaws_billing_awsbilling::ComAmazonawsBillingAwsbillingState>(
            );
        let mut state = state.lock();
        for feature in ALL_FEATURES {
            state.grant_feature_access(feature);
        }
    }
    seed(
        &engine,
        SEEDED_CREDIT_IDS
            .iter()
            .map(|credit_id| credit(credit_id, ACCOUNT_ID))
            .collect(),
    );
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one awsJson1_0 call and return its HTTP status and decoded body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSBilling.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/billing/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

/// The `__type` an awsJson error body carries, without its namespace prefix.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `UpdateBillingPreferences` for one feature and one or more key/value pairs.
async fn update(
    router: &MultiServiceRouter,
    feature: &str,
    pairs: &[(&str, &str)],
) -> (u16, Value) {
    let keys: Vec<Value> = pairs
        .iter()
        .map(|(key, value)| json!({ "key": key, "value": value }))
        .collect();
    call(
        router,
        "UpdateBillingPreferences",
        &json!({ "feature": feature, "billingPreferencesPerKey": keys }),
    )
    .await
}

/// `GetBillingPreferences` for one feature, with no filter and no page size.
async fn get(router: &MultiServiceRouter, feature: &str) -> (u16, Value) {
    call(
        router,
        "GetBillingPreferences",
        &json!({ "features": [feature] }),
    )
    .await
}

/// The `(key, value)` pairs a `GetBillingPreferences` body reports, in the order
/// it reported them.
fn reported(body: &Value) -> Vec<(String, String)> {
    body.get("billingPreferences")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry
                            .get("key")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        entry
                            .get("value")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every `(key, value)` pair a feature reports, walking every page — a feature
/// may hold more entries than the 50 one page carries by default.
async fn read_all(router: &MultiServiceRouter, feature: &str) -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "features": [feature], "maxResults": 50 });
        if let Some(next) = &token {
            request["nextToken"] = json!(next);
        }
        let (status, body) = call(router, "GetBillingPreferences", &request).await;
        assert_eq!(status, 200, "case {feature:?}: a page read failed: {body:?}");
        all.extend(reported(&body));
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return all;
        }
    }
}

/// One row of the round-trip table: the keys one feature carries after an update.
struct FeatureRow {
    case: &'static str,
    feature: &'static str,
    /// The key/value pairs to set, in request order.
    pairs: Vec<(&'static str, &'static str)>,
    /// The pairs the read must report, in key order.
    expected: Vec<(&'static str, &'static str)>,
}

/// The rows differ along the feature, the number of keys (one through four), the
/// key's shape (feature-level, account-list, `credit/{creditId}/{option}`, a key
/// using every character class `PreferenceKey`'s pattern admits) and the value.
fn feature_rows() -> Vec<FeatureRow> {
    vec![
        FeatureRow {
            case: "billing alerts, one feature-level key",
            feature: "BILLING_ALERTS",
            pairs: vec![("billing-alerts", "ENABLED")],
            expected: vec![("billing-alerts", "ENABLED")],
        },
        FeatureRow {
            case: "billing alerts, every character class the key pattern admits",
            feature: "BILLING_ALERTS",
            pairs: vec![("alerts/all: 9-*", "DISABLED")],
            expected: vec![("alerts/all: 9-*", "DISABLED")],
        },
        FeatureRow {
            case: "ri sharing, a feature-level key and an account-list key",
            feature: "RI_SHARING",
            pairs: vec![
                ("ri-sharing", "ENABLED"),
                ("account/000000000000", "DISABLED"),
            ],
            expected: vec![
                ("account/000000000000", "DISABLED"),
                ("ri-sharing", "ENABLED"),
            ],
        },
        FeatureRow {
            case: "credit sharing, four keys set in one request",
            feature: "CREDIT_SHARING",
            pairs: vec![
                ("credit-sharing", "ENABLED"),
                ("account/000000000000", "ENABLED"),
                ("scope/all", "DISABLED"),
                ("scope/payer", "ENABLED"),
            ],
            expected: vec![
                ("account/000000000000", "ENABLED"),
                ("credit-sharing", "ENABLED"),
                ("scope/all", "DISABLED"),
                ("scope/payer", "ENABLED"),
            ],
        },
        FeatureRow {
            case: "credit level sharing, one account-list key",
            feature: "CREDIT_LEVEL_SHARING",
            pairs: vec![("account/000000000000", "ENABLED")],
            expected: vec![("account/000000000000", "ENABLED")],
        },
        FeatureRow {
            case: "credit preference options, two options of one credit",
            feature: "CREDIT_PREFERENCE_OPTIONS",
            pairs: vec![
                ("credit/9001/status", "ENABLED"),
                ("credit/9001/sharing", "DISABLED"),
            ],
            expected: vec![
                ("credit/9001/sharing", "DISABLED"),
                ("credit/9001/status", "ENABLED"),
            ],
        },
    ]
}

#[tokio::test]
async fn an_update_is_what_the_next_read_reports() {
    for row in feature_rows() {
        let router = emulator();
        let (status, body) = update(&router, row.feature, &row.pairs).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the update to succeed, got {status} {body:?}",
            row.case
        );

        let (status, body) = get(&router, row.feature).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the read to succeed, got {status} {body:?}",
            row.case
        );
        let expected: Vec<(String, String)> = row
            .expected
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            reported(&body),
            expected,
            "case {:?}: expected the stored preferences in key order, got {body:?}",
            row.case
        );
        // Every entry names the feature that was asked for, and a read of a
        // non-history feature carries no billing period.
        for entry in body["billingPreferences"]
            .as_array()
            .expect("billingPreferences is a list")
        {
            assert_eq!(
                entry.get("feature").and_then(Value::as_str),
                Some(row.feature),
                "case {:?}: expected every entry to name the requested feature, got {entry:?}",
                row.case
            );
            assert!(
                entry.get("billingPeriod").is_none_or(Value::is_null),
                "case {:?}: a non-history feature reports no billingPeriod, got {entry:?}",
                row.case
            );
        }
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {:?}: one page holds every entry, so nextToken must be absent, got {body:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_feature_nothing_has_set_reports_no_preferences() {
    let router = emulator();
    for feature in ALL_FEATURES {
        let (status, body) = get(&router, feature).await;
        assert_eq!(
            status, 200,
            "case {feature:?}: expected the read to succeed, got {status} {body:?}"
        );
        assert_eq!(
            reported(&body),
            Vec::new(),
            "case {feature:?}: an unset feature reports nothing, got {body:?}"
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {feature:?}: an empty page carries no nextToken, got {body:?}"
        );
    }
}

#[tokio::test]
async fn one_feature_s_preferences_are_invisible_to_another() {
    let router = emulator();
    for feature in UPDATABLE_FEATURES {
        let key = if *feature == "CREDIT_PREFERENCE_OPTIONS" {
            "credit/1/status"
        } else {
            "shared-key"
        };
        let (status, body) = update(&router, feature, &[(key, "ENABLED")]).await;
        assert_eq!(
            status, 200,
            "case {feature:?}: expected the update to succeed, got {status} {body:?}"
        );
    }
    for feature in UPDATABLE_FEATURES {
        let expected_key = if *feature == "CREDIT_PREFERENCE_OPTIONS" {
            "credit/1/status"
        } else {
            "shared-key"
        };
        let (_, body) = get(&router, feature).await;
        assert_eq!(
            reported(&body),
            vec![(expected_key.to_string(), "ENABLED".to_string())],
            "case {feature:?}: a feature reports only its own keys, got {body:?}"
        );
    }
}

#[tokio::test]
async fn an_update_leaves_every_key_it_does_not_name_unchanged() {
    // The rule holds for each updatable feature, so each one is a row.
    for feature in UPDATABLE_FEATURES {
        let (kept, changed) = if *feature == "CREDIT_PREFERENCE_OPTIONS" {
            ("credit/7/status", "credit/7/sharing")
        } else {
            ("kept-key", "changed-key")
        };
        let router = emulator();
        let (status, body) =
            update(&router, feature, &[(kept, "ENABLED"), (changed, "ENABLED")]).await;
        assert_eq!(
            status, 200,
            "case {feature:?}: expected the first update to succeed, got {status} {body:?}"
        );

        let (status, body) = update(&router, feature, &[(changed, "DISABLED")]).await;
        assert_eq!(
            status, 200,
            "case {feature:?}: expected the second update to succeed, got {status} {body:?}"
        );

        let (_, body) = get(&router, feature).await;
        let mut expected = vec![
            (changed.to_string(), "DISABLED".to_string()),
            (kept.to_string(), "ENABLED".to_string()),
        ];
        expected.sort();
        assert_eq!(
            reported(&body),
            expected,
            "case {feature:?}: only the named key moves, got {body:?}"
        );
    }
}

#[tokio::test]
async fn an_account_list_key_reports_the_account_it_names() {
    let router = emulator();
    // A key whose last segment is a 12-digit account id in the caller's family is
    // an account-list key: `accountId` reports that id and `accountName` the
    // account's display name, "populated together with accountId".
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        ("RI_SHARING", "account/000000000000", Some(ACCOUNT_ID)),
        (
            "CREDIT_LEVEL_SHARING",
            "linked/account/000000000000",
            Some(ACCOUNT_ID),
        ),
        // Not an account-list key: the last segment is not a 12-digit id.
        ("CREDIT_SHARING", "credit-sharing", None),
        ("BILLING_ALERTS", "alerts/00000000000", None),
        // A non-sharing feature's account-list key: the trailing account is a
        // member of the family, so it is reported exactly as a sharing
        // feature's is. An account-list key naming an account OUTSIDE the
        // family is refused rather than stored, whichever feature carries it
        // (`tests/all/account_list_preference_keys.rs`), so no stored
        // account-list key reports an absent account.
        ("BILLING_ALERTS", "alerts/000000000000", Some(ACCOUNT_ID)),
    ];
    for (feature, key, expected_account) in rows {
        let (status, body) = update(&router, feature, &[(key, "ENABLED")]).await;
        assert_eq!(
            status, 200,
            "case {key:?}: expected the update to succeed, got {status} {body:?}"
        );
        let (_, body) = call(
            &router,
            "GetBillingPreferences",
            &json!({ "features": [feature], "filters": [{ "name": "PREFERENCE_KEY", "value": [key] }] }),
        )
        .await;
        let entry = body["billingPreferences"]
            .as_array()
            .and_then(|entries| entries.first().cloned())
            .unwrap_or(Value::Null);
        assert_eq!(
            entry.get("accountId").and_then(Value::as_str),
            expected_account,
            "case {key:?}: expected accountId {expected_account:?}, got {entry:?}"
        );
        assert_eq!(
            entry.get("accountName").and_then(Value::as_str),
            expected_account.map(|_| ACCOUNT_NAME),
            "case {key:?}: accountName is populated exactly with accountId, got {entry:?}"
        );
    }
}

#[tokio::test]
async fn a_history_feature_reports_the_changes_to_the_feature_behind_it() {
    // `BILLING_FEATURE_VALUES` pairs each of these with its history feature.
    let rows: Vec<(&str, &str)> = vec![
        ("RI_SHARING", "RI_SHARING_HISTORY"),
        ("CREDIT_SHARING", "CREDIT_SHARING_HISTORY"),
    ];
    for (feature, history) in rows {
        let router = emulator();
        for value in ["ENABLED", "DISABLED", "DISABLED", "ENABLED"] {
            let (status, body) = update(&router, feature, &[("account/000000000000", value)]).await;
            assert_eq!(
                status, 200,
                "case {feature:?}: expected the update to succeed, got {status} {body:?}"
            );
        }

        let (status, body) = get(&router, history).await;
        assert_eq!(
            status, 200,
            "case {history:?}: expected the read to succeed, got {status} {body:?}"
        );
        // Three changes, not four: re-setting a key to the value it already has
        // changes nothing, so the record of changes does not grow.
        assert_eq!(
            reported(&body),
            vec![
                ("account/000000000000".to_string(), "ENABLED".to_string()),
                ("account/000000000000".to_string(), "DISABLED".to_string()),
                ("account/000000000000".to_string(), "ENABLED".to_string()),
            ],
            "case {history:?}: expected the recorded changes oldest first, got {body:?}"
        );

        let (year, month) = current_billing_period();
        for entry in body["billingPreferences"]
            .as_array()
            .expect("billingPreferences is a list")
        {
            assert_eq!(
                entry.get("feature").and_then(Value::as_str),
                Some(history),
                "case {history:?}: every entry names the requested feature, got {entry:?}"
            );
            assert_eq!(
                entry["billingPeriod"]["year"].as_i64(),
                Some(i64::from(year)),
                "case {history:?}: expected the change's billing year, got {entry:?}"
            );
            assert_eq!(
                entry["billingPeriod"]["month"].as_i64(),
                Some(i64::from(month)),
                "case {history:?}: expected the change's billing month, got {entry:?}"
            );
        }

        // The other feature's history is untouched by this one's changes.
        let other = if history == "RI_SHARING_HISTORY" {
            "CREDIT_SHARING_HISTORY"
        } else {
            "RI_SHARING_HISTORY"
        };
        let (_, body) = get(&router, other).await;
        assert_eq!(
            reported(&body),
            Vec::new(),
            "case {history:?}: {other} records nothing about {feature}, got {body:?}"
        );
    }
}

/// The `(year, month)` of the current UTC billing period, rendered by the same
/// RFC 3339 formatter the wire uses — the modelled `BillingYear` range is
/// 2005..=2120 and `Month` is 1..=12.
fn current_billing_period() -> (i32, i32) {
    let rendered = DateTime::from(SystemTime::now())
        .fmt(Format::DateTime)
        .expect("the clock renders as RFC 3339");
    let year: i32 = rendered[0..4].parse().expect("a four-digit year");
    let month: i32 = rendered[5..7].parse().expect("a two-digit month");
    assert!(
        (2005..=2120).contains(&year) && (1..=12).contains(&month),
        "case now: expected a modelled BillingPeriod, got {year}-{month}"
    );
    (year, month)
}

#[tokio::test]
async fn pagination_walks_every_entry_exactly_once() {
    // Eight keys, so each page size below leaves a different remainder.
    let keys: Vec<String> = (0..8).map(|index| format!("key/{index}")).collect();
    // The page sizes span both modelled `maxResults` bounds (1 and 50), the
    // sizes that divide and do not divide the entry count, and the whole set.
    for page_size in [1_usize, 3, 4, 8, 50] {
        let router = emulator();
        let pairs: Vec<(&str, &str)> = keys
            .iter()
            .map(|key| (key.as_str(), "ENABLED"))
            .collect();
        let (status, body) = update(&router, "BILLING_ALERTS", &pairs).await;
        assert_eq!(
            status, 200,
            "case {page_size}: expected the update to succeed, got {status} {body:?}"
        );

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "features": ["BILLING_ALERTS"], "maxResults": page_size });
            if let Some(next) = &token {
                request["nextToken"] = json!(next);
            }
            let (status, body) = call(&router, "GetBillingPreferences", &request).await;
            assert_eq!(
                status, 200,
                "case {page_size}: expected page {pages} to succeed, got {status} {body:?}"
            );
            let page = reported(&body);
            assert!(
                page.len() <= page_size,
                "case {page_size}: a page holds at most maxResults entries, got {}",
                page.len()
            );
            walked.extend(page.into_iter().map(|(key, _)| key));
            pages += 1;
            assert!(
                pages <= keys.len() + 1,
                "case {page_size}: the walk must terminate, got {pages} pages"
            );
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, keys,
            "case {page_size}: expected every key exactly once in key order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let router = emulator();
    let (status, body) = update(&router, "BILLING_ALERTS", &[("alerts", "ENABLED")]).await;
    assert_eq!(status, 200, "case setup: got {status} {body:?}");
    // `maxResults` is modelled `@range(min: 1, max: 50)`: both bounds are
    // accepted and one past each is refused.
    for (case, max_results, expected) in [
        ("lower bound", 1, 200),
        ("upper bound", 50, 200),
        ("below the lower bound", 0, 400),
        ("above the upper bound", 51, 400),
    ] {
        let (status, body) = call(
            &router,
            "GetBillingPreferences",
            &json!({ "features": ["BILLING_ALERTS"], "maxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} for maxResults {max_results}, got {status} {body:?}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_preference_key_filter_selects_only_the_keys_it_names() {
    let router = emulator();
    let keys: Vec<String> = (0..5).map(|index| format!("key/{index}")).collect();
    let pairs: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "ENABLED")).collect();
    let (status, body) = update(&router, "CREDIT_SHARING", &pairs).await;
    assert_eq!(status, 200, "case setup: got {status} {body:?}");

    // The rows differ in how many values the filter carries (one, several, the
    // modelled maximum of ten) and whether a value names a stored key.
    let mut ten_values: Vec<String> = keys.clone();
    ten_values.extend((0..5).map(|index| format!("absent/{index}")));
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("one stored key", vec![keys[2].clone()], vec![keys[2].clone()]),
        (
            "two stored keys",
            vec![keys[0].clone(), keys[4].clone()],
            vec![keys[0].clone(), keys[4].clone()],
        ),
        ("one key nothing stored", vec!["key/99".to_string()], vec![]),
        ("ten values, five of them stored", ten_values, keys.clone()),
    ];
    for (case, values, expected) in rows {
        let (status, body) = call(
            &router,
            "GetBillingPreferences",
            &json!({
                "features": ["CREDIT_SHARING"],
                "filters": [{ "name": "PREFERENCE_KEY", "value": values }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to succeed, got {status} {body:?}"
        );
        let reported_keys: Vec<String> = reported(&body).into_iter().map(|(key, _)| key).collect();
        assert_eq!(
            reported_keys, expected,
            "case {case:?}: expected the filtered keys, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_pagination_token_this_service_did_not_issue_is_refused() {
    let router = emulator();
    let pairs: Vec<(&str, &str)> = vec![("a", "ENABLED"), ("b", "ENABLED")];
    let (status, body) = update(&router, "RI_SHARING", &pairs).await;
    assert_eq!(status, 200, "case setup: got {status} {body:?}");
    let (_, body) = call(
        &router,
        "GetBillingPreferences",
        &json!({ "features": ["RI_SHARING"], "maxResults": 1 }),
    )
    .await;
    let issued = body
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a second page is available")
        .to_string();

    // A token is accepted only for the request that issued it; anything else is
    // a bad request, not an empty page.
    let rows: Vec<(&str, &str, &str, u16)> = vec![
        ("the token this read issued", "RI_SHARING", &issued, 200),
        ("that token on another feature", "CREDIT_SHARING", &issued, 400),
        ("text that is not a token", "RI_SHARING", "not-a-token", 400),
        ("hex that decodes to nonsense", "RI_SHARING", "6e6f6e73656e7365", 400),
    ];
    for (case, feature, token, expected) in rows {
        let (status, body) = call(
            &router,
            "GetBillingPreferences",
            &json!({ "features": [feature], "maxResults": 1, "nextToken": token }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body:?}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
        }
    }
}

/// One row of the refusal table: a request the documented rules refuse.
struct RefusalRow {
    case: &'static str,
    feature: &'static str,
    pairs: Vec<(&'static str, &'static str)>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn a_refused_update_writes_nothing() {
    // Each row is a request the documented rules refuse, with the status and
    // error code it is refused with. None of them may change stored state.
    let rows: Vec<RefusalRow> = vec![
        RefusalRow {
            case: "a history feature is read-only",
            feature: "RI_SHARING_HISTORY",
            pairs: vec![("account/000000000000", "ENABLED")],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "the same key twice in one request",
            feature: "BILLING_ALERTS",
            pairs: vec![("alerts", "ENABLED"), ("alerts", "DISABLED")],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "a credit preference key that names no credit",
            feature: "CREDIT_PREFERENCE_OPTIONS",
            pairs: vec![("status", "ENABLED")],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "two credits in one credit preference request",
            feature: "CREDIT_PREFERENCE_OPTIONS",
            pairs: vec![
                ("credit/1/status", "ENABLED"),
                ("credit/7/status", "ENABLED"),
            ],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "a sharing key for an account outside the family",
            feature: "RI_SHARING",
            pairs: vec![("account/999999999999", "ENABLED")],
            status: 403,
            code: "AccessDeniedException",
        },
        RefusalRow {
            case: "a credit sharing key for an account outside the family",
            feature: "CREDIT_LEVEL_SHARING",
            pairs: vec![("account/999999999999", "DISABLED")],
            status: 403,
            code: "AccessDeniedException",
        },
    ];
    for row in rows {
        let router = emulator();
        let (status, body) = update(&router, row.feature, &row.pairs).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status} {body:?}",
            row.case, row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {:?}: expected {}, got {body:?}",
            row.case,
            row.code
        );
        for read in ALL_FEATURES {
            let (_, body) = get(&router, read).await;
            assert_eq!(
                reported(&body),
                Vec::new(),
                "case {:?}: a refused update stores nothing, but {read} reports {body:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_sharing_key_for_a_family_account_is_accepted() {
    // The counterpart of the refusal above: the caller IS the management account
    // of this consolidated billing family, and the account the key names is in
    // that family, so the write lands.
    for feature in ["RI_SHARING", "CREDIT_SHARING", "CREDIT_LEVEL_SHARING"] {
        let router = emulator();
        let key = format!("account/{ACCOUNT_ID}");
        let (status, body) = update(&router, feature, &[(key.as_str(), "ENABLED")]).await;
        assert_eq!(
            status, 200,
            "case {feature:?}: expected the sharing update to succeed, got {status} {body:?}"
        );
        let (_, body) = get(&router, feature).await;
        assert_eq!(
            reported(&body),
            vec![(key.clone(), "ENABLED".to_string())],
            "case {feature:?}: expected the stored sharing preference, got {body:?}"
        );
        // And the foreign account is still refused for this same feature.
        let foreign = format!("account/{FOREIGN_ACCOUNT_ID}");
        let (status, body) = update(&router, feature, &[(foreign.as_str(), "ENABLED")]).await;
        assert_eq!(
            status, 403,
            "case {feature:?}: expected HTTP 403 for an account outside the family, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn a_batch_past_the_per_request_ceiling_is_refused() {
    // `billingPreferencesPerKey` is modelled with a minimum and no maximum, so
    // the emulator carries its own ceiling (`state::MAX_KEYS_PER_UPDATE`): the
    // bound itself is accepted and one past it is refused, writing nothing.
    const CEILING: usize = 100;
    for (case, count, expected) in [
        ("one key", 1, 200),
        ("the ceiling", CEILING, 200),
        ("one past the ceiling", CEILING + 1, 400),
    ] {
        let router = emulator();
        let keys: Vec<String> = (0..count).map(|index| format!("key/{index}")).collect();
        let pairs: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "ENABLED")).collect();
        let (status, body) = update(&router, "BILLING_ALERTS", &pairs).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} for {count} keys, got {status} {body:?}"
        );
        let (_, read) = get(&router, "BILLING_ALERTS").await;
        let stored = read_all(&router, "BILLING_ALERTS").await.len();
        if expected == 200 {
            assert_eq!(
                stored, count,
                "case {case:?}: expected {count} stored keys, got {read:?}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
            assert_eq!(
                stored, 0,
                "case {case:?}: a refused batch stores nothing, got {read:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_credit_status_key_is_settable_and_read_back() {
    // `credit/{creditId}/status` is the one CREDIT_PREFERENCE_OPTIONS key the
    // doc singles out as settable by a member account, and `sharing` options of
    // the same credit travel with it.
    let router = emulator();
    let (status, body) = update(
        &router,
        "CREDIT_PREFERENCE_OPTIONS",
        &[
            ("credit/42/status", "ENABLED"),
            ("credit/42/sharing", "DISABLED"),
        ],
    )
    .await;
    assert_eq!(status, 200, "case setup: got {status} {body:?}");
    let (_, body) = get(&router, "CREDIT_PREFERENCE_OPTIONS").await;
    assert_eq!(
        reported(&body),
        vec![
            ("credit/42/sharing".to_string(), "DISABLED".to_string()),
            ("credit/42/status".to_string(), "ENABLED".to_string()),
        ],
        "case credit options: expected both stored options in key order, got {body:?}"
    );
}
