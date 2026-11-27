// SPDX-License-Identifier: CC0-1.0

//! A rate-based rule RECOVERS: what the edge decides once a caller's requests have
//! fallen out of the rule's evaluation window.
//!
//! The rule this suite pins: a rate-based rule counts a caller's requests per
//! aggregation instance and applies its action while that instance's count over the
//! `EvaluationWindowSec` immediately preceding THE INSTANT OF THE CHECK is over the
//! `Limit` — "the amount of time, in seconds, that WAF should include in its request
//! counts, looking back from the current time"
//! (`smithy_query(query="doc",shape="RateBasedStatement",member="EvaluationWindowSec")`).
//! So the window is a SLIDING one, measured from the call that reads it, and a caller
//! whose counted requests all lie further back than the window is under the limit again
//! and is served. The same arithmetic decides what `GetRateBasedStatementManagedKeys`
//! lists, so a key stops being reported at the instant the rule stops acting on it.
//!
//! WHEN that count is taken is the other half of the same member — "this setting doesn't
//! determine how often WAF checks the rate, but how far back it looks each time it
//! checks. WAF checks the rate about every 10 seconds" — so the edge acts on the rate
//! the last CHECK arrived at, and every request inside one interval is decided on it.
//! The rows below are about the WINDOW, so each one has the next check fall due before
//! the request it asserts ([`due_rate_check`]); the interval itself is what
//! `crate::rate_based_rate_checks` pins.
//!
//! TWO tables, because the window has two things to pin and they are measured against
//! different instants:
//!
//! * [`a_rate_based_rule_serves_the_caller_again_once_its_window_has_passed`] drives the
//!   whole path — the data plane's own entry point
//!   ([`service_com_amazonaws_wafv2_awswaf_20190729::service::edge::decide`]) on the
//!   SAME shared engine the service is mounted on, and the managed keys read over the
//!   wire — for every window a rate-based statement can look back over ("Valid settings
//!   are 60, 120, 300, and 600", the same member), against how far back the caller's
//!   counted requests lie: inside the window, far into it, and past it. Time is not
//!   waited for; the recorded per-second counts are PLACED at a chosen second before the
//!   current one, which is what the passage of that much time does to the window they
//!   are read through.
//! * [`the_evaluation_window_counts_the_seconds_it_names`] pins the window's EDGE, where
//!   the instant has to be exact: the counting both readers above share takes that
//!   instant as a parameter, so the table names it rather than reading the clock.
//!
//! The split is itself the rule at work. The window is measured from the instant of each
//! CHECK, which for a call is necessarily later than any state the suite placed for it,
//! so a row whose expected answer changes within a second of the placement cannot be
//! asserted across a call at all: it would hold only while the suite stayed inside the
//! wall-clock second it placed the counts in. The rows driven through the service
//! therefore sit well inside the window or past it, and the edge is pinned where the
//! instant is a parameter.

use serde_json::json;
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge::{
    self, InspectedRequest, RateBucket,
};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    self, ComAmazonawsWafv2Awswaf20190729State,
};

use crate::rate_based_rate_checks::due_rate_check;
use crate::web_acl_lifecycle::{visibility, Waf, TEST_REGION};

/// The `Scope` the web ACLs live in: an API Gateway stage is a REGIONAL protected
/// resource.
const SCOPE: &str = "REGIONAL";

/// `RateBasedStatement$Limit` is modelled `range(min 10)`, so this is the smallest
/// limit a rule can carry.
const LIMIT: i64 = 10;

/// Every window a rate-based rule can look back over: "Valid settings are 60, 120, 300,
/// and 600" (`RateBasedStatement$EvaluationWindowSec`), the shortest and the longest of
/// them included.
const EVALUATION_WINDOWS_SEC: [i64; 4] = [60, 120, 300, 600];

/// The rate-based rule's name, which is also its metric dimension.
const RULE: &str = "by-ip";

/// The one caller every request below comes from, so one aggregation instance holds
/// them all.
const CALLER: &str = "203.0.113.7";

/// The web ACL carrying the rule that looks back over `window_sec`, which is also the
/// `WebACLName` the managed-keys report takes. One web ACL per window, so each row's
/// counts are read through the window its own rule names.
fn web_acl_name(window_sec: i64) -> String {
    format!("rate-recovery-{window_sec}")
}

/// The protected resource the web ACL of `window_sec` is put in front of: an API
/// Gateway REST API stage, the one ARN form `AssociateWebACLRequest$ResourceArn`
/// documents for one. A resource carries ONE web ACL, so each window fronts its own
/// stage.
fn stage_arn(window_sec: i64) -> String {
    format!("arn:aws:apigateway:{TEST_REGION}::/restapis/a1b2c3d4e5/stages/w{window_sec}")
}

/// One request from [`CALLER`], as the edge inspects it.
fn request() -> InspectedRequest {
    InspectedRequest {
        method: "GET".to_string(),
        uri_path: "/prod/orders".to_string(),
        query_string: String::new(),
        headers: vec![("user-agent".to_string(), "rate-recovery".to_string())],
        body: Vec::new(),
        client_ip: CALLER.to_string(),
    }
}

/// Place the requests counted for every aggregation instance of `web_acl_id` at the
/// second `seconds_ago` before the current one: one per-second count holding the
/// instance's whole total, which is what the window reads when all of that caller's
/// requests lie exactly that far back.
///
/// Against the clock rather than relative to where the counts already sit — see the
/// module docs for why that is the difference between a row that holds and a row that
/// holds only until the wall clock ticks over.
fn place_counts(
    state: &::engine::ServiceState<ComAmazonawsWafv2Awswaf20190729State>,
    web_acl_id: &str,
    seconds_ago: i64,
) {
    let second = state::now_millis().div_euclid(1_000) - seconds_ago;
    let mut store = state.lock();
    for (key, buckets) in &mut store.rate_based_counts {
        if key.web_acl_id != web_acl_id {
            continue;
        }
        let requests = buckets
            .iter()
            .fold(0_u64, |total, bucket| total.saturating_add(bucket.requests));
        *buckets = vec![RateBucket { second, requests }];
    }
}

/// Drive one request at `resource_arn` and answer whether the web ACL in front of it
/// let the request through.
///
/// Given the shared ENGINE, which is what the data plane's entry point takes: what WAF
/// reports about one request spans its own store and the CloudWatch one.
fn allowed(engine: &::engine::InMemoryEngine, resource_arn: &str, case: &str) -> bool {
    edge::decide(engine, resource_arn, &request())
        .unwrap_or_else(|| panic!("case {case:?}: the associated web acl must decide"))
        .action
        .is_allow()
}

/// How much of the window a row asserted ACROSS a call leaves between the counts it
/// places and the window's edge.
///
/// Not an AWS figure: a call's own instant is later than the placement by however long
/// the dispatch takes, and the window is measured from that instant, so a row of this
/// kind is only well defined while that difference stays under its slack. Ten seconds
/// against a dispatch measured in milliseconds, and the shortest window the member
/// allows is sixty, so every such row stays well inside the window it names. The edge
/// itself is [`the_evaluation_window_counts_the_seconds_it_names`].
const WIRE_ROW_SLACK_SEC: i64 = 10;

/// How far back the caller's counted requests are placed, and what the rule then makes
/// of one more request from the same caller.
struct Case {
    case: &'static str,
    /// Seconds before the current one the recorded counts are placed at.
    seconds_ago: i64,
    /// Whether the next request is served.
    served: bool,
    /// Whether the caller's key is still among the ones over the limit.
    listed: bool,
}

/// The rows for a rule looking back over `window_sec`: inside the window at the instant
/// the counts are placed, far into it, one second past it, and a whole window past it.
///
/// The rows INSIDE the window keep [`WIRE_ROW_SLACK_SEC`] between the counts they place
/// and the window's edge. The rows PAST it need no slack: the window only ever moves
/// forward, so counts outside it at the placement are outside it at every later instant.
fn cases(window_sec: i64) -> [Case; 4] {
    [
        Case {
            case: "the counted requests are still where they were: inside the window",
            seconds_ago: 0,
            served: false,
            listed: true,
        },
        Case {
            case: "the counted requests lie far into the window, short of its edge",
            seconds_ago: window_sec - WIRE_ROW_SLACK_SEC,
            served: false,
            listed: true,
        },
        Case {
            case: "the counted requests lie one second past the window",
            seconds_ago: window_sec + 1,
            served: true,
            listed: false,
        },
        Case {
            case: "the counted requests lie a whole window further back still, so a \
                   recovered caller stays recovered",
            seconds_ago: 2 * window_sec,
            served: true,
            listed: false,
        },
    ]
}

#[tokio::test]
async fn a_rate_based_rule_serves_the_caller_again_once_its_window_has_passed() {
    let (waf, engine) = Waf::sharing_engine();
    let state = state::service_state(&*engine)
        .expect("the wafv2 store is reachable from the shared engine");

    for window_sec in EVALUATION_WINDOWS_SEC {
        let web_acl = web_acl_name(window_sec);
        let resource_arn = stage_arn(window_sec);
        let created = waf
            .ok(
                "CreateWebACL",
                json!({
                    "Name": web_acl,
                    "Scope": SCOPE,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&web_acl),
                    "Rules": [{
                        "Name": RULE,
                        "Priority": 0,
                        "Statement": {
                            "RateBasedStatement": {
                                "Limit": LIMIT,
                                "AggregateKeyType": "IP",
                                "EvaluationWindowSec": window_sec,
                            }
                        },
                        "Action": { "Block": {} },
                        "VisibilityConfig": visibility(RULE),
                    }],
                }),
            )
            .await;
        let web_acl_id = created["Summary"]["Id"]
            .as_str()
            .expect("Summary.Id")
            .to_string();
        let web_acl_arn = created["Summary"]["ARN"]
            .as_str()
            .expect("Summary.ARN")
            .to_string();
        waf.ok(
            "AssociateWebACL",
            json!({ "WebACLArn": web_acl_arn, "ResourceArn": resource_arn }),
        )
        .await;

        // Take the caller over the limit: every request of the burst is served, because
        // the rate in force is the one the check at its first request arrived at, and
        // the rule acts on the caller at the first check past the limit.
        for sent in 1..=LIMIT + 1 {
            assert!(
                allowed(&engine, &resource_arn, "a burst inside one check interval"),
                "case a burst inside one check interval at EvaluationWindowSec \
                 {window_sec}: request {sent} of {} must be served, the rate checked \
                 for the caller is not over the limit yet",
                LIMIT + 1
            );
        }
        due_rate_check(&state, &web_acl_id);
        assert!(
            !allowed(
                &engine,
                &resource_arn,
                "the first request past the check that found the caller over the limit"
            ),
            "case the first request past the check that found the caller over the limit \
             at EvaluationWindowSec {window_sec}: expected the rule to act on it"
        );

        for case in &cases(window_sec) {
            place_counts(&state, &web_acl_id, case.seconds_ago);
            due_rate_check(&state, &web_acl_id);

            let keys = waf
                .ok(
                    "GetRateBasedStatementManagedKeys",
                    json!({
                        "Scope": SCOPE,
                        "WebACLName": web_acl,
                        "WebACLId": web_acl_id,
                        "RuleName": RULE,
                    }),
                )
                .await;
            let listed: Vec<String> = keys["ManagedKeysIPV4"]["Addresses"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|address| address.as_str().map(str::to_string))
                .collect();
            let reports_caller = listed.iter().any(|address| address.starts_with(CALLER));
            assert_eq!(
                reports_caller, case.listed,
                "case {:?} at EvaluationWindowSec {window_sec}: expected the caller \
                 among the keys over the limit = {}, got {listed:?}",
                case.case, case.listed
            );

            let served = allowed(&engine, &resource_arn, case.case);
            assert_eq!(
                served, case.served,
                "case {:?} at EvaluationWindowSec {window_sec}: expected the request to \
                 be served = {}, got served = {served}",
                case.case, case.served
            );
        }
    }
}

/// A fixed instant to count against — epoch seconds for 2026-01-01T00:00:00Z — so the
/// boundary below is decided by the window rather than by when the suite ran.
const BASE_SECS: i64 = 1_767_225_600;

/// Where inside its own second the check falls, in milliseconds: the first instant of
/// it, the next one, the middle and the last one. A count's age is whole seconds and the
/// instant it is read at is not, so these rows are what says the window's edge does not
/// move with the fraction.
const SUBSECOND_MILLIS: [i64; 4] = [0, 1, 500, 999];

#[test]
fn the_evaluation_window_counts_the_seconds_it_names() {
    /// How far back one recorded count lies, and whether the window still holds it.
    struct Row {
        case: &'static str,
        /// Seconds between the counted second and the second the check falls in; a
        /// negative one is a count the clock has not reached yet.
        seconds_ago: i64,
    }

    /// The requests the one placed count holds, so a window that holds it answers this
    /// and one that does not answers zero.
    const REQUESTS: u64 = 7;

    for window_sec in EVALUATION_WINDOWS_SEC {
        let rows = [
            Row {
                case: "a count the clock has not reached",
                seconds_ago: -1,
            },
            Row {
                case: "the second the check falls in",
                seconds_ago: 0,
            },
            Row {
                case: "one second back",
                seconds_ago: 1,
            },
            Row {
                case: "the last second the window reaches",
                seconds_ago: window_sec - 1,
            },
            Row {
                case: "the first second past the window",
                seconds_ago: window_sec,
            },
            Row {
                case: "one second further past it",
                seconds_ago: window_sec + 1,
            },
            Row {
                case: "a whole window past it",
                seconds_ago: 2 * window_sec,
            },
        ];
        for row in &rows {
            for fraction in SUBSECOND_MILLIS {
                let now = BASE_SECS * 1_000 + fraction;
                let buckets = [RateBucket {
                    second: BASE_SECS - row.seconds_ago,
                    requests: REQUESTS,
                }];
                // "The amount of time, in seconds, that WAF should include in its
                // request counts, looking back from the current time": the counts of
                // the `window_sec` seconds up to and including the one the check falls
                // in, and nothing older and nothing still to come.
                let held = (0..window_sec).contains(&row.seconds_ago);
                let expected = if held { REQUESTS } else { 0 };
                let counted = edge::count_within(
                    &buckets,
                    now,
                    edge::evaluation_window_millis(window_sec),
                );
                assert_eq!(
                    counted, expected,
                    "case {:?} at EvaluationWindowSec {window_sec}, {fraction}ms into \
                     the second: expected {expected} counted, got {counted}",
                    row.case
                );
            }
        }
    }
}
