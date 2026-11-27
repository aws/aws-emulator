// SPDX-License-Identifier: CC0-1.0

//! What a continuation token means to a paginated read of this service, including a token this
//! service did not mint.
//!
//! The rule under test: **a `NextToken` names a POSITION a walk reached, and a listing that
//! serves such a token rather than refusing it resumes AT that position — never at the
//! beginning** — so a continuation delivers only the items after the ones already delivered,
//! while a value this service can read NO cursor out of names no position at all and is
//! IGNORED: the page served for it is the FIRST one, the same page the identical request
//! without the member is served (`DescribeAlarmsInput$NextToken` "The token returned by a
//! previous call to indicate that there is more data available.",
//! `ListAlarmMuteRulesOutput$NextToken` "If this field is absent, there are no more results to
//! retrieve."). An EMPTY `NextToken` is no token at all: the only page it can name is the
//! first.
//!
//! The rows differ along what the rule generalises over: which listing is walked (the alarm
//! listing narrowed by a name prefix, the same listing asked for a composite alarm's children,
//! and the alarm mute rule listing), the form the token takes (one this service minted, a value
//! that is no cursor at all, a value whose bytes are not text, an opaque blob of the form AWS
//! carries a token in, the empty string, and no token at all), and how many items the listing
//! holds beside the page. The recorded case — a prefix-scoped alarm listing continued with a
//! token it cannot resume from — is one row among them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The mute-rule request builders this suite needs already exist beside it, for the same
// resource: the sibling suites own them and this one writes a rule through the same helpers
// rather than a second copy.
use crate::alarm_mute_rules::{listed_arns, put_request, with_members};
use crate::harness::{build_router, metric_alarm_request, ok};

/// The prefix every alarm this suite writes shares, so a listing narrowed by it holds exactly
/// the alarms the row created.
const PREFIX: &str = "cursor-alarm-";

/// A `NextToken` value the service did not mint, in each form one can take.
///
/// The first is no cursor at all; the second reads as a hexadecimal cursor whose bytes are not
/// text; the third is the padded base64 blob AWS carries its own tokens in. No position of this
/// enumeration can be read out of any of them, so a listing that serves such a value ignores it
/// and answers the first page.
const FOREIGN_TOKENS: [(&str, &str); 3] = [
    ("a value that is no cursor at all", "not-a-cursor"),
    ("a cursor whose bytes are not text", "fffe"),
    (
        "an opaque blob of the form AWS mints a token in",
        "eyJuZXh0IjoiYWxhcm0ifQ==",
    ),
];

/// The alarm names a `DescribeAlarms` answer reports, in the order it reported them.
fn listed_alarms(listed: &Value) -> Vec<String> {
    listed["MetricAlarms"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|alarm| alarm.get("AlarmName").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect()
}

/// The token an answer owes the next page, if any.
fn next_token(listed: &Value) -> Option<&str> {
    listed.get("NextToken").and_then(Value::as_str)
}

/// Write `count` alarms whose names sort in the order they were created.
async fn given_alarms(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let mut names: Vec<String> = vec![];
    for index in 0..count {
        let name = format!("{PREFIX}{index:03}");
        ok(router, "PutMetricAlarm", &metric_alarm_request(&name)).await;
        names.push(name);
    }
    names
}

#[tokio::test]
async fn a_continued_alarm_listing_delivers_only_the_alarms_after_the_ones_already_delivered() {
    // (case, how many alarms the prefix holds, the page size each call asks for)
    //
    // Rows differ along the shape of the walk: a listing whose last page is exactly full (the
    // recorded case, where the continuation has nothing left to deliver), one with a part-full
    // page left, and one walked an alarm at a time.
    let rows: [(&str, usize, i64); 4] = [
        ("two alarms in pages of two", 2, 2),
        ("three alarms in pages of two", 3, 2),
        ("three alarms one at a time", 3, 1),
        ("one alarm in a page of two", 1, 2),
    ];
    for (case, total, page_size) in rows {
        let router = build_router();
        let names = given_alarms(&router, total).await;

        // Walk the whole listing: every page delivers the alarms after the ones before it, and
        // no alarm is delivered twice.
        let mut delivered: Vec<String> = vec![];
        let mut token: Option<String> = None;
        for page in 0..=total {
            let mut request = json!({"AlarmNamePrefix": PREFIX, "MaxRecords": page_size});
            if let Some(token) = token.as_deref() {
                request = with_members(request, json!({"NextToken": token}));
            }
            let listed = ok(&router, "DescribeAlarms", &request).await;
            let reported = listed_alarms(&listed);
            assert!(
                reported.len() <= usize::try_from(page_size).unwrap_or_default(),
                "case {case:?}: page {page} may hold at most {page_size} alarms, got {reported:?}"
            );
            delivered.extend(reported);
            token = next_token(&listed).map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            delivered, names,
            "case {case:?}: expected the walk to deliver each of the {total} alarms exactly \
             once in name order, got {delivered:?}"
        );
        assert!(
            token.is_none(),
            "case {case:?}: expected the walk to end with no token, got {token:?}"
        );
    }
}

#[tokio::test]
async fn a_token_the_alarm_listing_cannot_read_is_ignored_and_the_first_page_served() {
    let router = build_router();
    let names = given_alarms(&router, 2).await;
    let first = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmNamePrefix": PREFIX, "MaxRecords": 2}),
    )
    .await;
    assert_eq!(
        listed_alarms(&first),
        names,
        "the first page must deliver both alarms, got {first}"
    );

    for (case, token) in FOREIGN_TOKENS {
        let listed = ok(
            &router,
            "DescribeAlarms",
            &json!({"AlarmNamePrefix": PREFIX, "MaxRecords": 2, "NextToken": token}),
        )
        .await;
        // No position of this enumeration can be read out of that value, so it names none and
        // is ignored: the page served is the one this request is served without it.
        assert_eq!(
            listed_alarms(&listed),
            names,
            "case {case:?}: expected the first page {names:?}, got {listed}"
        );
        assert_eq!(
            next_token(&listed),
            next_token(&first),
            "case {case:?}: expected the first page's own token {:?}, got {listed}",
            next_token(&first)
        );
        // The three alarm lists the read reports are PRESENT, which is what makes the page
        // readable at all.
        for member in ["CompositeAlarms", "MetricAlarms", "LogAlarms"] {
            assert!(
                listed.get(member).and_then(Value::as_array).is_some(),
                "case {case:?}: expected {member} present, got {listed}"
            );
        }
    }

    // An EMPTY `NextToken` is no token at all — the member is a plain string, so an empty one
    // names no walk — and asks for the first page, as a request carrying none does.
    for (case, request) in [
        (
            "an empty NextToken",
            json!({"AlarmNamePrefix": PREFIX, "MaxRecords": 2, "NextToken": ""}),
        ),
        (
            "no NextToken at all",
            json!({"AlarmNamePrefix": PREFIX, "MaxRecords": 2}),
        ),
    ] {
        let listed = ok(&router, "DescribeAlarms", &request).await;
        assert_eq!(
            listed_alarms(&listed),
            names,
            "case {case:?}: expected the first page, got {listed}"
        );
    }
}

#[tokio::test]
async fn a_token_a_relationship_walk_cannot_read_is_ignored_and_the_first_page_served() {
    let router = build_router();
    let children = given_alarms(&router, 2).await;
    let parent = "cursor-parent";
    ok(
        &router,
        "PutCompositeAlarm",
        &json!({
            "AlarmName": parent,
            "AlarmRule": format!("ALARM(\"{}\") OR ALARM(\"{}\")", children[0], children[1]),
        }),
    )
    .await;

    let first = ok(
        &router,
        "DescribeAlarms",
        &json!({"ChildrenOfAlarmName": parent, "MaxRecords": 2}),
    )
    .await;
    assert_eq!(
        listed_alarms(&first),
        children,
        "the children listing must deliver both children, got {first}"
    );

    for (case, token) in FOREIGN_TOKENS {
        let listed = ok(
            &router,
            "DescribeAlarms",
            &json!({"ChildrenOfAlarmName": parent, "MaxRecords": 2, "NextToken": token}),
        )
        .await;
        assert_eq!(
            listed_alarms(&listed),
            children,
            "case {case:?}: expected the first page of the children walk {children:?}, got \
             {listed}"
        );
        assert_eq!(
            next_token(&listed),
            next_token(&first),
            "case {case:?}: expected the first page's own token {:?}, got {listed}",
            next_token(&first)
        );
    }

    // And the same request with no token is still answered the first page, so the rule is
    // about the TOKEN rather than about the relationship filter.
    let again = ok(
        &router,
        "DescribeAlarms",
        &json!({"ChildrenOfAlarmName": parent, "MaxRecords": 2}),
    )
    .await;
    assert_eq!(
        listed_alarms(&again),
        children,
        "a request carrying no token is still answered the first page, got {again}"
    );
}

#[tokio::test]
async fn a_token_the_mute_rule_listing_cannot_read_is_ignored_and_the_first_page_served() {
    let router = build_router();
    // The same rule, read through the OTHER listing of this service that serves a token it
    // cannot read: one rule decides what such a token names for both.
    for index in 0..2 {
        ok(
            &router,
            "PutAlarmMuteRule",
            &with_members(
                put_request(&format!("cursor-rule-{index:03}"), "cron(* * * * *)", "PT1H"),
                json!({}),
            ),
        )
        .await;
    }
    let first = ok(&router, "ListAlarmMuteRules", &json!({"MaxRecords": 2})).await;
    let both = listed_arns(&first);
    assert_eq!(
        both.len(),
        2,
        "the first page must deliver both rules, got {first}"
    );

    for (case, token) in FOREIGN_TOKENS {
        let listed = ok(
            &router,
            "ListAlarmMuteRules",
            &json!({"MaxRecords": 2, "NextToken": token}),
        )
        .await;
        assert_eq!(
            listed_arns(&listed),
            both,
            "case {case:?}: expected the first page {both:?}, got {listed}"
        );
        assert_eq!(
            next_token(&listed),
            next_token(&first),
            "case {case:?}: expected the first page's own token {:?}, got {listed}",
            next_token(&first)
        );
    }

    // A request carrying no token is still answered the first page.
    let again = ok(&router, "ListAlarmMuteRules", &json!({"MaxRecords": 2})).await;
    assert_eq!(
        listed_arns(&again),
        both,
        "a request carrying no token is still answered the first page, got {again}"
    );
}
