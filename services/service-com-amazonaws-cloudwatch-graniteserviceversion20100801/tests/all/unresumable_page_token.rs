// SPDX-License-Identifier: CC0-1.0

//! What a `NextToken` a listing cannot read a position out of asks for, and what one naming a
//! position past the last item asks for.
//!
//! THE RULE: a continuation token NAMES a position a listing's enumeration reached — "The token
//! returned by a previous call to indicate that there is more data available."
//! (`DescribeAlarmsInput$NextToken`), "The token that marks the start of the next batch of
//! returned results." (`DescribeAlarmsOutput$NextToken`), "The token returned from a previous call
//! to indicate where to continue retrieving results." (`ListAlarmMuteRulesInput$NextToken`) — so
//! for **the listings of this service that SERVE such a token rather than refusing it, a value no
//! position of this enumeration can be read out of names no position at all and is IGNORED: the
//! page served is the FIRST one, exactly what the same request without the member is served**. A
//! token that IS readable is answered at the position it names, which for a key no item sits at or
//! after is the end of the listing: the exhausted page, every item member present and EMPTY with
//! no `NextToken`.
//!
//! The rows span what the rule generalises over: both listings of this service that serve such a
//! token, the FORM an unreadable value takes (a word, a hexadecimal cursor whose bytes are no
//! text, a base64 blob of the form another service's cursor travels in, a minted cursor with a
//! character cut off, and one with text appended), the page size the request asks for (none at
//! all, one, and more than the listing holds), and how many items the listing holds (one, and
//! several) — beside the readable values: the empty string and an absent member, which are no
//! token at all, a cursor sorting before every item, a cursor this service really minted, which
//! resumes where it points, and a cursor naming a key past every item, which is the end.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::alarm_mute_rules::{listed_arns, put_request, with_members};
use crate::harness::{build_router, metric_alarm_request, ok};

/// The prefix every alarm this suite writes shares, so a listing narrowed by it holds exactly
/// the alarms a row created.
const PREFIX: &str = "cursor-page-alarm-";

/// The prefix every mute rule this suite writes shares. A mute rule listing is not narrowed by a
/// name, so this only decides where the rules sort relative to the cursors below.
const RULE_PREFIX: &str = "cursor-page-rule-";

/// The values a row offers as a `NextToken` this service can read NO position out of.
///
/// The first three are values of no form this service mints; `cursor` is a token the listing really
/// minted, so the next two are that cursor broken rather than a value written out by hand (they are
/// offered only where the listing owed a cursor at all, since a listing that fits in one page mints
/// none to break).
fn unreadable_tokens(cursor: &str) -> Vec<(String, String)> {
    let mut rows = vec![
        ("a word".to_string(), "not-a-cursor".to_string()),
        (
            "a hexadecimal cursor whose bytes are no text".to_string(),
            "fffe".to_string(),
        ),
        (
            "a base64 blob of another service's form".to_string(),
            "eyJuZXh0IjoiYWxhcm0ifQ==".to_string(),
        ),
    ];
    if !cursor.is_empty() {
        rows.push((
            "a minted cursor with its last character cut off".to_string(),
            cursor
                .get(..cursor.len().saturating_sub(1))
                .unwrap_or_default()
                .to_string(),
        ));
        rows.push((
            "a minted cursor with text appended".to_string(),
            format!("{cursor}zz"),
        ));
    }
    rows
}

/// A cursor of this service's own form (`state::encode_page_token` is hexadecimal) naming a key
/// that sorts PAST every item under `item_prefix`: `~` is the last printable ASCII character, so
/// no name this suite writes sorts at or after it.
fn past_every_item(item_prefix: &str) -> String {
    ::hex::encode(format!("{item_prefix}~").as_bytes())
}

/// A cursor of this service's own form naming a key that sorts BEFORE every item under
/// `item_prefix` — a position the enumeration DOES hold, namely its beginning.
fn before_every_item(item_prefix: &str) -> String {
    ::hex::encode(item_prefix.as_bytes())
}

/// The alarm names a `DescribeAlarms` answer reports.
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

/// The token an answer owes the page after it, if any.
fn next_token(listed: &Value) -> Option<String> {
    listed
        .get("NextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string)
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

/// Write `count` mute rules whose names sort in the order they were created.
async fn given_rules(router: &MultiServiceRouter, count: usize) {
    for index in 0..count {
        ok(
            router,
            "PutAlarmMuteRule",
            &with_members(
                put_request(
                    &format!("{RULE_PREFIX}{index:03}"),
                    "cron(* * * * *)",
                    "PT1H",
                ),
                json!({}),
            ),
        )
        .await;
    }
}

#[tokio::test]
async fn a_token_the_alarm_listing_cannot_read_is_ignored_and_the_first_page_served() {
    // (case, how many alarms the prefix holds, the page size the rows ask for)
    let rows: [(&str, usize, Option<i64>); 4] = [
        ("one alarm, no page size asked for", 1, None),
        ("three alarms, no page size asked for", 3, None),
        ("three alarms in pages of one", 3, Some(1)),
        ("three alarms in a page wider than the listing", 3, Some(50)),
    ];

    for (case, total, page_size) in rows {
        let router = build_router();
        let names = given_alarms(&router, total).await;

        // The page this request is served when it carries NO token: the FIRST one, which is what
        // a token naming no position at all asks for too.
        let mut baseline_request = json!({"AlarmNamePrefix": PREFIX});
        if let Some(page_size) = page_size {
            baseline_request["MaxRecords"] = json!(page_size);
        }
        let baseline = ok(&router, "DescribeAlarms", &baseline_request).await;
        let baseline_alarms = listed_alarms(&baseline);
        let baseline_token = next_token(&baseline);

        // A cursor the listing really minted, taken from a page of one so one is owed wherever
        // an alarm is left over beyond that page.
        let minted = ok(
            &router,
            "DescribeAlarms",
            &json!({"AlarmNamePrefix": PREFIX, "MaxRecords": 1}),
        )
        .await;
        let cursor = next_token(&minted).unwrap_or_default();

        for (token_case, token) in unreadable_tokens(&cursor) {
            let mut request = json!({"AlarmNamePrefix": PREFIX, "NextToken": token});
            if let Some(page_size) = page_size {
                request["MaxRecords"] = json!(page_size);
            }
            let listed = ok(&router, "DescribeAlarms", &request).await;
            let case = format!("{case} with {token_case} ({token:?})");
            // No position of this enumeration can be read out of that value, so it names none
            // and is ignored: the page served is the one this request is served without it.
            assert_eq!(
                listed_alarms(&listed),
                baseline_alarms,
                "case {case:?}: expected the first page {baseline_alarms:?}, got {listed}"
            );
            assert_eq!(
                next_token(&listed),
                baseline_token,
                "case {case:?}: expected the first page's own token {baseline_token:?}, got \
                 {listed}"
            );
            // Asking for it again answers the same page: a read of a position consumes nothing.
            let again = ok(&router, "DescribeAlarms", &request).await;
            assert_eq!(
                listed_alarms(&again),
                baseline_alarms,
                "case {case:?}: expected the same first page twice, got {again}"
            );
        }

        // A READABLE cursor naming a key past every alarm is the position it says: the end, where
        // every one of the three lists this read reports is present and EMPTY and no token is owed.
        let mut request =
            json!({"AlarmNamePrefix": PREFIX, "NextToken": past_every_item(PREFIX)});
        if let Some(page_size) = page_size {
            request["MaxRecords"] = json!(page_size);
        }
        let exhausted = ok(&router, "DescribeAlarms", &request).await;
        for member in ["MetricAlarms", "CompositeAlarms", "LogAlarms"] {
            assert_eq!(
                exhausted.get(member).and_then(Value::as_array),
                Some(&Vec::new()),
                "case {case:?}: expected {member} present and empty past the last alarm, got \
                 {exhausted}"
            );
        }
        assert_eq!(
            next_token(&exhausted),
            None,
            "case {case:?}: an exhausted page owes no token, got {exhausted}"
        );

        // The alarms themselves are untouched: a request carrying no token still reports every
        // one of them, so the pages above answered the POSITION rather than a broken store.
        let whole = ok(
            &router,
            "DescribeAlarms",
            &json!({"AlarmNamePrefix": PREFIX}),
        )
        .await;
        assert_eq!(
            listed_alarms(&whole),
            names,
            "case {case:?}: expected the whole listing to survive, got {whole}"
        );

        // The positions the enumeration DOES hold are served the items at and after them: the
        // beginning, which is what no token at all and the empty string ask for and what a cursor
        // sorting before every alarm asks for too, and the alarm a minted cursor names.
        let mut beginning = vec![
            ("no NextToken at all", None),
            ("an empty NextToken", Some(String::new())),
            (
                "a cursor sorting before every alarm",
                Some(before_every_item(PREFIX)),
            ),
        ];
        if !cursor.is_empty() {
            beginning.push(("the cursor the listing minted", Some(cursor.clone())));
        }
        for (token_case, token) in beginning {
            let mut request = json!({"AlarmNamePrefix": PREFIX});
            if let Some(page_size) = page_size {
                request["MaxRecords"] = json!(page_size);
            }
            if let Some(token) = token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let listed = ok(&router, "DescribeAlarms", &request).await;
            let case = format!("{case} with {token_case} ({token:?})");
            let expected: Vec<String> = if token_case == "the cursor the listing minted" {
                // That cursor names the alarm after the page of one it was minted by, so the
                // page it asks for starts there and is cut at the size this row asks for.
                let after_first: Vec<String> = names.get(1..).unwrap_or_default().to_vec();
                match page_size {
                    None => after_first,
                    Some(page_size) => after_first
                        .get(..usize::try_from(page_size).unwrap_or_default().min(after_first.len()))
                        .unwrap_or_default()
                        .to_vec(),
                }
            } else {
                baseline_alarms.clone()
            };
            assert_eq!(
                listed_alarms(&listed),
                expected,
                "case {case:?}: expected the page at that position {expected:?}, got {listed}"
            );
            if token_case != "the cursor the listing minted" {
                assert_eq!(
                    next_token(&listed),
                    baseline_token,
                    "case {case:?}: expected the first page's own token {baseline_token:?}, got \
                     {listed}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_token_the_mute_rule_listing_cannot_read_is_ignored_and_the_first_page_served() {
    // The same rule through the other listing of this service that serves such a token.
    for (case, total) in [("one rule", 1_usize), ("three rules", 3)] {
        let router = build_router();
        given_rules(&router, total).await;

        let minted = ok(&router, "ListAlarmMuteRules", &json!({"MaxRecords": 1})).await;
        let cursor = next_token(&minted).unwrap_or_default();
        let whole = listed_arns(&ok(&router, "ListAlarmMuteRules", &json!({})).await);
        assert_eq!(
            whole.len(),
            total,
            "case {case:?}: the rules must be stored before they are paged, got {whole:?}"
        );

        for (token_case, token) in unreadable_tokens(&cursor) {
            let request = json!({"NextToken": token});
            let listed = ok(&router, "ListAlarmMuteRules", &request).await;
            let case = format!("{case} with {token_case} ({token:?})");
            assert_eq!(
                listed_arns(&listed),
                whole,
                "case {case:?}: expected the first page {whole:?}, got {listed}"
            );
            assert_eq!(
                next_token(&listed),
                None,
                "case {case:?}: a page holding every rule owes no token, got {listed}"
            );
            let again = ok(&router, "ListAlarmMuteRules", &request).await;
            assert_eq!(
                listed_arns(&again),
                whole,
                "case {case:?}: expected the same first page twice, got {again}"
            );
        }

        // A readable cursor past every rule is the end of this enumeration too.
        let exhausted = ok(
            &router,
            "ListAlarmMuteRules",
            &json!({"NextToken": past_every_item(RULE_PREFIX)}),
        )
        .await;
        assert_eq!(
            exhausted
                .get("AlarmMuteRuleSummaries")
                .and_then(Value::as_array),
            Some(&Vec::new()),
            "case {case:?}: expected AlarmMuteRuleSummaries present and empty past the last \
             rule, got {exhausted}"
        );
        assert_eq!(
            next_token(&exhausted),
            None,
            "case {case:?}: an exhausted page owes no token, got {exhausted}"
        );

        // The positions this enumeration holds: the beginning, asked for by no token, by the
        // empty string and by a cursor sorting before every rule, and the rule a minted cursor
        // names.
        for (token_case, request) in [
            ("no token at all", json!({})),
            ("the empty string", json!({"NextToken": ""})),
            (
                "a cursor sorting before every rule",
                json!({"NextToken": before_every_item(RULE_PREFIX)}),
            ),
        ] {
            let listed = ok(&router, "ListAlarmMuteRules", &request).await;
            assert_eq!(
                listed_arns(&listed),
                whole,
                "case {case:?} with {token_case}: expected the first page {whole:?}, got {listed}"
            );
        }
        if !cursor.is_empty() {
            let resumed = ok(
                &router,
                "ListAlarmMuteRules",
                &json!({"NextToken": cursor}),
            )
            .await;
            assert_eq!(
                listed_arns(&resumed),
                whole.get(1..).unwrap_or_default(),
                "case {case:?}: expected a minted cursor to resume at the rule after the first, \
                 got {resumed}"
            );
        }

        // None of those readings removed a rule.
        let after = listed_arns(&ok(&router, "ListAlarmMuteRules", &json!({})).await);
        assert_eq!(
            after, whole,
            "case {case:?}: expected every stored rule to survive the readings, got {after:?}"
        );
    }
}
