// SPDX-License-Identifier: CC0-1.0

//! What `DescribeAlarms` answers a `NextToken` it cannot read a position out of.
//!
//! THE RULE: a continuation token NAMES a position the enumeration reached — "The token returned
//! by a previous call to indicate that there is more data available."
//! (`DescribeAlarmsInput$NextToken`), "The token that marks the start of the next batch of
//! returned results." (`DescribeAlarmsOutput$NextToken`) — and this listing refuses none: a value
//! it cannot read a position out of names no position of this enumeration, so it names nothing
//! and is IGNORED. The page served is the FIRST one of the selection the request's other filters
//! name — exactly what the same request without the member is served, token and all.
//!
//! What a client gets from it: a paginator resuming with a token it kept from an earlier walk, or
//! passing one through from another call, is served the listing it asked for rather than an empty
//! page that hides every alarm it holds.
//!
//! The rows span what the rule generalises over: the FORM the unreadable value takes (a word, text
//! that is no cursor, a hexadecimal cursor whose bytes are no text, an opaque base64 blob of
//! another service's form, a minted cursor truncated, and one with text appended) crossed with the
//! selection the request names (no filter at all, an exact name list, a name list naming an alarm
//! that does not exist, a name prefix, a state filter every alarm matches, a name list with a
//! state filter none of them matches, an action prefix, and a name prefix cut by a page size,
//! where the first page owes a token of its own) — and, beside them, a state filter that selects
//! part of a name list, and a cursor this listing really minted, which resumes where it points.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, ok};

/// The ARN prefix the alarms of this suite send their notifications under.
const TOPICS: &str = "arn:aws:sns:us-east-1:000000000000:";

/// One metric alarm notifying `topic`, with no datapoint published for the metric it watches —
/// so its evaluated state is the INSUFFICIENT_DATA every alarm of this suite reports, whatever
/// else the clock does, and no row depends on how fast the host runs.
async fn given_alarm(router: &MultiServiceRouter, name: &str, topic: &str) {
    ok(
        router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": "MyApp/StaleToken",
            "MetricName": "Latency",
            "Statistic": "Average",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
            "ActionsEnabled": false,
            "AlarmActions": [format!("{TOPICS}{topic}")],
        }),
    )
    .await;
}

/// The alarm names an answer reports, in the order it reported them.
fn listed(described: &Value) -> Vec<String> {
    described
        .get("MetricAlarms")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|alarm| alarm.get("AlarmName").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect()
}

/// The token an answer owes the page after it, if any.
fn next_token(described: &Value) -> Option<String> {
    described
        .get("NextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The same request with `token` added as its `NextToken`.
fn resumed(request: &Value, token: &str) -> Value {
    let mut request = request.clone();
    request["NextToken"] = json!(token);
    request
}

/// Each form a value this listing cannot read a cursor out of takes, `minted` being a cursor the
/// listing really issued so the broken forms are that cursor damaged rather than values written
/// out by hand.
fn unreadable_tokens(minted: &str) -> Vec<(String, String)> {
    vec![
        (
            "a word no cursor could be".to_string(),
            "garbage-not-a-real-token".to_string(),
        ),
        (
            "text that is no cursor".to_string(),
            "not-a-cursor".to_string(),
        ),
        (
            "a cursor whose bytes are no text".to_string(),
            "fffe".to_string(),
        ),
        (
            "an opaque blob of another service's form".to_string(),
            "eyJuZXh0IjoiYWxhcm0ifQ==".to_string(),
        ),
        (
            "a minted cursor truncated".to_string(),
            minted
                .get(..minted.len().saturating_sub(1))
                .unwrap_or_default()
                .to_string(),
        ),
        (
            "a minted cursor with text appended".to_string(),
            format!("{minted}zz"),
        ),
    ]
}

#[tokio::test]
async fn a_next_token_this_listing_cannot_read_is_ignored_and_the_first_page_served() {
    let router = build_router();
    for (name, topic) in [
        ("other-quiet", "delta"),
        ("stale-first", "beta"),
        ("stale-second", "gamma"),
        ("stale-third", "alpha"),
    ] {
        given_alarm(&router, name, topic).await;
    }

    let minted = next_token(
        &ok(
            &router,
            "DescribeAlarms",
            &json!({"AlarmNamePrefix": "stale-", "MaxRecords": 1}),
        )
        .await,
    )
    .expect("a page of one of three alarms owes a token");
    let tokens = unreadable_tokens(&minted);

    // (case, the request WITHOUT a token, the alarms that selection holds)
    let selections: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filter at all",
            json!({}),
            vec!["other-quiet", "stale-first", "stale-second", "stale-third"],
        ),
        (
            "an exact name list",
            json!({"AlarmNames": ["stale-third", "stale-first"]}),
            vec!["stale-first", "stale-third"],
        ),
        (
            "a name list naming an alarm that does not exist",
            json!({"AlarmNames": ["stale-third", "no-such-alarm"]}),
            vec!["stale-third"],
        ),
        (
            "a name prefix",
            json!({"AlarmNamePrefix": "stale-"}),
            vec!["stale-first", "stale-second", "stale-third"],
        ),
        (
            "a state filter every alarm matches",
            json!({"StateValue": "INSUFFICIENT_DATA"}),
            vec!["other-quiet", "stale-first", "stale-second", "stale-third"],
        ),
        (
            "a name list with a state filter none of them matches",
            json!({"AlarmNames": ["stale-third", "stale-first"], "StateValue": "ALARM"}),
            vec![],
        ),
        (
            "an action prefix",
            json!({"ActionPrefix": format!("{TOPICS}de")}),
            vec!["other-quiet"],
        ),
        (
            "a name prefix cut by a page size",
            json!({"AlarmNamePrefix": "stale-", "MaxRecords": 2}),
            vec!["stale-first", "stale-second"],
        ),
    ];

    for (selection, request, expected) in selections {
        let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
        // What the selection answers with no token at all, which is what the rows below show
        // the token does NOT hand back a second time.
        let baseline = ok(&router, "DescribeAlarms", &request).await;
        assert_eq!(
            listed(&baseline),
            expected,
            "case {selection:?}: expected the selection {expected:?}, got {baseline}"
        );

        for (form, token) in &tokens {
            let case = format!("{selection} resumed with {form} ({token:?})");
            let answered = ok(&router, "DescribeAlarms", &resumed(&request, token)).await;
            assert_eq!(
                listed(&answered),
                expected,
                "case {case:?}: expected the first page of that selection {expected:?}, got \
                 {answered}"
            );
            assert_eq!(
                next_token(&answered),
                next_token(&baseline),
                "case {case:?}: expected the first page's own token {:?}, got {answered}",
                next_token(&baseline)
            );
            // Nothing was consumed: the same request answers the same page again, and the
            // selection itself is still there to be read without a token.
            let again = ok(&router, "DescribeAlarms", &resumed(&request, token)).await;
            assert_eq!(
                listed(&again),
                expected,
                "case {case:?}: expected the same page twice, got {again}"
            );
            let untouched = ok(&router, "DescribeAlarms", &request).await;
            assert_eq!(
                listed(&untouched),
                expected,
                "case {case:?}: the selection must survive the reading, got {untouched}"
            );
        }
    }
}

#[tokio::test]
async fn a_next_token_this_listing_cannot_read_is_ignored_over_a_filtered_selection() {
    // The recorded shape of the rule: a name list naming two alarms in DIFFERENT states, narrowed
    // by `StateValue`. "Specify this parameter to receive information only about alarms that are
    // currently in the state that you specify." (`DescribeAlarmsInput$StateValue`) — the selection
    // the filters name is what the read answers, with the unreadable token ignored.
    //
    // The states are forced rather than evaluated: "Temporarily sets the state of an alarm for
    // testing purposes." (`SetAlarmState`) — a metric alarm returns to its evaluated state
    // shortly after, so this row reads the listing while the forced states stand and keeps its
    // readings few.
    let router = build_router();
    for (name, topic) in [("filtered-ok", "alpha"), ("filtered-alarm", "beta")] {
        given_alarm(&router, name, topic).await;
    }
    for (name, state) in [("filtered-ok", "OK"), ("filtered-alarm", "ALARM")] {
        ok(
            &router,
            "SetAlarmState",
            &json!({"AlarmName": name, "StateValue": state, "StateReason": "seeded"}),
        )
        .await;
    }

    let request = json!({
        "AlarmNames": ["filtered-ok", "filtered-alarm"],
        "StateValue": "OK",
    });
    let expected = vec!["filtered-ok".to_string()];
    let baseline = ok(&router, "DescribeAlarms", &request).await;
    assert_eq!(
        listed(&baseline),
        expected,
        "case \"no token\": expected the OK alarm of the list, got {baseline}"
    );
    for (form, token) in [
        ("a word no cursor could be", "garbage-not-a-real-token"),
        ("text that is no cursor", "not-a-cursor"),
        ("an opaque blob of another service's form", "eyJuZXh0IjoiYSJ9"),
    ] {
        let answered = ok(&router, "DescribeAlarms", &resumed(&request, token)).await;
        assert_eq!(
            (listed(&answered), next_token(&answered)),
            (expected.clone(), None),
            "case {form:?}: expected the narrowed selection's first page, got {answered}"
        );
    }
    // The narrowed selection is still what the same request without a token reports.
    let untouched = ok(&router, "DescribeAlarms", &request).await;
    assert_eq!(
        listed(&untouched),
        expected,
        "the narrowed selection must survive the readings, got {untouched}"
    );
}

#[tokio::test]
async fn a_cursor_this_listing_minted_still_resumes_where_it_points() {
    // The other half of the rule: a token that IS readable names a position, so the walk it
    // belongs to advances rather than restarting. Rows differ along the page size the walk uses.
    let router = build_router();
    let names = ["walk-a", "walk-b", "walk-c"];
    for name in names {
        given_alarm(&router, name, "alpha").await;
    }

    for page_size in [1_i64, 2] {
        let case = format!("pages of {page_size}");
        let mut delivered: Vec<String> = vec![];
        let mut token: Option<String> = None;
        for _ in 0..=names.len() {
            let mut request = json!({"AlarmNamePrefix": "walk-", "MaxRecords": page_size});
            if let Some(token) = token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let page = ok(&router, "DescribeAlarms", &request).await;
            delivered.extend(listed(&page));
            token = next_token(&page);
            if token.is_none() {
                break;
            }
        }
        let expected: Vec<String> = names.iter().map(ToString::to_string).collect();
        assert_eq!(
            delivered, expected,
            "case {case:?}: expected the walk to deliver each alarm exactly once, got \
             {delivered:?}"
        );
    }
}
