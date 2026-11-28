// SPDX-License-Identifier: CC0-1.0

//! Invoking an api through the ROUTING RULES of a CUSTOM DOMAIN NAME, over a real connection
//! to the address the domain name publishes.
//!
//! The rule these cases assert, one level above the recorded case (one regional domain name in
//! `ROUTING_RULE_ONLY`, one rule matching one base path, one request served and one refused):
//!
//! > A custom domain name's `RoutingMode` says which of the record's own statements its front
//! > door reads — its `RoutingRule`s, its `ApiMapping`s, or the rules and then the mappings —
//! > and a routing rule selects a request by its CONDITIONS: the base path the request begins
//! > with (`RoutingRuleMatchBasePaths$AnyOf`, "the string of the case sensitive base path to be
//! > matched", so matching on a segment boundary and with no case folding) and the header
//! > values its globs match (`RoutingRuleMatchHeaders$AnyOf`, "matched if any of the header
//! > name and header value globs are matched"), ANDed. The matching rule of the lowest
//! > `Priority` value serves, on the `{ApiId, Stage}` its `invokeApi` action names, with the
//! > matched base path left on the path that api is given unless the action's `StripBasePath`
//! > asks for it to be removed. A request no statement the mode admits answers reaches no api
//! > at all.
//!
//! Nothing here is a value this emulator fills in: every served row is answered by a FUNCTION
//! that ran, which is why AWS Lambda is mounted beside this service, and the handler reports
//! which api, which stage and which path it was given — so a row proves WHICH stage served it,
//! and under WHICH path, rather than merely that something answered.
//!
//! The rows span what the rule generalises over: the base path a rule names (one segment, two
//! segments, the base path itself, a segment that merely starts with it, another case of it, a
//! path no rule names), the `StripBasePath` setting (left out, which is off, and on), a header
//! condition beside a base-path one (a glob that matches, one that does not, a field the
//! request does not carry), the `Priority` order of two matching rules, a query string, each
//! of the three `RoutingMode` values, and a rule that has been deleted.
//!
//! The control-plane surface of a domain name and its rules is not restated here:
//! `domain_name_lifecycle.rs`, `routing_rule_lifecycle.rs` and `routing_rule_read_back.rs` own
//! it, and the api-mapping half of the same front door is `custom_domain_mapping_invoke.rs`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::custom_domain_mapping_invoke::{
    control, given_domain_name_in_mode, given_mapping, request_as, ECHO,
};
use crate::http_api_backend_dispatch::{
    emulator_with_lambda, given_api, given_function, lambda_integration,
};

/// The status a request a rule matched is answered with: the function's own.
const SERVED: u16 = 200;

/// The status and body the domain name's front door turns a request away with when nothing the
/// record states answers it — the refusal API Gateway answers a request that reaches no api of
/// the account with, in the `{"message": …}` envelope every answer of this data plane carries.
const NO_RULE: u16 = 403;
const NO_RULE_MESSAGE: &str = "Forbidden";

/// The three values of `RoutingMode` (`smithy_query(query="shape", shape="RoutingMode")`).
const API_MAPPING_ONLY: &str = "API_MAPPING_ONLY";
const ROUTING_RULE_ONLY: &str = "ROUTING_RULE_ONLY";
const ROUTING_RULE_THEN_API_MAPPING: &str = "ROUTING_RULE_THEN_API_MAPPING";

/// Create the custom domain name in `routing_mode` and answer the address it published,
/// through the one create-and-read-back-the-published-address helper the suites that drive a
/// custom domain name's front door share
/// (`custom_domain_mapping_invoke::given_domain_name_in_mode`).
async fn given_domain_name(
    emulator: &MultiServiceRouter,
    case: &str,
    domain: &str,
    routing_mode: &str,
) -> String {
    given_domain_name_in_mode(emulator, case, domain, Some(routing_mode)).await
}

/// Create one routing rule of `domain` and answer its `RoutingRuleId`.
async fn given_rule(
    emulator: &MultiServiceRouter,
    case: &str,
    domain: &str,
    request: Value,
) -> String {
    let (status, created) = control(
        emulator,
        "POST",
        &format!("/v2/domainnames/{domain}/routingrules"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoutingRule answered {status} {created}"
    );
    created
        .get("routingRuleId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A rule's body: its priority, the conditions it selects on, and the api stage its action
/// invokes — with `stripBasePath` named only when the row asks for it, so a row also covers
/// the setting a request that names none is served under.
fn rule_body(
    priority: i32,
    conditions: Value,
    api_id: &str,
    stage: &str,
    strip_base_path: Option<bool>,
) -> Value {
    let mut invoke_api = json!({ "apiId": api_id, "stage": stage });
    if let Some(strip) = strip_base_path {
        invoke_api["stripBasePath"] = json!(strip);
    }
    json!({
        "priority": priority,
        "conditions": conditions,
        "actions": [{ "invokeApi": invoke_api }]
    })
}

/// A `matchBasePaths` condition over the base paths `any_of`.
fn base_paths(any_of: &[&str]) -> Value {
    json!([{ "matchBasePaths": { "anyOf": any_of } }])
}

/// Which of the two apis a row's answer must come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Api {
    /// The api on a named stage that the base-path rule invokes.
    Alpha,
    /// The api on another named stage, which the header rule and the stripping rule invoke.
    Beta,
}

/// One row: the path a client asks the domain name for, the fields it sends, and either the
/// api, stage and path the answer must report or the refusal it must take.
struct Row {
    case: &'static str,
    path: &'static str,
    headers: &'static [(&'static str, &'static str)],
    /// `Some((api, stage, path))` for a request a rule serves, `None` for one no rule matches.
    served: Option<(Api, &'static str, &'static str)>,
}

/// One row of the routing-mode table: the path a client asks for, and either the api and the
/// path that api is given, or the refusal the request takes.
struct ModeRow<'r> {
    case: &'static str,
    path: &'static str,
    served: Option<(&'r String, &'static str)>,
}

/// The two apis every case here routes to, each on a NAMED stage so the path a stage reports
/// names the stage that served it.
async fn given_two_apis(emulator: &MultiServiceRouter, case: &str, function: &str) -> (String, String) {
    let (alpha, _) = given_api(
        emulator,
        case,
        lambda_integration(function, "2.0"),
        "$default",
        json!({ "stageName": "prod", "autoDeploy": true }),
    )
    .await;
    let (beta, _) = given_api(
        emulator,
        case,
        lambda_integration(function, "2.0"),
        "$default",
        json!({ "stageName": "live", "autoDeploy": true }),
    )
    .await;
    (alpha, beta)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_routing_rules_conditions_and_priority_decide_which_stage_serves_a_request() {
    let case = "three routing rules on one custom domain name";
    let (emulator, engine) = emulator_with_lambda();
    let function = given_function(&emulator, &engine, case, "domain-rule-echo", ECHO).await;
    let (alpha, beta) = given_two_apis(&emulator, case, &function).await;

    let domain = "rules.api.example.com";
    let authority = given_domain_name(&emulator, case, domain, ROUTING_RULE_ONLY).await;
    // The base-path rule, which names no `stripBasePath` — so it is served under the setting a
    // request that names none takes.
    let widgets = given_rule(
        &emulator,
        case,
        domain,
        rule_body(100, base_paths(&["widgets"]), &alpha, "prod", None),
    )
    .await;
    // The stripping rule, over two base paths, so the longest matching one is the one removed.
    given_rule(
        &emulator,
        case,
        domain,
        rule_body(
            200,
            base_paths(&["gadgets", "gadgets/eu"]),
            &beta,
            "live",
            Some(true),
        ),
    )
    .await;
    // A rule of a LOWER priority value over the SAME base path, conditioned on a header too:
    // it takes a request from the rule above only when both of its conditions hold.
    given_rule(
        &emulator,
        case,
        domain,
        rule_body(
            50,
            json!([
                { "matchBasePaths": { "anyOf": ["widgets"] } },
                { "matchHeaders": { "anyOf": [{ "header": "x-tier", "valueGlob": "go*" }] } }
            ]),
            &beta,
            "live",
            Some(false),
        ),
    )
    .await;

    let rows = [
        Row {
            case: "a path under the base path a rule names",
            path: "/widgets/42",
            headers: &[],
            served: Some((Api::Alpha, "prod", "/prod/widgets/42")),
        },
        Row {
            case: "the base path itself",
            path: "/widgets",
            headers: &[],
            served: Some((Api::Alpha, "prod", "/prod/widgets")),
        },
        Row {
            case: "a segment that merely starts with the base path",
            path: "/widgetsmith",
            headers: &[],
            served: None,
        },
        Row {
            case: "another case of the base path, which is matched case sensitively",
            path: "/Widgets",
            headers: &[],
            served: None,
        },
        Row {
            case: "a path no rule of the record names",
            path: "/sprockets/42",
            headers: &[],
            served: None,
        },
        Row {
            case: "the domain root, which no rule's base path covers",
            path: "/",
            headers: &[],
            served: None,
        },
        Row {
            case: "the base path removed when the action asks for it",
            path: "/gadgets/42",
            headers: &[],
            served: Some((Api::Beta, "live", "/live/42")),
        },
        Row {
            case: "the longest of two matching base paths is the one removed",
            path: "/gadgets/eu/42",
            headers: &[],
            served: Some((Api::Beta, "live", "/live/42")),
        },
        Row {
            case: "the stripping rule's base path with nothing under it",
            path: "/gadgets",
            headers: &[],
            served: Some((Api::Beta, "live", "/live")),
        },
        Row {
            case: "a lower priority value whose header glob matches takes the request",
            path: "/widgets/42",
            headers: &[("x-tier", "gold")],
            served: Some((Api::Beta, "live", "/live/widgets/42")),
        },
        Row {
            case: "the same request whose header value the glob does not match",
            path: "/widgets/42",
            headers: &[("x-tier", "silver")],
            served: Some((Api::Alpha, "prod", "/prod/widgets/42")),
        },
        Row {
            case: "a field the header condition does not name",
            path: "/widgets/42",
            headers: &[("x-other", "gold")],
            served: Some((Api::Alpha, "prod", "/prod/widgets/42")),
        },
        Row {
            case: "the header rule's own base-path condition still has to hold",
            path: "/sprockets/42",
            headers: &[("x-tier", "gold")],
            served: None,
        },
        Row {
            // The query string is the client's and is carried on unchanged: the rule chooses
            // the api and the stage, and nothing of the request's own target is rewritten
            // beyond the base path the action removes.
            case: "a query string under a matched base path",
            path: "/widgets/42?colour=red&size=2",
            headers: &[],
            served: Some((Api::Alpha, "prod", "/prod/widgets/42")),
        },
    ];

    for row in rows {
        let answer = crate::published_invoke_socket_front_door::request(
            row.case,
            &authority,
            "GET",
            row.path,
            domain,
            row.headers,
        )
        .await;
        match row.served {
            Some((api, stage, path)) => {
                let query = row
                    .path
                    .split_once('?')
                    .map(|(_path, query)| query)
                    .unwrap_or_default();
                let expected = json!({
                    "api": match api {
                        Api::Alpha => alpha.clone(),
                        Api::Beta => beta.clone(),
                    },
                    "stage": stage,
                    "path": path,
                    "query": query,
                });
                assert_eq!(
                    answer.status, SERVED,
                    "case {:?}: expected {SERVED} from the rule's stage, got {answer:?}",
                    row.case
                );
                assert_eq!(
                    answer.json(),
                    expected,
                    "case {:?}: expected the answer of {expected}, got {answer:?}",
                    row.case
                );
            }
            None => {
                assert_eq!(
                    answer.status, NO_RULE,
                    "case {:?}: expected {NO_RULE} for a request no rule matches, got \
                     {answer:?}",
                    row.case
                );
                assert_eq!(
                    answer.message(),
                    NO_RULE_MESSAGE,
                    "case {:?}: expected the gateway's own sentence for a request that reaches \
                     no api, got {answer:?}",
                    row.case
                );
            }
        }
    }

    // A rule that is DELETED stops routing: the request it served reaches no api of the
    // account, while the rule beside it — the one conditioned on the header — serves the same
    // path as before. The rules are read per request, off the records the control plane wrote.
    let (status, deleted) = control(
        &emulator,
        "DELETE",
        &format!("/v2/domainnames/{domain}/routingrules/{widgets}"),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: DeleteRoutingRule answered {status} {deleted}"
    );
    let answer = request_as(
        "a path whose rule was deleted",
        &authority,
        "/widgets/42",
        domain,
    )
    .await;
    assert_eq!(
        answer.status, NO_RULE,
        "case {case:?}: expected {NO_RULE} once the base-path rule is gone, got {answer:?}"
    );
    let answer = crate::published_invoke_socket_front_door::request(
        "the rule that is still there",
        &authority,
        "GET",
        "/widgets/42",
        domain,
        &[("x-tier", "gold")],
    )
    .await;
    assert_eq!(
        answer.status, SERVED,
        "case {case:?}: expected the remaining rule to serve as before, got {answer:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_routing_mode_decides_whether_the_rules_or_the_mappings_answer() {
    let case = "one rule and one root mapping on a domain name of each routing mode";
    let (emulator, engine) = emulator_with_lambda();
    let function = given_function(&emulator, &engine, case, "domain-mode-echo", ECHO).await;
    let (alpha, beta) = given_two_apis(&emulator, case, &function).await;

    // One domain name per mode, each holding the SAME rule (over the base path `widgets`, onto
    // alpha) and the SAME root api mapping (onto beta) — so only the mode differs.
    for (mode, domain) in [
        (API_MAPPING_ONLY, "mappings.api.example.com"),
        (ROUTING_RULE_ONLY, "rules-only.api.example.com"),
        (ROUTING_RULE_THEN_API_MAPPING, "rules-then.api.example.com"),
    ] {
        let authority = given_domain_name(&emulator, mode, domain, mode).await;
        given_rule(
            &emulator,
            mode,
            domain,
            rule_body(100, base_paths(&["widgets"]), &alpha, "prod", None),
        )
        .await;
        given_mapping(&emulator, mode, domain, &beta, "live", None).await;

        // Two paths per mode: the one the rule matches, and one it does not.
        let rows: [ModeRow; 2] = match mode {
            API_MAPPING_ONLY => [
                ModeRow {
                    case: "the rule's own path",
                    path: "/widgets/42",
                    served: Some((&beta, "/live/widgets/42")),
                },
                ModeRow {
                    case: "a path the rule does not match",
                    path: "/other/42",
                    served: Some((&beta, "/live/other/42")),
                },
            ],
            ROUTING_RULE_ONLY => [
                ModeRow {
                    case: "the rule's own path",
                    path: "/widgets/42",
                    served: Some((&alpha, "/prod/widgets/42")),
                },
                ModeRow {
                    case: "a path the rule does not match",
                    path: "/other/42",
                    served: None,
                },
            ],
            _ => [
                ModeRow {
                    case: "the rule's own path",
                    path: "/widgets/42",
                    served: Some((&alpha, "/prod/widgets/42")),
                },
                ModeRow {
                    case: "a path the rule does not match",
                    path: "/other/42",
                    served: Some((&beta, "/live/other/42")),
                },
            ],
        };
        for row in rows {
            let label = format!("{mode}: {}", row.case);
            let answer = request_as(&label, &authority, row.path, domain).await;
            match row.served {
                Some((api, invoked)) => {
                    assert_eq!(
                        answer.status, SERVED,
                        "case {label:?}: expected {SERVED}, got {answer:?}"
                    );
                    let served = answer.json();
                    assert_eq!(
                        (
                            served.get("api").and_then(Value::as_str).unwrap_or_default(),
                            served.get("path").and_then(Value::as_str).unwrap_or_default()
                        ),
                        (api.as_str(), invoked),
                        "case {label:?}: expected api {api:?} to serve {invoked:?}, got \
                         {answer:?}"
                    );
                }
                None => assert_eq!(
                    answer.status, NO_RULE,
                    "case {label:?}: expected {NO_RULE} where the mode admits only the rules, \
                     got {answer:?}"
                ),
            }
        }
    }
}
