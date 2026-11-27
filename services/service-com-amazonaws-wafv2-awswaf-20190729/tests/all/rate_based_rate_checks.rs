// SPDX-License-Identifier: CC0-1.0

//! WHEN a rate-based rule starts acting on an aggregation instance: the rate it acts on
//! is the one WAF's last CHECK of that instance arrived at, and the action applies only
//! once that rate is OVER the rule's `Limit`.
//!
//! Both halves come from the same two members:
//!
//! * `RateBasedStatement$Limit` is "the limit on requests during the specified
//!   evaluation window for a single aggregation instance for the rate-based rule", and
//!   `RateBasedStatement$AggregateKeyType` says the action applies "when the count of
//!   all requests that satisfy the rate-based rule's criteria goes over the limit" — so
//!   an instance AT the limit is not rate-limited, only one over it is
//!   (`smithy_query(query="shape",shape="RateBasedStatement")`).
//! * `RateBasedStatement$EvaluationWindowSec` says the window "doesn't determine how
//!   often WAF checks the rate, but how far back it looks each time it checks. WAF
//!   checks the rate about every 10 seconds"
//!   (`smithy_query(query="doc",shape="RateBasedStatement",member="EvaluationWindowSec")`)
//!   — so the rate is not taken per request: a check establishes it, every request
//!   decided before the next check is due is decided on it, and a burst sent as fast as
//!   a caller can send it therefore crosses no limit the previous check did not already
//!   find crossed.
//!
//! Two tables, measured against different instants, as `crate::rate_based_window_recovery`
//! splits the window's own rows:
//!
//! * [`a_rate_based_rule_acts_on_the_rate_its_last_check_arrived_at`] takes the instant
//!   as a parameter and tables the check interval's edge: how old the last check is, and
//!   whether the rate the rule reads is that check's or a fresh one.
//! * [`a_rate_based_rule_acts_only_once_a_check_finds_the_instance_over_the_limit`]
//!   drives whole requests through the data plane for several `Limit`/`EvaluationWindowSec`
//!   pairs: a burst inside one check interval, then the limit's own edge at a check that
//!   IS due, read both from the edge's answer and from
//!   `GetRateBasedStatementManagedKeys`.

use serde_json::json;
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge::{
    self, InspectedRequest, RateBucket, RateEvaluation, RateInstanceKey,
    RATE_CHECK_INTERVAL_MILLIS,
};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    self, ComAmazonawsWafv2Awswaf20190729State,
};

use crate::web_acl_lifecycle::{visibility, Waf, TEST_REGION};

/// The `Scope` the web ACLs live in: an API Gateway stage is a REGIONAL protected
/// resource.
const SCOPE: &str = "REGIONAL";

/// The rate-based rule's name, which is also its metric dimension.
const RULE: &str = "by-ip";

/// The one caller every request below comes from, so one aggregation instance holds
/// them all.
const CALLER: &str = "198.51.100.23";

/// One request from [`CALLER`], as the edge inspects it.
fn request() -> InspectedRequest {
    InspectedRequest {
        method: "GET".to_string(),
        uri_path: "/prod/flood".to_string(),
        query_string: String::new(),
        headers: vec![("user-agent".to_string(), "rate-checks".to_string())],
        body: Vec::new(),
        client_ip: CALLER.to_string(),
    }
}

/// A fixed instant to check against — epoch milliseconds for 2026-01-01T00:00:00Z — so
/// the rows of the first table are decided by the interval rather than by when the suite
/// ran.
const BASE_MILLIS: i64 = 1_767_225_600_000;

/// The aggregation instance the first table checks the rate of.
fn key() -> RateInstanceKey {
    RateInstanceKey {
        scope: SCOPE.to_string(),
        web_acl_id: "6b9e1a0c-interval".to_string(),
        rule_path: RULE.to_string(),
        aggregate_key: CALLER.to_string(),
    }
}

#[test]
fn a_rate_based_rule_acts_on_the_rate_its_last_check_arrived_at() {
    /// How old the last check is when the rate is read, and what the read answers.
    struct Row {
        case: &'static str,
        /// Milliseconds between the stored check and the instant the rate is read at.
        check_age_millis: i64,
        /// Whether the next check is due, so the rate is counted again.
        rechecked: bool,
    }

    /// The requests the instance's counts actually hold at the instant of the read.
    const COUNTED: u64 = 41;

    /// The rate the stored check arrived at: another figure than [`COUNTED`], so a row
    /// cannot pass by reading the wrong one.
    const CHECKED: u64 = 7;

    /// Every window a rate-based rule can look back over: "Valid settings are 60, 120,
    /// 300, and 600" (`RateBasedStatement$EvaluationWindowSec`). The counts below all
    /// lie in the second of the read, so every window holds them and the rows are about
    /// the check interval alone.
    const EVALUATION_WINDOWS_SEC: [i64; 4] = [60, 120, 300, 600];

    let rows = [
        Row {
            case: "the check just ran",
            check_age_millis: 0,
            rechecked: false,
        },
        Row {
            case: "a millisecond into the interval",
            check_age_millis: 1,
            rechecked: false,
        },
        Row {
            case: "the last millisecond of the interval",
            check_age_millis: RATE_CHECK_INTERVAL_MILLIS - 1,
            rechecked: false,
        },
        Row {
            case: "the interval has passed, so the next check is due",
            check_age_millis: RATE_CHECK_INTERVAL_MILLIS,
            rechecked: true,
        },
        Row {
            case: "a millisecond past the interval",
            check_age_millis: RATE_CHECK_INTERVAL_MILLIS + 1,
            rechecked: true,
        },
        Row {
            case: "many intervals past it",
            check_age_millis: 11 * RATE_CHECK_INTERVAL_MILLIS,
            rechecked: true,
        },
    ];

    for window_sec in EVALUATION_WINDOWS_SEC {
        for row in &rows {
            let mut store = ComAmazonawsWafv2Awswaf20190729State::default();
            store.rate_based_counts.insert(
                key(),
                vec![RateBucket {
                    second: BASE_MILLIS.div_euclid(1_000),
                    requests: COUNTED,
                }],
            );
            store.rate_based_evaluations.insert(
                key(),
                RateEvaluation {
                    checked_at: BASE_MILLIS - row.check_age_millis,
                    counted: CHECKED,
                },
            );
            let expected = if row.rechecked { COUNTED } else { CHECKED };
            let read = edge::checked_rate(
                &mut store,
                &key(),
                edge::evaluation_window_millis(window_sec),
                BASE_MILLIS,
            );
            assert_eq!(
                read, expected,
                "case {:?} at EvaluationWindowSec {window_sec}: expected the rule to \
                 read {expected}, got {read}",
                row.case
            );
            // A check that ran replaces the stored one, so the interval starts again
            // from it; one that was not due leaves it exactly where it was.
            let stored = store
                .rate_based_evaluations
                .get(&key())
                .expect("the instance's check is stored")
                .clone();
            let expected_at = if row.rechecked {
                BASE_MILLIS
            } else {
                BASE_MILLIS - row.check_age_millis
            };
            assert_eq!(
                stored,
                RateEvaluation {
                    checked_at: expected_at,
                    counted: expected,
                },
                "case {:?} at EvaluationWindowSec {window_sec}: expected the check \
                 stored for the instance to be {expected} at {expected_at}, got \
                 {stored:?}",
                row.case
            );
        }
    }
}

/// One `Limit`/`EvaluationWindowSec` pair a rate-based rule is driven at.
///
/// `RateBasedStatement$Limit` is `@range(min 10, max 2000000000)` and the windows are the
/// four the member names, so the rows span the smallest limit the model allows and larger
/// ones, each against a different window.
struct Rule {
    /// The rule's `Limit`.
    limit: i64,
    /// Its `EvaluationWindowSec`.
    window_sec: i64,
}

/// The rules driven end to end: the smallest limit the model allows at the shortest and
/// the longest window, and two larger limits in between.
const RULES: [Rule; 4] = [
    Rule {
        limit: 10,
        window_sec: 60,
    },
    Rule {
        limit: 10,
        window_sec: 600,
    },
    Rule {
        limit: 13,
        window_sec: 120,
    },
    Rule {
        limit: 24,
        window_sec: 300,
    },
];

/// The web ACL carrying the rule of `rule`, which is also the `WebACLName` the
/// managed-keys report takes.
fn web_acl_name(rule: &Rule) -> String {
    format!("rate-checks-{}-{}", rule.limit, rule.window_sec)
}

/// The protected resource that web ACL is put in front of: an API Gateway REST API
/// stage, one of the ARN forms `AssociateWebACLRequest$ResourceArn` documents. A
/// resource carries ONE web ACL, so each rule fronts its own stage.
fn stage_arn(rule: &Rule) -> String {
    format!(
        "arn:aws:apigateway:{TEST_REGION}::/restapis/b2c3d4e5f6/stages/l{}w{}",
        rule.limit, rule.window_sec
    )
}

/// Place `requests` requests for every aggregation instance of `web_acl_id` in the
/// second the clock is in, so the instance's windowed count IS that figure.
///
/// Against the clock rather than relative to what is already recorded: the count is read
/// through a window measured from the instant of the check, so the rows have to say where
/// the counts sit rather than how many were sent.
fn place_count(
    state: &::engine::ServiceState<ComAmazonawsWafv2Awswaf20190729State>,
    web_acl_id: &str,
    requests: u64,
) {
    let second = state::now_millis().div_euclid(1_000);
    let mut store = state.lock();
    for (key, buckets) in &mut store.rate_based_counts {
        if key.web_acl_id != web_acl_id {
            continue;
        }
        *buckets = vec![RateBucket { second, requests }];
    }
}

/// Put every rate check of `web_acl_id` a whole interval behind the clock, so the next
/// request to reach a rate-based rule of that web ACL re-checks the rate instead of
/// acting on the rate already checked.
///
/// A suite can no more wait ten seconds for the next check than it can wait for a window
/// to pass, so the stored check is AGED, which is what the passage of that much time does
/// to when the next one falls due.
///
/// Shared with `crate::rate_based_window_recovery`, whose rows are about the WINDOW and
/// so need the check they are read through to be due.
pub(crate) fn due_rate_check(
    state: &::engine::ServiceState<ComAmazonawsWafv2Awswaf20190729State>,
    web_acl_id: &str,
) {
    let mut store = state.lock();
    for (key, evaluation) in &mut store.rate_based_evaluations {
        if key.web_acl_id != web_acl_id {
            continue;
        }
        evaluation.checked_at -= RATE_CHECK_INTERVAL_MILLIS;
    }
}

/// Drive one request at `resource_arn` and answer whether the web ACL in front of it let
/// the request through.
fn allowed(engine: &::engine::InMemoryEngine, resource_arn: &str, case: &str) -> bool {
    edge::decide(engine, resource_arn, &request())
        .unwrap_or_else(|| panic!("case {case:?}: the associated web acl must decide"))
        .action
        .is_allow()
}

#[tokio::test]
async fn a_rate_based_rule_acts_only_once_a_check_finds_the_instance_over_the_limit() {
    let (waf, engine) = Waf::sharing_engine();
    let state = state::service_state(&*engine)
        .expect("the wafv2 store is reachable from the shared engine");

    for rule in &RULES {
        let web_acl = web_acl_name(rule);
        let resource_arn = stage_arn(rule);
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
                                "Limit": rule.limit,
                                "AggregateKeyType": "IP",
                                "EvaluationWindowSec": rule.window_sec,
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

        // A burst sent inside one check interval: the rate in force is the one the check
        // at its first request arrived at, and no later check has run, so every request
        // of the burst is served — including the ones past the limit, which the next
        // check is what finds.
        let burst = rule.limit + 2;
        for sent in 1..=burst {
            assert!(
                allowed(&engine, &resource_arn, "a burst inside one check interval"),
                "case a burst inside one check interval at Limit {} and \
                 EvaluationWindowSec {}: request {sent} of {burst} must be served, no \
                 check has found the caller over the limit",
                rule.limit,
                rule.window_sec
            );
        }

        // The limit's own edge, read at a check that IS due: at the limit the caller is
        // served, over it the rule acts. The figures are the limit's neighbours and a
        // rate well past it.
        for counted in [rule.limit - 1, rule.limit, rule.limit + 1, 3 * rule.limit] {
            let over = counted > rule.limit;
            place_count(&state, &web_acl_id, counted as u64);
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
                reports_caller, over,
                "case a rate of {counted} at Limit {} and EvaluationWindowSec {}: \
                 expected the caller among the keys the rule is limiting = {over}, got \
                 {listed:?}",
                rule.limit, rule.window_sec
            );

            let served = allowed(&engine, &resource_arn, "the limit's own edge");
            assert_eq!(
                served, !over,
                "case a rate of {counted} at Limit {} and EvaluationWindowSec {}: \
                 expected the request to be served = {}, got served = {served}",
                rule.limit,
                rule.window_sec,
                !over
            );
        }
    }
}
