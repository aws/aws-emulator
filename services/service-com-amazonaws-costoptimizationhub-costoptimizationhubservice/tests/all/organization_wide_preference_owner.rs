// SPDX-License-Identifier: CC0-1.0

//! Who the organization-wide `memberAccountDiscountVisibility` preference
//! belongs to, driven end-to-end over the wire (`awsJson1_0`) through the same
//! multi-service router the emulator boots.
//!
//! The rule under test: the preference is one setting of the organization rather
//! than of one account, so it belongs to the account that PAYS for the member
//! accounts' spend it is about — the MANAGEMENT account of that organization.
//! That account sets either value of the member
//! (`smithy_query(query="shape", shape="MemberAccountDiscountVisibility")`:
//! `All | None`), is reported it by a read, and so can reach
//! `savingsEstimationMode` = `AfterDiscounts`, which is available only while the
//! discounts are visible; every other caller is refused both, because neither
//! preference is its own.
//!
//! Which accounts an organization has is an Organizations fact this emulator
//! records none of, so the accepting arm's caller is recorded as its
//! organization's management account in the store
//! (`management_account_router`) and then drives nothing but wire calls;
//! `management_account_only_discount_visibility` owns the arm every client of a
//! freshly booted emulator meets, whose account manages no organization at all.
//!
//! `preference_gates` owns the two rules' tables of inputs; this suite is about
//! the account the rules are decided for.

use serde_json::{json, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;
use smithy_server_runtime::MultiServiceRouter;

use cost_optimization_hub::ops::types::shared_types::{
    MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES, PAYMENT_OPTION_VALUES, TERM_VALUES,
};
use cost_optimization_hub::state::{AFTER_DISCOUNTS, BEFORE_DISCOUNTS, DISCOUNT_VISIBILITY_NONE};

use crate::cost_optimization_hub::{
    assert_stated_rule_refusal, call, enrolled_router_with_discounts_visible,
    management_account_router, member_account_router, ok, preferences_record,
    DISCOUNT_VISIBILITY_ALL, MODE_MEMBER, VISIBILITY_MEMBER,
};

/// The `savingsPercentage` an answer reports, which `SummaryMetricsResult`
/// models as a String whichever kind of value it carries.
fn reported_percentage(payload: &Value) -> String {
    payload["metrics"]["savingsPercentage"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a savingsPercentage metric in {payload}"))
        .to_string()
}

/// Send one `UpdatePreferences` and require it accepted, answering the record
/// the write left.
async fn updated(router: &MultiServiceRouter, case: &str, request: &Value) -> Value {
    let (status, payload) = call(router, "UpdatePreferences", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the write accepted, got HTTP {status} with {payload}"
    );
    payload
}

/// The management account of an organization sets either value of
/// `memberAccountDiscountVisibility`: the write is accepted, echoed back, and
/// reported by a following `GetPreferences`.
///
/// The rows span the member's whole enum and the members it is sent beside, so
/// it is the member itself — and not its company — that the owner may set. Each
/// row starts from a fresh store, so a row's answer is about the request and not
/// about what another row left.
#[tokio::test]
async fn the_payer_of_the_member_accounts_spend_sets_the_discount_visibility_preference() {
    // (case, the other members sent beside the visibility)
    let companions: Vec<(&str, Value)> = vec![
        ("alone", json!({})),
        (
            "beside the savings estimation mode",
            json!({MODE_MEMBER: BEFORE_DISCOUNTS}),
        ),
        (
            "beside the commitment",
            json!({"preferredCommitment": {
                "term": TERM_VALUES[0],
                "paymentOption": PAYMENT_OPTION_VALUES[0],
            }}),
        ),
        (
            "beside both other members",
            json!({
                MODE_MEMBER: BEFORE_DISCOUNTS,
                "preferredCommitment": {"term": TERM_VALUES[1]},
            }),
        ),
    ];

    for visibility in MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES {
        for (companion, extra) in &companions {
            let case = format!("{visibility} {companion}");
            let router = management_account_router();

            let mut request = extra.clone();
            request
                .as_object_mut()
                .expect("the request is an object")
                .insert(
                    VISIBILITY_MEMBER.to_string(),
                    Value::String((*visibility).to_string()),
                );

            let written = updated(&router, &case, &request).await;
            assert_eq!(
                written[VISIBILITY_MEMBER].as_str(),
                Some(*visibility),
                "case {case:?}: expected the written value echoed, got {written}"
            );

            // The round trip: the preference the write stored is what a read
            // reports, and the write's own answer is that same record.
            let fetched = preferences_record(&router).await;
            assert_eq!(
                fetched[VISIBILITY_MEMBER].as_str(),
                Some(*visibility),
                "case {case:?}: expected the read to report the preference, got {fetched}"
            );
            assert_eq!(
                fetched, written,
                "case {case:?}: expected the write's answer to be the record a read reports, \
                 got {fetched} after {written}"
            );
        }
    }
}

/// The discount-adjusted savings estimation mode is reachable with nothing but
/// wire calls once the caller is the organization's management account: making
/// the discounts visible is what the mode needs, and the account that owns that
/// preference is the one asking for the mode.
///
/// The rows differ in how the pair is reached — in two requests, in one, and the
/// two states of the visibility that leave the mode unavailable — and each row
/// asserts the record a following read reports, so the answer is the stored
/// state and not the echo of a request.
#[tokio::test]
async fn the_discounted_mode_is_reachable_over_the_wire() {
    // (case, the requests to send before the mode, whether the mode is then
    // available)
    let cases: Vec<(&str, Vec<Value>, bool)> = vec![
        ("no visibility set at all", vec![], false),
        (
            "visibility None",
            vec![json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_NONE})],
            false,
        ),
        (
            "visibility All",
            vec![json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_ALL})],
            true,
        ),
        (
            "visibility All, then back to None",
            vec![
                json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_ALL}),
                json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_NONE}),
            ],
            false,
        ),
        (
            "visibility None, then All",
            vec![
                json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_NONE}),
                json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_ALL}),
            ],
            true,
        ),
    ];

    for (case, setup, available) in &cases {
        let router = management_account_router();
        for request in setup {
            updated(&router, case, request).await;
        }

        let ask = json!({MODE_MEMBER: AFTER_DISCOUNTS});
        if *available {
            let written = updated(&router, case, &ask).await;
            assert_eq!(
                written[MODE_MEMBER].as_str(),
                Some(AFTER_DISCOUNTS),
                "case {case:?}: expected the discounted mode stored, got {written}"
            );
            let fetched = preferences_record(&router).await;
            assert_eq!(
                fetched[MODE_MEMBER].as_str(),
                Some(AFTER_DISCOUNTS),
                "case {case:?}: expected the read to report the discounted mode, got {fetched}"
            );
        } else {
            let (status, payload) = call(&router, "UpdatePreferences", &ask).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {payload}"
            );
            assert_stated_rule_refusal(&payload, case, MODE_MEMBER);
            let fetched = preferences_record(&router).await;
            assert_eq!(
                fetched[MODE_MEMBER].as_str(),
                Some(BEFORE_DISCOUNTS),
                "case {case:?}: expected the stored mode untouched, got {fetched}"
            );
        }
    }

    // And both members in one request: the pair the request leaves is the one
    // the rule judges, so enabling the visibility and asking for the mode
    // together is accepted from a fresh store.
    let router = management_account_router();
    let together = updated(
        &router,
        "both members at once",
        &json!({
            VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_ALL,
            MODE_MEMBER: AFTER_DISCOUNTS,
        }),
    )
    .await;
    assert_eq!(
        together[MODE_MEMBER].as_str(),
        Some(AFTER_DISCOUNTS),
        "case both members at once: expected the discounted mode stored, got {together}"
    );
    assert_eq!(
        together[VISIBILITY_MEMBER].as_str(),
        Some(DISCOUNT_VISIBILITY_ALL),
        "case both members at once: expected the visibility stored, got {together}"
    );
}

/// What the mode changes downstream: the organization's management account, which
/// has made the discounts visible and moved to the discounted mode, is reported a
/// `savingsPercentage` FIGURE, where the same request under the undiscounted mode
/// carries the description of why the metric is unsupported.
///
/// The rows are the grouping dimensions the answer can be asked for, so the
/// metric is shown to follow the preference and not the grouping.
#[tokio::test]
async fn the_savings_percentage_metric_follows_the_mode_the_management_account_reaches() {
    let router = enrolled_router_with_discounts_visible().await;

    for group_by in ["Region", "ActionType", "ResourceType"] {
        let request = json!({"groupBy": group_by, "metrics": ["SavingsPercentage"]});
        let before = reported_percentage(&ok(&router, "ListRecommendationSummaries", &request).await);
        assert!(
            before.parse::<f64>().is_err() && before.contains("supported"),
            "case groupBy {group_by} before the move: expected the description of an \
             unsupported metric, got {before:?}"
        );
    }

    // The wire calls that move the account to the discounted mode: the
    // visibility the organization's management account owns (written again
    // here, which is the state it is already in), and then the mode it gates.
    updated(
        &router,
        "make the discounts visible",
        &json!({VISIBILITY_MEMBER: DISCOUNT_VISIBILITY_ALL}),
    )
    .await;
    updated(
        &router,
        "move to the discounted mode",
        &json!({MODE_MEMBER: AFTER_DISCOUNTS}),
    )
    .await;

    for group_by in ["Region", "ActionType", "ResourceType"] {
        let request = json!({"groupBy": group_by, "metrics": ["SavingsPercentage"]});
        let payload = ok(&router, "ListRecommendationSummaries", &request).await;
        let reported = reported_percentage(&payload);
        let figure = reported.parse::<f64>().unwrap_or_else(|_| {
            panic!("case groupBy {group_by}: expected a percentage, got {reported:?} in {payload}")
        });
        assert!(
            figure > 0.0 && figure <= 100.0,
            "case groupBy {group_by}: expected a percentage of the summarised spend, got {figure}"
        );
    }
}

/// The other arm of the same rule: an account whose organization is managed by
/// ANOTHER account pays none of the spend the preference is about, so it may set
/// neither the visibility — whichever value it asks for — nor the mode that
/// preference gates, and a read never reports the member to it.
#[tokio::test]
async fn an_account_another_account_pays_for_is_refused_both_preferences() {
    let router = member_account_router();

    for visibility in MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES {
        let case = format!("member account asks for {visibility}");
        let (status, payload) = call(
            &router,
            "UpdatePreferences",
            &json!({VISIBILITY_MEMBER: visibility}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_stated_rule_refusal(&payload, &case, VISIBILITY_MEMBER);

        let fetched = preferences_record(&router).await;
        assert!(
            fetched[VISIBILITY_MEMBER].is_null(),
            "case {case:?}: expected the organization's preference left out of the report, \
             got {fetched}"
        );
    }

    // And the mode that preference gates: the discounts the organization's
    // management account has not made visible are not visible to this caller.
    let (status, payload) = call(
        &router,
        "UpdatePreferences",
        &json!({MODE_MEMBER: AFTER_DISCOUNTS}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case member account asks for the discounted mode: expected HTTP 400, got {status} \
         with {payload}"
    );
    assert_stated_rule_refusal(
        &payload,
        "member account asks for the discounted mode",
        MODE_MEMBER,
    );
}
