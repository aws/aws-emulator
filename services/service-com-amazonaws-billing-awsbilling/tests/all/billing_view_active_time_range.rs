// SPDX-License-Identifier: CC0-1.0

//! The `ListBillingViews` `activeTimeRange` filter, end to end over the
//! awsJson1_0 wire.
//!
//! Every assertion here is about the RULE the filter follows: a view is selected
//! by a time range when the resource behind it already existed at the range's
//! `activeBeforeInclusive` instant, and the account's default view — which
//! represents the account's own billing data rather than a resource with a
//! lifetime — is selected by EVERY range, so no window a caller asks about can
//! hide it.
//!
//! The rows differ along what the rule generalises over: where the window sits
//! relative to each view's creation instant (entirely before it, exactly on it,
//! after it, far in the future), how wide the window is, the type of the view
//! being selected, and whether a second filter or a page size is combined with
//! it.
//!
//! The harness (`call`, `emulator_with_engine`, …) is the one `billing_views.rs`
//! owns.

use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::billing_views::{call, emulator_with_engine, ACCOUNT_ID, PRIMARY_ARN};

/// Every `BillingViewType` the model declares, from
/// `smithy_query(query="shape", shape="BillingViewType")`: `PRIMARY`,
/// `BILLING_GROUP`, `CUSTOM`, `BILLING_TRANSFER`, `BILLING_TRANSFER_SHOWBACK`.
/// Only the first is active for as long as the account; the rest name resources
/// that came into existence at an instant.
const TIME_SCOPED_VIEW_TYPES: [&str; 4] = [
    "BILLING_GROUP",
    "CUSTOM",
    "BILLING_TRANSFER",
    "BILLING_TRANSFER_SHOWBACK",
];

/// A whole-second instant well in the past (2001-09-09T01:46:40Z), used as the
/// creation instant of the planted views so a window can sit exactly on it.
const PAST_SECS: i64 = 1_000_000_000;

/// One planted view: the type it carries and the instant it came into existence.
struct PlantedView {
    arn: String,
    view_type: &'static str,
    created_secs: i64,
}

/// Plant one view of `view_type`, created at `created_secs`, directly in state.
///
/// Only `CUSTOM` views can be made through the API, so the managed types the
/// rule also covers (a Billing Conductor billing group, a billing transfer) are
/// written to the store through the harness's planting helper, which clones the
/// account's `PRIMARY` view as the template so every unrelated field is a shape
/// the service itself produced.
fn plant_view(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    id: &str,
    view_type: &'static str,
    created_secs: i64,
) -> PlantedView {
    let arn = format!("arn:aws:billing::{ACCOUNT_ID}:billingview/{id}");
    crate::billing_views::plant_view(engine, &arn, |view| {
        view.name = format!("planted-{id}");
        view.description = None;
        view.billing_view_type = view_type.to_string();
        view.created_at = if created_secs >= 0 {
            UNIX_EPOCH + Duration::from_secs(created_secs.unsigned_abs())
        } else {
            UNIX_EPOCH - Duration::from_secs(created_secs.unsigned_abs())
        };
    });
    PlantedView {
        arn,
        view_type,
        created_secs,
    }
}

/// The ARNs an answer lists.
fn listed_arns(body: &Value) -> Vec<String> {
    body["billingViews"]
        .as_array()
        .expect("billingViews is a list")
        .iter()
        .map(|view| view["arn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// List with the given `activeTimeRange`, asserting the call succeeded.
async fn list_for_range(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    after: i64,
    before: i64,
) -> Vec<String> {
    let request = json!({
        "activeTimeRange": { "activeAfterInclusive": after, "activeBeforeInclusive": before }
    });
    let (status, listed) = call(router, "ListBillingViews", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListBillingViews, got {status} {listed:?}"
    );
    listed_arns(&listed)
}

#[tokio::test]
async fn the_default_view_is_listed_for_every_time_range() {
    let (router, engine) = emulator_with_engine();

    // One planted view per time-scoped type, all created at the same past
    // instant, so a window can sit before, exactly on, or after it.
    let planted: Vec<PlantedView> = TIME_SCOPED_VIEW_TYPES
        .iter()
        .enumerate()
        .map(|(index, view_type)| {
            plant_view(&engine, &format!("scoped-{index}"), view_type, PAST_SECS)
        })
        .collect();

    // (case, activeAfterInclusive, activeBeforeInclusive)
    let windows: Vec<(&str, i64, i64)> = vec![
        ("the epoch's first second, before anything existed", 0, 1),
        ("a one-day window ending a day before creation", PAST_SECS - 172_800, PAST_SECS - 86_400),
        ("a window ending one second before creation", PAST_SECS - 3_600, PAST_SECS - 1),
        ("a window ending exactly on the creation instant", PAST_SECS - 3_600, PAST_SECS),
        ("a window ending one second after creation", PAST_SECS - 3_600, PAST_SECS + 1),
        ("a one-month window well after creation", PAST_SECS + 86_400, PAST_SECS + 2_678_400),
        ("a degenerate window: one instant, long after creation", 2_000_000_000, 2_000_000_000),
        ("a window ending far in the future", 0, 4_000_000_000),
    ];

    for (case, after, before) in windows {
        let got = list_for_range(&router, case, after, before).await;
        assert!(
            got.contains(&PRIMARY_ARN.to_string()),
            "case {case:?}: the account's default view is listed for every range, got {got:?}"
        );

        // The same rule decides every other view: it is listed exactly when it
        // already existed at the window's end.
        let mut expected: Vec<String> = vec![PRIMARY_ARN.to_string()];
        for view in &planted {
            if view.created_secs <= before {
                expected.push(view.arn.clone());
            }
        }
        expected.sort();
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_historical_window_lists_the_default_view_alone() {
    let (router, _engine) = emulator_with_engine();

    // A view a caller makes through the API is CUSTOM and is created now, so a
    // window that ended before this process started cannot select it — while
    // the account's default view is selected all the same.
    let (status, created) = call(
        &router,
        "CreateBillingView",
        &json!({
            "name": "historical-window-view",
            "sourceViews": [PRIMARY_ARN],
            "clientToken": "historical-window",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"create a CUSTOM view\": got {status} {created:?}"
    );
    let custom_arn = created["arn"].as_str().unwrap_or_default().to_string();

    let got = list_for_range(&router, "a window that ended before boot", 0, 1).await;
    assert_eq!(
        got,
        vec![PRIMARY_ARN.to_string()],
        "case \"a window that ended before boot\": expected only the default view, got {got:?}"
    );

    // Combining the range with another filter does not revive the exemption for
    // a view the range excludes, nor withdraw it from the one it never applies
    // to.
    let combinations: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "a past window plus billingViewTypes=PRIMARY",
            json!({
                "billingViewTypes": ["PRIMARY"],
                "activeTimeRange": { "activeAfterInclusive": 0, "activeBeforeInclusive": 1 }
            }),
            vec![PRIMARY_ARN.to_string()],
        ),
        (
            "a past window plus billingViewTypes=CUSTOM",
            json!({
                "billingViewTypes": ["CUSTOM"],
                "activeTimeRange": { "activeAfterInclusive": 0, "activeBeforeInclusive": 1 }
            }),
            vec![],
        ),
        (
            "a past window plus a name search the default view matches",
            json!({
                "names": [{ "searchOption": "STARTS_WITH", "searchValue": "Primary" }],
                "activeTimeRange": { "activeAfterInclusive": 0, "activeBeforeInclusive": 1 }
            }),
            vec![PRIMARY_ARN.to_string()],
        ),
        (
            "a past window plus a name search only the CUSTOM view matches",
            json!({
                "names": [{ "searchOption": "STARTS_WITH", "searchValue": "historical" }],
                "activeTimeRange": { "activeAfterInclusive": 0, "activeBeforeInclusive": 1 }
            }),
            vec![],
        ),
        (
            "a past window plus maxResults=1",
            json!({
                "maxResults": 1,
                "activeTimeRange": { "activeAfterInclusive": 0, "activeBeforeInclusive": 1 }
            }),
            vec![PRIMARY_ARN.to_string()],
        ),
        (
            "a window reaching the present lists the CUSTOM view too",
            json!({
                "activeTimeRange": { "activeAfterInclusive": 0, "activeBeforeInclusive": 4_000_000_000i64 }
            }),
            {
                let mut all = vec![PRIMARY_ARN.to_string(), custom_arn.clone()];
                all.sort();
                all
            },
        ),
    ];

    for (case, request, expected) in combinations {
        let (status, listed) = call(&router, "ListBillingViews", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListBillingViews, got {status} {listed:?}"
        );
        let got = listed_arns(&listed);
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_filtered_list_leaves_the_store_as_it_found_it() {
    let (router, engine) = emulator_with_engine();
    let planted = plant_view(&engine, "scoped-unchanged", "BILLING_GROUP", PAST_SECS);

    let (_, before) = call(&router, "ListBillingViews", &json!({})).await;
    let unfiltered_before = listed_arns(&before);

    for (case, after, end) in [
        ("a window before every view", 0_i64, 1_i64),
        ("a window ending on the planted view", PAST_SECS - 1, PAST_SECS),
        ("a window ending far in the future", 0, 4_000_000_000),
    ] {
        let _ = list_for_range(&router, case, after, end).await;
    }

    let (_, after_listing) = call(&router, "ListBillingViews", &json!({})).await;
    let unfiltered_after = listed_arns(&after_listing);
    assert_eq!(
        unfiltered_after, unfiltered_before,
        "case \"a read changes nothing\": the unfiltered list must be unchanged, \
         got {unfiltered_after:?} after {unfiltered_before:?}"
    );
    assert!(
        unfiltered_after.contains(&planted.arn) && unfiltered_after.contains(&PRIMARY_ARN.to_string()),
        "case \"a read changes nothing\": expected both the planted {:?} view and the default \
         view to still be listed, got {unfiltered_after:?}",
        planted.view_type
    );
}
