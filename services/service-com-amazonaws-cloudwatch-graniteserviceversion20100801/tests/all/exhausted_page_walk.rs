// SPDX-License-Identifier: CC0-1.0

//! What the END of a paginated walk of this service looks like: which page owes a token, what
//! the page past the last item holds, and what a token the listing cannot place is answered.
//!
//! The rule under test: a `NextToken` is a POSITION a walk reached. A page owes one only while
//! an item is left over beyond it ("The token returned by a previous call to indicate that
//! there is more data available.", `DescribeAlarmsInput$NextToken`; "If this field is absent,
//! there are no more results to retrieve.", `ListAlarmMuteRulesOutput$NextToken`), so the last
//! page carrying items ends the walk — even one the listing filled exactly. A cursor naming a
//! position past every item is the END of a walk: the page served holds no item and owes no
//! token. A value this listing can read no cursor out of names no position at all, so it is
//! ignored and the page served is the first. So no page ever delivers an item the walk already
//! delivered, and every walk terminates.
//!
//! The rows differ along what the rule generalises over: how many items the listing holds (none
//! through more than one page's worth), the page size the request asks for (each published bound
//! of `MaxRecords` and sizes between them), whether the item count divides by that size — which
//! is what decides whether the last delivered page was exactly full — and WHICH listing is
//! walked: the alarm listing narrowed by a name prefix and the alarm mute rule listing, the two
//! reads of this service that share one page-cutting rule.
//!
//! Every call goes through the shared wire harness, the same front door a client uses.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The mute-rule request builders and readers already exist beside this suite, for the same
// resource: it writes and reads a rule through those rather than a second copy.
use crate::alarm_mute_rules::{listed_arns, put_request, rule_arn, with_members};
use crate::harness::{build_router, metric_alarm_request, ok};

/// The Region the harness signs its requests for, and so the one a rule it writes lives in.
const REGION: &str = "us-east-1";

/// The prefix every alarm this suite writes shares, so a listing narrowed by it holds exactly
/// the alarms a row created.
const PREFIX: &str = "exhausted-alarm-";

/// The page sizes the rows walk with: the modelled minimum of `MaxRecords`
/// (`smithy_query(shape="MaxRecords")`: range 1..100), two sizes between the bounds, and the
/// modelled maximum.
const PAGE_SIZES: [i64; 4] = [1, 2, 3, 100];

/// How many items a row's listing holds: none at all, fewer than the smallest page, and counts
/// that do and do not divide by the page sizes above.
const ITEM_COUNTS: [usize; 5] = [0, 1, 2, 3, 4];

/// One page of a walk: the items it delivered and the token it owes.
struct Page {
    items: Vec<String>,
    token: Option<String>,
}

/// The token an answer owes the next page, if any.
fn token_of(listed: &Value) -> Option<String> {
    listed
        .get("NextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

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

/// Write `count` mute rules whose names sort in the order they were created, answering the ARNs
/// a listing identifies them by.
async fn given_rules(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let mut arns: Vec<String> = vec![];
    for index in 0..count {
        let name = format!("exhausted-rule-{index:03}");
        // A window that opens every minute and stays open an hour, so the status the clock
        // makes of the rule cannot change while the suite runs.
        ok(
            router,
            "PutAlarmMuteRule",
            &put_request(&name, "cron(* * * * *)", "PT1H"),
        )
        .await;
        arns.push(rule_arn(REGION, &name));
    }
    arns
}

/// One page of the alarm listing, narrowed by the prefix.
async fn alarm_page(router: &MultiServiceRouter, page_size: i64, token: Option<&str>) -> Page {
    let mut request = json!({"AlarmNamePrefix": PREFIX, "MaxRecords": page_size});
    if let Some(token) = token {
        request = with_members(request, json!({"NextToken": token}));
    }
    let listed = ok(router, "DescribeAlarms", &request).await;
    Page {
        items: listed_alarms(&listed),
        token: token_of(&listed),
    }
}

/// One page of the mute rule listing.
async fn rule_page(router: &MultiServiceRouter, page_size: i64, token: Option<&str>) -> Page {
    let mut request = json!({"MaxRecords": page_size});
    if let Some(token) = token {
        request = with_members(request, json!({"NextToken": token}));
    }
    let listed = ok(router, "ListAlarmMuteRules", &request).await;
    Page {
        items: listed_arns(&listed),
        token: token_of(&listed),
    }
}

/// Walk one listing to its end, asserting the rule on every page it is served, and answer the
/// token the last page carried (which must be none) after checking what it delivered.
///
/// `read` serves one page at a time, which is the only thing the two listings differ in.
async fn assert_walk<Read, Reading>(case: &str, page_size: i64, expected: &[String], read: Read)
where
    Read: Fn(i64, Option<String>) -> Reading,
    Reading: std::future::Future<Output = Page>,
{
    let size = usize::try_from(page_size).unwrap_or_default();
    let mut delivered: Vec<String> = vec![];
    let mut token: Option<String> = None;
    // Every walk ends within one page per item plus one; the loop is generous rather than
    // tight, and the panic below is what reports a walk that does not terminate.
    let budget = expected.len() + 2;
    for index in 0..budget {
        let page = read(page_size, token.clone()).await;
        assert!(
            page.items.len() <= size,
            "case {case:?}: page {index} may hold at most {size} items, got {:?}",
            page.items
        );
        // The rule, on every page: a token is owed only while an item is left over beyond it,
        // which is exactly when the page was full AND items remain undelivered.
        let left_over = expected.len() > delivered.len() + page.items.len();
        assert_eq!(
            page.token.is_some(),
            left_over,
            "case {case:?}: page {index} delivered {} items with {} left over, so a token is \
             owed only if some are, got {:?}",
            page.items.len(),
            expected.len().saturating_sub(delivered.len() + page.items.len()),
            page.token
        );
        delivered.extend(page.items.clone());
        match page.token {
            None => {
                assert_eq!(
                    delivered, expected,
                    "case {case:?}: expected every item exactly once in order, got \
                     {delivered:?}"
                );
                return;
            }
            Some(next) => token = Some(next),
        }
    }
    panic!("case {case:?}: the walk did not terminate within {budget} pages, saw {delivered:?}");
}

#[tokio::test]
async fn an_alarm_walk_ends_on_the_last_page_carrying_items() {
    for count in ITEM_COUNTS {
        let router = build_router();
        let names = given_alarms(&router, count).await;
        for page_size in PAGE_SIZES {
            let case = format!("{count} alarms in pages of {page_size}");
            let router = &router;
            assert_walk(&case, page_size, &names, move |size, token| async move {
                alarm_page(router, size, token.as_deref()).await
            })
            .await;
        }
    }
}

#[tokio::test]
async fn a_mute_rule_walk_ends_on_the_last_page_carrying_items() {
    for count in ITEM_COUNTS {
        let router = build_router();
        let arns = given_rules(&router, count).await;
        for page_size in PAGE_SIZES {
            let case = format!("{count} mute rules in pages of {page_size}");
            let router = &router;
            assert_walk(&case, page_size, &arns, move |size, token| async move {
                rule_page(router, size, token.as_deref()).await
            })
            .await;
        }
    }
}

/// A cursor of this service's own naming a position past every item of the selection is the END
/// of that walk on both listings: the page delivers nothing and owes no token, so a client that
/// walks while a token is present stops there, having seen each item once. A value NO cursor can
/// be read out of names no position at all and is ignored instead, so the page it is served is
/// the listing's first.
#[tokio::test]
async fn a_token_past_the_end_of_a_walk_is_answered_the_empty_page() {
    let router = build_router();
    let names = given_alarms(&router, 2).await;
    let arns = given_rules(&router, 2).await;

    // (case, the token the continuation carries, the items the page it is served delivers) — a
    // token this service never minted, in the two forms one can take, which names no position and
    // is ignored, and one of its OWN tokens whose cursor is past every item, which names the end.
    let past_every_alarm = ::hex::encode("exhausted-alarm-zzz");
    let past_every_rule = ::hex::encode("exhausted-rule-zzz");
    let alarm_tokens: [(&str, &str, bool); 3] = [
        ("a value that is no cursor at all", "not-a-cursor", true),
        (
            "an opaque blob of the form AWS mints a token in",
            "eyJhIjoxfQ==",
            true,
        ),
        (
            "a cursor of its own, past every alarm",
            past_every_alarm.as_str(),
            false,
        ),
    ];
    let rule_tokens: [(&str, &str, bool); 3] = [
        ("a value that is no cursor at all", "not-a-cursor", true),
        (
            "an opaque blob of the form AWS mints a token in",
            "eyJhIjoxfQ==",
            true,
        ),
        (
            "a cursor of its own, past every rule",
            past_every_rule.as_str(),
            false,
        ),
    ];

    // The listing really does hold the items, so an empty page below is the walk's end rather
    // than an empty store.
    let first = alarm_page(&router, 2, None).await;
    assert_eq!(
        first.items, names,
        "the first page must deliver both alarms, got {:?}",
        first.items
    );
    assert_eq!(
        first.token, None,
        "both alarms fit in the page asked for, so no token is owed, got {:?}",
        first.token
    );

    for (case, token, ignored) in alarm_tokens {
        let page = alarm_page(&router, 2, Some(token)).await;
        let expected: Vec<String> = if ignored { names.clone() } else { vec![] };
        assert!(
            page.items == expected && page.token.is_none(),
            "case {case:?}: expected {expected:?} with no token, got {:?} with {:?}",
            page.items,
            page.token
        );
    }

    let first = rule_page(&router, 2, None).await;
    assert_eq!(
        first.items, arns,
        "the first page must deliver both mute rules, got {:?}",
        first.items
    );
    assert_eq!(
        first.token, None,
        "both rules fit in the page asked for, so no token is owed, got {:?}",
        first.token
    );

    for (case, token, ignored) in rule_tokens {
        let page = rule_page(&router, 2, Some(token)).await;
        let expected: Vec<String> = if ignored { arns.clone() } else { vec![] };
        assert!(
            page.items == expected && page.token.is_none(),
            "case {case:?}: expected {expected:?} with no token, got {:?} with {:?}",
            page.items,
            page.token
        );
    }
}
