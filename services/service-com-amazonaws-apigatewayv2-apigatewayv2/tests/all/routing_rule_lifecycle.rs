// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `RoutingRule` collection of a custom
//! domain name: `CreateRoutingRule` and `ListRoutingRules`.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly.
//!
//! The rules asserted here, each over a TABLE rather than over one request:
//!
//! * A routing rule is a persisted child of the custom domain name its path names: what
//!   `CreateRoutingRule` reports — the caller's actions, conditions and priority, a
//!   minted `RoutingRuleId` and the rule's ARN — is what a later `ListRoutingRules`
//!   reports for it, and a reader of one domain name reports the rules THAT domain name
//!   owns and no other's.
//! * `ListRoutingRules` serves a page of at most `MaxResults` rules chained through
//!   `NextToken`, and changes nothing a later caller sees.
//! * `Priority` is accepted across its published range (1..1000000) and refused outside
//!   it with the modelled `BadRequestException` (400) — the recorded case that filed the
//!   issue is one row of that table.
//! * A rule names at least one action and at least one condition, every condition names
//!   exactly one of `MatchBasePaths`/`MatchHeaders` with at least one value to match on,
//!   and the list carries at most two `MatchHeaders` and one `MatchBasePaths` condition;
//!   anything else is incomplete-or-incorrect input (400) and persists nothing.
//! * An action's `InvokeApi` is a reference the rule carries: it is stored as the request
//!   wrote it and reported back that way, not resolved against the apis the account holds
//!   when the rule is written.
//! * An operation whose path names a domain name the account does not hold — or whose
//!   `DomainNameId` is not that domain name's own — is refused with the modelled
//!   `NotFoundException` (404).
//! * Caller-driven growth is bounded: a domain name takes no more rules than the store's
//!   bound, and the refusal is the modelled `TooManyRequestsException` (429).
//! * A `NextToken` this reader did not issue for this collection is refused (400).
//!
//! The rows vary what each rule generalises over: the form of the conditions (base paths,
//! one or two header matches, several values in an `AnyOf`), the number of actions, the
//! `StripBasePath` setting, the priority (both published bounds and values between them),
//! the stage invoked, the rule count and the page size.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::paging::MAX_PAGE_SIZE;
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    routing_rule_priority_range_message, EMPTY_ROUTING_RULE_ACTIONS, EMPTY_ROUTING_RULE_CONDITIONS,
    INVALID_DOMAIN_NAME_IDENTIFIER, INVALID_NEXT_TOKEN_MESSAGE, MAX_ROUTING_RULES_PER_DOMAIN,
    ROUTING_RULE_CONDITION_ALTERNATIVES, ROUTING_RULE_CONDITION_LIMITS,
};

use crate::api_lifecycle::{as_str, assert_minted_id, call_in_region, error_code};

/// The region every request in this suite is signed for. Deliberately NOT the emulator's
/// `us-east-1` default, so nothing region-derived (the rule's ARN) can be hardcoded.
const REGION: &str = "ca-central-1";

/// The custom domain name every test seeds unless the row is about an absent one.
const DOMAIN: &str = "ruled.example.com";

/// A second seeded domain name, so a row can prove a reader reports one domain name's
/// rules and not another's.
const NEIGHBOUR_DOMAIN: &str = "neighbour.example.com";

/// The stage every seeded api holds, and a second one so an action's target can move
/// without inventing a stage.
const STAGE: &str = "$default";
const OTHER_STAGE: &str = "prod";

/// The published bounds of `RoutingRulePriority` (`range` 1..1000000), from
/// `smithy_query(query="shape", shape="RoutingRulePriority")`.
const PRIORITY_MIN: i64 = 1;
const PRIORITY_MAX: i64 = 1_000_000;

/// Send one request signed for [`REGION`] and answer `(status, body)`.
async fn call(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_region(emulator, REGION, method, uri, body).await
}

async fn create_routing_rule(
    emulator: &MultiServiceRouter,
    domain: &str,
    request: &Value,
) -> (u16, Value) {
    call(
        emulator,
        "POST",
        &format!("/v2/domainnames/{domain}/routingrules"),
        Some(request.clone()),
    )
    .await
}

async fn list_routing_rules(
    emulator: &MultiServiceRouter,
    domain: &str,
    query: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/domainnames/{domain}/routingrules{query}"),
        None,
    )
    .await
}

/// The rules a listing reports, in the order it reported them.
fn listed(body: &Value) -> Vec<Value> {
    body["routingRules"].as_array().cloned().unwrap_or_default()
}

/// Create one custom domain name, asserting the 201 the model binds to
/// `CreateDomainName`.
async fn seed_domain_name(emulator: &MultiServiceRouter, domain: &str) {
    let (status, body) = call(
        emulator,
        "POST",
        "/v2/domainnames",
        Some(json!({
            "domainName": domain,
            "domainNameConfigurations": [{
                "certificateArn": engine::emulator_arn(
                    "acm",
                    REGION,
                    &format!("certificate/{domain}"),
                ),
            }],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "seeding domain name {domain:?}: expected 201 from CreateDomainName, got {status} {body}"
    );
}

/// Create one HTTP api with the stages `stage_names` names, and answer its `ApiId`.
async fn seed_api(emulator: &MultiServiceRouter, name: &str, stage_names: &[&str]) -> String {
    let (status, body) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "seeding api {name:?}: expected 201 from CreateApi, got {status} {body}"
    );
    let api_id = as_str(&body, "apiId");
    for stage_name in stage_names {
        let (status, stage) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": stage_name })),
        )
        .await;
        assert_eq!(
            status, 201,
            "seeding stage {stage_name:?} of api {name:?}: expected 201, got {status} {stage}"
        );
    }
    api_id
}

/// A fresh emulator holding [`DOMAIN`], [`NEIGHBOUR_DOMAIN`] and one api with the stages
/// [`STAGE`] and [`OTHER_STAGE`]. Answers `(emulator, api_id)`.
async fn seeded() -> (MultiServiceRouter, String) {
    let (emulator, api_id, _engine) = seeded_with_engine().await;
    (emulator, api_id)
}

/// [`seeded`], and the engine it was built from — for the rows that address a domain name by
/// its own `DomainNameId`, a value this model reports through no operation at all
/// ([`minted_domain_name_id`]).
async fn seeded_with_engine(
) -> (MultiServiceRouter, String, ::std::sync::Arc<engine::InMemoryEngine>) {
    let (emulator, engine) = crate::api_lifecycle::emulator_with_engine();
    seed_domain_name(&emulator, DOMAIN).await;
    seed_domain_name(&emulator, NEIGHBOUR_DOMAIN).await;
    let api_id = seed_api(&emulator, "ruled-api", &[STAGE, OTHER_STAGE]).await;
    (emulator, api_id, engine)
}

/// The simplest rule body an action table row can vary: one `invokeApi` action on
/// `stage`, one base-path condition, and `priority`.
fn simple_rule(api_id: &str, stage: &str, priority: i64) -> Value {
    json!({
        "priority": priority,
        "actions": [{ "invokeApi": { "apiId": api_id, "stage": stage } }],
        "conditions": [{ "matchBasePaths": { "anyOf": ["orders"] } }]
    })
}

/// One row of the create/read table: a whole rule body, with `{api}` standing in for the
/// `ApiId` of the seeded api.
struct RuleCase {
    label: &'static str,
    request: Value,
}

/// The rows span what a rule is made of: which condition kind it carries and how many,
/// how many values one `AnyOf` holds, how many actions it names, whether
/// `StripBasePath` is set, which stage is invoked, and where in the published priority
/// range it sits (both bounds included).
fn rule_cases() -> Vec<RuleCase> {
    vec![
        RuleCase {
            label: "one base path, the lowest published priority",
            request: json!({
                "priority": PRIORITY_MIN,
                "actions": [{ "invokeApi": { "apiId": "{api}", "stage": STAGE } }],
                "conditions": [{ "matchBasePaths": { "anyOf": ["orders"] } }]
            }),
        },
        RuleCase {
            label: "several base paths, the highest published priority",
            request: json!({
                "priority": PRIORITY_MAX,
                "actions": [{
                    "invokeApi": { "apiId": "{api}", "stage": OTHER_STAGE, "stripBasePath": true }
                }],
                "conditions": [{
                    "matchBasePaths": { "anyOf": ["orders", "orders/v1", "carts"] }
                }]
            }),
        },
        RuleCase {
            label: "one header match",
            request: json!({
                "priority": 42,
                "actions": [{
                    "invokeApi": { "apiId": "{api}", "stage": STAGE, "stripBasePath": false }
                }],
                "conditions": [{
                    "matchHeaders": {
                        "anyOf": [{ "header": "x-tenant", "valueGlob": "acme-*" }]
                    }
                }]
            }),
        },
        RuleCase {
            label: "the documented maximum of each condition kind",
            request: json!({
                "priority": 7,
                "actions": [{ "invokeApi": { "apiId": "{api}", "stage": OTHER_STAGE } }],
                "conditions": [
                    { "matchBasePaths": { "anyOf": ["admin"] } },
                    {
                        "matchHeaders": {
                            "anyOf": [
                                { "header": "x-tenant", "valueGlob": "acme-*" },
                                { "header": "x-tenant", "valueGlob": "globex-*" }
                            ]
                        }
                    },
                    {
                        "matchHeaders": {
                            "anyOf": [{ "header": "x-channel", "valueGlob": "*" }]
                        }
                    }
                ]
            }),
        },
        RuleCase {
            label: "two actions invoking two stages",
            request: json!({
                "priority": 999,
                "actions": [
                    { "invokeApi": { "apiId": "{api}", "stage": STAGE } },
                    { "invokeApi": { "apiId": "{api}", "stage": OTHER_STAGE } }
                ],
                "conditions": [{ "matchBasePaths": { "anyOf": ["two-actions"] } }]
            }),
        },
    ]
}

/// The request of `case` with `{api}` resolved to `api_id`.
fn resolved(case: &RuleCase, api_id: &str) -> Value {
    let rendered = case.request.to_string().replace("{api}", api_id);
    serde_json::from_str(&rendered).expect("the row is valid JSON")
}

#[tokio::test]
async fn create_routing_rule_persists_the_rule_list_routing_rules_reports_it_back() {
    let (emulator, api_id) = seeded().await;
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut created_rules: Vec<Value> = Vec::new();

    for case in rule_cases() {
        let label = case.label;
        let request = resolved(&case, &api_id);
        let (status, created) = create_routing_rule(&emulator, DOMAIN, &request).await;
        assert_eq!(
            status, 201,
            "case {label:?}: expected 201 from CreateRoutingRule, got {status} {created}"
        );

        let routing_rule_id = as_str(&created, "routingRuleId");
        assert_minted_id(label, "RoutingRuleId", &routing_rule_id);
        assert!(
            minted.insert(routing_rule_id.clone()),
            "case {label:?}: a minted RoutingRuleId must be new, got {routing_rule_id:?} twice"
        );

        // The rule's ARN names the resource path the rule is addressed at, under the
        // region the request was signed for.
        let expected_arn = engine::emulator_arn(
            "apigateway",
            REGION,
            &format!("/domainnames/{DOMAIN}/routingrules/{routing_rule_id}"),
        );
        assert_eq!(
            as_str(&created, "routingRuleArn"),
            expected_arn,
            "case {label:?}: expected the rule's own ARN, got {created}"
        );

        // Every member the caller sent is reported back as it was stored.
        assert_eq!(
            created["priority"], request["priority"],
            "case {label:?}: expected the Priority that was sent, got {created}"
        );
        assert_eq!(
            created["actions"], request["actions"],
            "case {label:?}: expected the Actions that were sent, got {created}"
        );
        assert_eq!(
            created["conditions"], request["conditions"],
            "case {label:?}: expected the Conditions that were sent, got {created}"
        );
        created_rules.push(created);
    }

    // The reader reports every stored rule, each exactly as the create reported it.
    let (status, body) = list_routing_rules(&emulator, DOMAIN, "").await;
    assert_eq!(
        status, 200,
        "expected 200 from ListRoutingRules, got {status} {body}"
    );
    let reported = listed(&body);
    assert_eq!(
        reported.len(),
        created_rules.len(),
        "expected every stored rule, got {body}"
    );
    for created in &created_rules {
        let id = as_str(created, "routingRuleId");
        let found = reported
            .iter()
            .find(|rule| as_str(rule, "routingRuleId") == id);
        assert_eq!(
            found,
            Some(created),
            "case {id:?}: the listing must report the rule the create reported, got {body}"
        );
    }

    // A rule belongs to the domain name it was created under, and to no other.
    let (status, neighbour) = list_routing_rules(&emulator, NEIGHBOUR_DOMAIN, "").await;
    assert_eq!(
        status, 200,
        "expected 200 from ListRoutingRules, got {status} {neighbour}"
    );
    assert!(
        listed(&neighbour).is_empty(),
        "expected no rule under a domain name none was created for, got {neighbour}"
    );
}

#[tokio::test]
async fn a_priority_outside_the_published_range_is_refused_and_both_bounds_are_accepted() {
    let (emulator, api_id) = seeded().await;

    // Outside the published range: both bounds crossed by one, and further out. The
    // recorded case that filed this issue is one of these rows.
    let refused = [
        ("one below the published minimum", PRIORITY_MIN - 1),
        ("far below the published minimum", -1),
        ("one above the published maximum", PRIORITY_MAX + 1),
        ("far above the published maximum", 2_000_000),
    ];
    for (label, priority) in refused {
        let (status, body) = create_routing_rule(
            &emulator,
            DOMAIN,
            &simple_rule(&api_id, STAGE, priority),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 for Priority {priority}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {label:?}: expected BadRequestException, got {body}"
        );
        // The refusal is stated in the member's own terms: the whole published range it
        // has to fall in, with the bound written the way a number of that size is, and
        // however far outside it the value sat.
        assert_eq!(
            as_str(&body, "message"),
            "Priority must be between 1 and 1,000,000.",
            "case {label:?}: expected the published range to be stated, got {body}"
        );
        // And the sentence is built from the modelled bounds rather than written twice.
        assert_eq!(
            as_str(&body, "message"),
            routing_rule_priority_range_message(),
            "case {label:?}: expected the refusal the state helper words, got {body}"
        );
    }

    // In range, including both published bounds: a bound is IN the range.
    for priority in [PRIORITY_MIN, 2, 500_000, PRIORITY_MAX - 1, PRIORITY_MAX] {
        let (status, created) = create_routing_rule(
            &emulator,
            DOMAIN,
            &simple_rule(&api_id, STAGE, priority),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {priority}: a published Priority must be accepted, got {status} {created}"
        );
        assert_eq!(
            created["priority"].as_i64(),
            Some(priority),
            "case {priority}: expected the Priority that was sent, got {created}"
        );
    }

    // The refused calls persisted nothing: only the accepted ones are stored.
    let (_, body) = list_routing_rules(&emulator, DOMAIN, "").await;
    assert_eq!(
        listed(&body).len(),
        5,
        "expected only the accepted rules to be stored, got {body}"
    );
}

#[tokio::test]
async fn a_rule_that_describes_nothing_evaluable_is_refused_and_persists_nothing() {
    let (emulator, api_id) = seeded().await;
    let action = json!({ "invokeApi": { "apiId": api_id, "stage": STAGE } });
    let base_paths = json!({ "matchBasePaths": { "anyOf": ["orders"] } });
    let headers = json!({
        "matchHeaders": { "anyOf": [{ "header": "x-tenant", "valueGlob": "acme-*" }] }
    });

    // (the case, the request, the sentence the refusal must carry)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "no action at all",
            json!({ "priority": 1, "actions": [], "conditions": [base_paths.clone()] }),
            EMPTY_ROUTING_RULE_ACTIONS,
        ),
        (
            "no condition at all",
            json!({ "priority": 1, "actions": [action.clone()], "conditions": [] }),
            EMPTY_ROUTING_RULE_CONDITIONS,
        ),
        (
            "a condition naming neither alternative",
            json!({ "priority": 1, "actions": [action.clone()], "conditions": [{}] }),
            ROUTING_RULE_CONDITION_ALTERNATIVES,
        ),
        (
            "a condition naming both alternatives",
            json!({
                "priority": 1,
                "actions": [action.clone()],
                "conditions": [{
                    "matchBasePaths": { "anyOf": ["orders"] },
                    "matchHeaders": {
                        "anyOf": [{ "header": "x-tenant", "valueGlob": "acme-*" }]
                    }
                }]
            }),
            ROUTING_RULE_CONDITION_ALTERNATIVES,
        ),
        (
            "a base-path condition with nothing to match on",
            json!({
                "priority": 1,
                "actions": [action.clone()],
                "conditions": [{ "matchBasePaths": { "anyOf": [] } }]
            }),
            "AnyOf",
        ),
        (
            "a header condition with nothing to match on",
            json!({
                "priority": 1,
                "actions": [action.clone()],
                "conditions": [{ "matchHeaders": { "anyOf": [] } }]
            }),
            "AnyOf",
        ),
        (
            "one more header condition than the documented maximum",
            json!({
                "priority": 1,
                "actions": [action.clone()],
                "conditions": [headers.clone(), headers.clone(), headers.clone()]
            }),
            ROUTING_RULE_CONDITION_LIMITS,
        ),
        (
            "one more base-path condition than the documented maximum",
            json!({
                "priority": 1,
                "actions": [action.clone()],
                "conditions": [base_paths.clone(), base_paths.clone()]
            }),
            ROUTING_RULE_CONDITION_LIMITS,
        ),
    ];

    for (label, request, expected) in rows {
        let (status, body) = create_routing_rule(&emulator, DOMAIN, &request).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from CreateRoutingRule, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {label:?}: expected BadRequestException, got {body}"
        );
        let message = as_str(&body, "message");
        assert!(
            message.contains(expected),
            "case {label:?}: expected a message naming {expected:?}, got {message:?}"
        );
    }

    // Nothing the refusals named reached the store.
    let (_, body) = list_routing_rules(&emulator, DOMAIN, "").await;
    assert!(
        listed(&body).is_empty(),
        "expected the refused calls to have persisted nothing, got {body}"
    );

    // The positive control: the same rule, within the documented condition counts, IS
    // stored — so the refusals above are about those rows and not about the shape of
    // every rule.
    let (status, created) = create_routing_rule(
        &emulator,
        DOMAIN,
        &json!({
            "priority": 1,
            "actions": [action],
            "conditions": [base_paths, headers.clone(), headers]
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected the documented maximum of each condition kind to be accepted, got {status} {created}"
    );
}

#[tokio::test]
async fn an_action_is_stored_as_the_reference_the_request_wrote() {
    let (emulator, api_id) = seeded().await;
    // An api of its own, so one row's action can name a stage that exists on ANOTHER api.
    let neighbour = seed_api(&emulator, "neighbour-api", &["staging"]).await;

    // An action's `InvokeApi` is a REFERENCE the rule carries, not a resource the rule is
    // created under: the write keeps it as it was written instead of resolving it, so the
    // rows span a live api stage, another api's stage, a stage name no api holds, and an
    // api id the account does not hold at all.
    let rows = [
        ("a live api and its own stage", api_id.as_str(), STAGE),
        ("a live api and its other stage", api_id.as_str(), OTHER_STAGE),
        ("another api and its own stage", neighbour.as_str(), "staging"),
        ("a live api and a stage nothing created", api_id.as_str(), "nosuchstage"),
        ("an api id nothing created", "abcdefghij", STAGE),
    ];
    let mut expected: Vec<(String, Value)> = Vec::new();
    for (index, (label, api, stage)) in rows.iter().enumerate() {
        let priority = i64::try_from(index).unwrap_or(0) + 1;
        let request = simple_rule(api, stage, priority);
        let (status, created) = create_routing_rule(&emulator, DOMAIN, &request).await;
        assert_eq!(
            status, 201,
            "case {label:?}: expected 201 from CreateRoutingRule, got {status} {created}"
        );
        assert_eq!(
            created["actions"], request["actions"],
            "case {label:?}: expected the Actions that were sent, got {created}"
        );
        expected.push((as_str(&created, "routingRuleId"), created["actions"].clone()));
    }

    // And the reference the listing reports is the one that was stored.
    let (status, body) = list_routing_rules(&emulator, DOMAIN, "").await;
    assert_eq!(
        status, 200,
        "expected 200 from ListRoutingRules, got {status} {body}"
    );
    let reported = listed(&body);
    assert_eq!(
        reported.len(),
        expected.len(),
        "expected every stored rule to be reported, got {body}"
    );
    for (id, actions) in &expected {
        let found = reported
            .iter()
            .find(|rule| as_str(rule, "routingRuleId") == *id)
            .unwrap_or_else(|| panic!("case {id:?}: the stored rule must be reported, got {body}"));
        assert_eq!(
            found["actions"], *actions,
            "case {id:?}: expected the stored Actions, got {found}"
        );
    }
}

/// Walk `domain`'s rules in pages of `page_size`, answering every `RoutingRuleId` seen in
/// the order the chain reported them.
async fn walk(
    emulator: &MultiServiceRouter,
    domain: &str,
    page_size: usize,
    total: usize,
) -> Vec<String> {
    let label = format!("{total} rules in pages of {page_size}");
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One more iteration than pages are possible, so a token that never clears is caught
    // instead of looping forever.
    for _ in 0..total + 2 {
        let query = match &token {
            Some(t) => format!("?maxResults={page_size}&nextToken={t}"),
            None => format!("?maxResults={page_size}"),
        };
        let (status, body) = list_routing_rules(emulator, domain, &query).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from ListRoutingRules, got {status} {body}"
        );
        let page = listed(&body);
        assert!(
            page.len() <= page_size,
            "case {label:?}: a page holds at most {page_size} rules, got {body}"
        );
        seen.extend(page.iter().map(|rule| as_str(rule, "routingRuleId")));
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case {label:?}: the walk must end, got a token that never cleared"
    );
    seen
}

#[tokio::test]
async fn list_routing_rules_pages_the_rules_of_the_named_domain_name() {
    // The rule holds for any rule count and any page size, so the table walks both: each
    // count is paged at every size from 1 up to one past it.
    for total in [0_usize, 1, 2, 5, 11] {
        let (emulator, api_id) = seeded().await;
        let mut expected: BTreeSet<String> = BTreeSet::new();
        for index in 0..total {
            let (status, created) = create_routing_rule(
                &emulator,
                DOMAIN,
                &simple_rule(&api_id, STAGE, i64::try_from(index).unwrap_or(1) + 1),
            )
            .await;
            assert_eq!(
                status, 201,
                "case total={total}: expected 201 from CreateRoutingRule, got {status} {created}"
            );
            expected.insert(as_str(&created, "routingRuleId"));
        }

        for page_size in 1..=total + 1 {
            let label = format!("{total} rules in pages of {page_size}");
            let seen = walk(&emulator, DOMAIN, page_size, total).await;
            assert_eq!(
                seen.len(),
                total,
                "case {label:?}: expected {total} rules across the chain, got {seen:?}"
            );
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<_>>(),
                expected,
                "case {label:?}: expected exactly the stored rules, got {seen:?}"
            );
            let mut sorted = seen.clone();
            sorted.sort();
            assert_eq!(
                sorted, seen,
                "case {label:?}: a paged walk reports the collection in key order, got {seen:?}"
            );
        }

        // An unpaged read reports every rule and has nothing left to resume.
        let (status, body) = list_routing_rules(&emulator, DOMAIN, "").await;
        assert_eq!(
            status, 200,
            "case total={total}: expected 200 from ListRoutingRules, got {status} {body}"
        );
        assert_eq!(
            listed(&body).len(),
            total,
            "case total={total}: an unpaged ListRoutingRules reports every rule, got {body}"
        );
        assert!(
            body.get("nextToken").is_none() || body["nextToken"].is_null(),
            "case total={total}: an unpaged ListRoutingRules has nothing to resume, got {body}"
        );

        // A listing is a report: repeating it changes nothing a later caller sees.
        let (_, again) = list_routing_rules(&emulator, DOMAIN, "").await;
        assert_eq!(
            again, body,
            "case total={total}: reading twice must report the same rules, got {again}"
        );
    }
}

/// The `DomainNameId` this store minted for `domain`, read out of the store itself: no
/// operation of this model reports the id, and the address a domain name publishes is the one
/// its own socket answers on rather than a name that spells the id
/// (`crate::service::domain_name_socket`).
fn minted_domain_name_id(
    engine: &::std::sync::Arc<engine::InMemoryEngine>,
    domain: &str,
) -> String {
    let state = engine.state::<
        service_com_amazonaws_apigatewayv2_apigatewayv2::ComAmazonawsApigatewayv2Apigatewayv2State,
    >();
    let store = state.lock();
    store
        .domain_names
        .get(domain)
        .map(|record| record.domain_name_id.clone())
        .unwrap_or_default()
}

#[tokio::test]
async fn an_operation_naming_a_domain_name_the_account_does_not_hold_is_refused() {
    let (emulator, api_id, engine) = seeded_with_engine().await;
    let (status, created) =
        create_routing_rule(&emulator, DOMAIN, &simple_rule(&api_id, STAGE, 1)).await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateRoutingRule, got {status} {created}"
    );

    // The rows span the forms an absent domain name takes: a name nothing created, a
    // held name spelled differently, and a name the account holds under another
    // `DomainNameId` than the request named.
    let absent_names = [
        ("a domain name nothing created", "absent.example.com".to_string()),
        ("a held domain name in another case", DOMAIN.to_uppercase()),
        (
            "a held domain name with a trailing dot",
            format!("{DOMAIN}."),
        ),
    ];
    for (label, domain) in &absent_names {
        let (status, body) = list_routing_rules(&emulator, domain, "").await;
        assert_refused_as_absent(label, status, &body);
        let (status, body) =
            create_routing_rule(&emulator, domain, &simple_rule(&api_id, STAGE, 2)).await;
        assert_refused_as_absent(label, status, &body);
    }

    // A `DomainNameId` beside the name addresses the domain name that CARRIES it, so an
    // id no domain name carries and another domain name's own id both name a resource
    // this account does not hold under this name.
    let foreign = minted_domain_name_id(&engine, NEIGHBOUR_DOMAIN);
    let wrong_ids = [
        ("a DomainNameId no domain name carries", "0000000000"),
        ("the DomainNameId of ANOTHER domain name", foreign.as_str()),
    ];
    for (label, id) in wrong_ids {
        let (status, body) =
            list_routing_rules(&emulator, DOMAIN, &format!("?domainNameId={id}")).await;
        assert_refused_as_absent(label, status, &body);
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/domainnames/{DOMAIN}/routingrules?domainNameId={id}"),
            Some(simple_rule(&api_id, STAGE, 2)),
        )
        .await;
        assert_refused_as_absent(label, status, &body);
    }

    // The positive control: the domain name's OWN id addresses it, and the rule stored
    // under it is reported.
    let own = minted_domain_name_id(&engine, DOMAIN);
    assert!(!own.is_empty(), "expected a minted DomainNameId for {DOMAIN}");
    let (status, body) =
        list_routing_rules(&emulator, DOMAIN, &format!("?domainNameId={own}")).await;
    assert_eq!(
        status, 200,
        "expected the domain name's own id to address it, got {status} {body}"
    );
    assert_eq!(
        listed(&body).len(),
        1,
        "expected the stored rule to be reported, got {body}"
    );
}

/// Assert one response is the modelled refusal of a request naming a domain name the
/// account does not hold: 404 `NotFoundException`, carrying the one sentence this service
/// refuses an unusable domain name identifier with.
fn assert_refused_as_absent(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404 for a domain name the account does not hold, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "NotFoundException",
        "case {case:?}: expected NotFoundException, got {body}"
    );
    assert_eq!(
        as_str(body, "message"),
        INVALID_DOMAIN_NAME_IDENTIFIER,
        "case {case:?}: expected the domain-name-identifier refusal, got {body}"
    );
}

#[tokio::test]
async fn caller_driven_growth_is_bounded() {
    let (emulator, api_id) = seeded().await;

    for index in 0..MAX_ROUTING_RULES_PER_DOMAIN {
        let priority = i64::try_from(index).unwrap_or(0) + 1;
        let (status, created) =
            create_routing_rule(&emulator, DOMAIN, &simple_rule(&api_id, STAGE, priority)).await;
        assert_eq!(
            status, 201,
            "case rule {index}: expected 201 while under the bound, got {status} {created}"
        );
    }

    let (status, body) = create_routing_rule(
        &emulator,
        DOMAIN,
        &simple_rule(&api_id, STAGE, PRIORITY_MAX),
    )
    .await;
    assert_eq!(
        status, 429,
        "expected 429 once the bound is reached, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "expected TooManyRequestsException, got {body}"
    );

    // The refusal did not drop a rule the account still holds — one bounded page of them
    // is reported — and another domain name has a bound of its own.
    let (_, body) = list_routing_rules(&emulator, DOMAIN, "?maxResults=100").await;
    assert_eq!(
        listed(&body).len(),
        MAX_PAGE_SIZE.min(MAX_ROUTING_RULES_PER_DOMAIN),
        "expected a bounded page of the stored rules, got {body}"
    );
    let (status, created) =
        create_routing_rule(&emulator, NEIGHBOUR_DOMAIN, &simple_rule(&api_id, STAGE, 1)).await;
    assert_eq!(
        status, 201,
        "expected another domain name to have a bound of its own, got {status} {created}"
    );
}

#[tokio::test]
async fn a_next_token_this_reader_did_not_issue_is_refused() {
    let (emulator, api_id) = seeded().await;
    for priority in 1..=3 {
        let (status, created) =
            create_routing_rule(&emulator, DOMAIN, &simple_rule(&api_id, STAGE, priority)).await;
        assert_eq!(
            status, 201,
            "expected 201 from CreateRoutingRule, got {status} {created}"
        );
    }
    let (status, created) =
        create_routing_rule(&emulator, NEIGHBOUR_DOMAIN, &simple_rule(&api_id, STAGE, 1)).await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateRoutingRule, got {status} {created}"
    );

    let (_, first) = list_routing_rules(&emulator, DOMAIN, "?maxResults=1").await;
    let issued = as_str(&first, "nextToken");
    assert!(!issued.is_empty(), "expected a NextToken, got {first}");

    // The rows span what can arrive in place of a token this collection issued: text
    // that is not hex, hex that decodes to nothing meaningful, a token far longer than
    // any this service mints, and another collection's token.
    let mut rows: Vec<(&str, String)> = vec![
        ("a word", "not-a-token".to_string()),
        ("an odd number of hex digits", "abc".to_string()),
        ("hex whose bytes are not UTF-8", "ff".to_string()),
        ("a token past any this service mints", "61".repeat(4_096)),
    ];
    // Another domain name's own token names no position in this collection.
    let (_, neighbour_first) =
        list_routing_rules(&emulator, NEIGHBOUR_DOMAIN, "?maxResults=1").await;
    if let Some(foreign) = neighbour_first.get("nextToken").and_then(Value::as_str) {
        rows.push((
            "a token issued for another domain name's rules",
            foreign.to_string(),
        ));
    }

    for (label, token) in &rows {
        let (status, body) =
            list_routing_rules(&emulator, DOMAIN, &format!("?nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from ListRoutingRules, got {status} {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            INVALID_NEXT_TOKEN_MESSAGE,
            "case {label:?}: expected the invalid-position refusal, got {body}"
        );
    }

    // The positive control: the token this reader DID issue resumes the walk.
    let (status, resumed) =
        list_routing_rules(&emulator, DOMAIN, &format!("?maxResults=1&nextToken={issued}")).await;
    assert_eq!(
        status, 200,
        "expected the issued token to resume the walk, got {status} {resumed}"
    );
    assert_eq!(
        listed(&resumed).len(),
        1,
        "expected the resumed page to carry one rule, got {resumed}"
    );
}
