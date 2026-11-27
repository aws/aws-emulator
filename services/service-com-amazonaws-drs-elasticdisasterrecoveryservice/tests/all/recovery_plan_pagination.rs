// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for enumerating the account's DRS Recovery Plans.
//!
//! Every test drives the service the way the router does — one buffered
//! `http::Request` per call through `MultiServiceRouter::dispatch` — so what is
//! asserted is the wire behaviour (status, error code, JSON body), not a handler's
//! Rust return value. The harness (the router, `call_in`, `Answer`) is the one
//! `replication_configuration_template_lifecycle` builds.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. The enumeration walks every plan the store holds EXACTLY once, in a stable order
//!    (the order the plans were created in), whatever page size the caller asks for,
//!    and it changes nothing a later caller sees: a second walk reports the same plans.
//!    A store holding no plan reports an empty list and no token.
//! 2. One page is bounded by the service's own page limit, however many plans the store
//!    holds and however large a `maxResults` the model lets the caller ask for;
//!    anything past the page is reached through the token.
//! 3. A listed plan is the SAME plan the single-plan read reports: every member the
//!    summary carries equals that plan's, and the summary carries exactly the members
//!    the model gives it.
//! 4. A `nextToken` this operation did not mint — a string that decodes to nothing, a
//!    token naming a plan the store no longer holds, or a token another paginated
//!    operation minted — is refused as a 400 `ValidationException` naming `nextToken`,
//!    and a `maxResults` outside the modelled range is refused too; neither changes the
//!    store.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_drs_elasticdisasterrecoveryservice::state::ComAmazonawsDrsElasticdisasterrecoveryserviceState as DrsState;
use service_com_amazonaws_drs_elasticdisasterrecoveryservice::state::PAGE_LIMIT;
use smithy_server_runtime::MultiServiceRouter;

use crate::recovery_plan_lifecycle::stored_plans;
use crate::replication_configuration_template_lifecycle::{
    call_in, create_body, drs_with_engine, items, Answer,
};

/// The Region every request in this suite signs for.
const REGION: &str = "eu-west-1";

/// The members the model gives `RecoveryPlanSummary`, all five required
/// (`smithy_query(query="shape", shape="RecoveryPlanSummary")`).
pub(crate) const SUMMARY_MEMBERS: [&str; 5] = [
    "recoveryPlanArn",
    "name",
    "status",
    "createdAt",
    "updatedAt",
];

/// Create a plan and return the ARN it minted, failing the test on anything but the
/// modelled 201.
async fn create_plan(router: &MultiServiceRouter, body: &Value) -> String {
    let answer = call_in(router, REGION, "CreateRecoveryPlan", body).await;
    assert_eq!(
        answer.status, 201,
        "create {body}: expected 201, got {answer:?}"
    );
    answer
        .member("recoveryPlan")
        .and_then(|plan| plan.get("recoveryPlanArn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("create {body}: no recoveryPlanArn in {answer:?}"))
        .to_string()
}

/// Create `count` plans, each under its own name, and return their ARNs in creation
/// order.
async fn create_plans(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let mut arns = Vec::with_capacity(count);
    for index in 0..count {
        // Each name differs, so no create is served by the idempotency path; a digit
        // and a space are both within `RecoveryPlanName`'s pattern.
        arns.push(create_plan(router, &json!({"name": format!("Plan {index}")})).await);
    }
    arns
}

/// One `ListRecoveryPlans` call: `maxResults` and `nextToken` are sent only when named.
async fn list(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Answer {
    let mut body = json!({});
    if let Some(max_results) = max_results {
        body["maxResults"] = json!(max_results);
    }
    if let Some(token) = next_token {
        body["nextToken"] = json!(token);
    }
    call_in(router, REGION, "ListRecoveryPlans", &body).await
}

/// The `recoveryPlans` list of an answer — modelled required, so an answer without it
/// is a failure rather than an empty page.
pub(crate) fn plans_of(answer: &Answer) -> &Vec<Value> {
    answer
        .member("recoveryPlans")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a recoveryPlans list, got {answer:?}"))
}

/// The ARN a reported summary carries.
fn arn_of(summary: &Value) -> String {
    summary
        .get("recoveryPlanArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a recoveryPlanArn, got {summary}"))
        .to_string()
}

/// Walk the whole enumeration at `max_results`, following `nextToken` to the end, and
/// return every summary reported, in the order reported.
///
/// The walk is bounded by the number of plans the store holds plus one: a page carries
/// at least one plan while any remain, so a correct enumeration cannot need more steps
/// than that — and a token that resumed in the wrong place would be caught here rather
/// than looping forever.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<i64>,
    stored: usize,
) -> Vec<Value> {
    let expected_page = usize::try_from(
        max_results
            .unwrap_or(i64::from(PAGE_LIMIT))
            .min(i64::from(PAGE_LIMIT)),
    )
    .expect("a page size in 1..=PAGE_LIMIT fits in usize");

    let mut reported: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for step in 0..=stored + 1 {
        let answer = list(router, max_results, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case}, page {step}: expected 200, got {answer:?}"
        );
        let page = plans_of(&answer);
        assert!(
            page.len() <= expected_page,
            "case {case}, page {step}: a page carries at most {expected_page} plans, \
             got {}",
            page.len()
        );
        reported.extend(page.iter().cloned());
        token = answer
            .member("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        match &token {
            // A token is minted only when a plan actually follows, so the page that
            // carries one is full.
            Some(_) => assert_eq!(
                page.len(),
                expected_page,
                "case {case}, page {step}: a page followed by a token must be full, \
                 got {answer:?}"
            ),
            None => return reported,
        }
    }
    panic!(
        "case {case}: the enumeration of {stored} plans did not end, reported {} \
         summaries",
        reported.len()
    )
}

// ---------------------------------------------------------------------------
// Rule 1 — every plan exactly once, in creation order, at any page size
// ---------------------------------------------------------------------------

/// Whatever the store holds and whatever page size the caller asks for, the walk
/// reports each stored plan exactly once in creation order, and a second walk reports
/// the same plans: the read changes nothing. The rows cross store sizes (including a
/// store holding none, one, and more plans than a page carries) with the page sizes the
/// model allows, from the smallest to the largest.
#[tokio::test]
async fn every_stored_plan_is_reported_exactly_once_at_any_page_size() {
    // `maxResults` is modelled `@range(min: 1, max: 1000)`; `None` is the request that
    // names none.
    let page_sizes: [Option<i64>; 6] = [None, Some(1), Some(2), Some(3), Some(100), Some(1000)];

    for stored in [0_usize, 1, 2, 5] {
        let (router, engine) = drs_with_engine();
        let created = create_plans(&router, stored).await;

        for max_results in page_sizes {
            let case = format!("{stored} plans at maxResults {max_results:?}");
            let reported = walk(&router, &case, max_results, stored).await;
            let reported_arns: Vec<String> = reported.iter().map(arn_of).collect();
            assert_eq!(
                reported_arns, created,
                "case {case}: the walk must report every stored plan exactly once, in \
                 creation order, got {reported_arns:?}"
            );
            assert_eq!(
                reported_arns.iter().collect::<BTreeSet<_>>().len(),
                created.len(),
                "case {case}: no plan may be reported twice, got {reported_arns:?}"
            );

            // The walk is a read: the store it walked is the store it leaves.
            let again = walk(&router, &case, max_results, stored).await;
            assert_eq!(
                again, reported,
                "case {case}: a second walk must report the same plans, got {again:?}"
            );
            assert_eq!(
                stored_plans(&engine),
                stored,
                "case {case}: enumerating the plans must store nothing"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — one page is bounded by the service's page limit
// ---------------------------------------------------------------------------

/// A store holding more plans than one page carries serves exactly the page limit and a
/// token, however large a `maxResults` the caller asks for — and the token walks the
/// rest. The rows are the page sizes at and above the service's limit, plus the request
/// that names none.
#[tokio::test]
async fn one_page_carries_at_most_the_services_page_limit() {
    let limit = usize::try_from(PAGE_LIMIT).expect("the page limit fits in usize");
    let stored = limit + 3;

    let (router, _engine) = drs_with_engine();
    let created = create_plans(&router, stored).await;

    for max_results in [None, Some(i64::from(PAGE_LIMIT)), Some(500), Some(1000)] {
        let case = format!("{stored} plans at maxResults {max_results:?}");
        let first = list(&router, max_results, None).await;
        assert_eq!(first.status, 200, "case {case}: got {first:?}");
        assert_eq!(
            plans_of(&first).len(),
            limit,
            "case {case}: the first page must carry the page limit, got {first:?}"
        );
        assert!(
            first.member("nextToken").is_some(),
            "case {case}: a bounded first page must carry a token, got {first:?}"
        );

        // What the page left out is exactly what the token resumes at.
        let reported = walk(&router, &case, max_results, stored).await;
        let reported_arns: Vec<String> = reported.iter().map(arn_of).collect();
        assert_eq!(
            reported_arns, created,
            "case {case}: the token must walk the plans the first page left out"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3 — a listed plan is the plan the single-plan read reports
// ---------------------------------------------------------------------------

/// Every member of a listed summary equals the member of the same name on the plan
/// `GetRecoveryPlan` reports for that ARN, and the summary carries exactly the five
/// members the model gives it — no `description` and no `tags`. The rows differ along
/// what a plan can carry: a bare name, a description, several tags, and both.
#[tokio::test]
async fn each_listed_summary_is_the_plan_read_back_by_its_arn() {
    let bodies: Vec<(&str, Value)> = vec![
        ("a bare name", json!({"name": "Bare"})),
        (
            "a description",
            json!({"name": "Described", "description": "Recovers the order tier"}),
        ),
        (
            "several tags",
            json!({"name": "Tagged", "tags": {"env": "prod", "tier": "1"}}),
        ),
        (
            "a description and tags",
            json!({
                "name": "Both 2",
                "description": "",
                "tags": {"env": "dev"},
            }),
        ),
    ];

    let (router, _engine) = drs_with_engine();
    let mut created = Vec::new();
    for (_, body) in &bodies {
        created.push(create_plan(&router, body).await);
    }

    let reported = walk(&router, "every plan", None, created.len()).await;
    assert_eq!(
        reported.len(),
        created.len(),
        "expected every created plan to be listed, got {reported:?}"
    );

    for ((case, _), summary) in bodies.iter().zip(&reported) {
        let members: BTreeSet<&str> = summary
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: expected a summary object, got {summary}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members,
            SUMMARY_MEMBERS.into_iter().collect::<BTreeSet<_>>(),
            "case {case}: a summary carries exactly the modelled members, got {summary}"
        );

        let arn = arn_of(summary);
        let read = call_in(
            &router,
            REGION,
            "GetRecoveryPlan",
            &json!({"recoveryPlanArn": arn}),
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the listed plan to read back, got {read:?}"
        );
        let plan = read
            .member("recoveryPlan")
            .unwrap_or_else(|| panic!("case {case}: no recoveryPlan in {read:?}"));
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                summary.get(member),
                plan.get(member),
                "case {case}: the listed {member} must be the stored one, got \
                 {summary} and {plan}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4 — a token this operation did not mint, and a page size out of range
// ---------------------------------------------------------------------------

/// A `nextToken` this operation did not mint is refused with HTTP 400
/// `ValidationException` naming `nextToken`, and a `maxResults` outside the modelled
/// range is refused with 400 as well. Neither changes the store, and the enumeration
/// still walks every plan afterwards. The rows are tokens absent for different reasons:
/// a string that decodes to nothing, a token whose plan the store no longer holds, and
/// a token another paginated operation of this service minted.
#[tokio::test]
async fn a_token_this_operation_did_not_mint_is_refused() {
    let (router, engine) = drs_with_engine();
    let created = create_plans(&router, 3).await;

    // A token this operation DID mint, over plans later deleted: it names a plan the
    // store no longer holds.
    let first = list(&router, Some(1), None).await;
    let stale = first
        .member("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a token over three plans, got {first:?}"))
        .to_string();

    // A token another paginated operation minted, over its own collection.
    for subnet in ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"] {
        let answer = call_in(
            &router,
            REGION,
            "CreateReplicationConfigurationTemplate",
            &create_body(subnet, json!({})),
        )
        .await;
        assert_eq!(answer.status, 201, "template create: got {answer:?}");
    }
    let templates = call_in(
        &router,
        REGION,
        "DescribeReplicationConfigurationTemplates",
        &json!({"maxResults": 1}),
    )
    .await;
    assert_eq!(templates.status, 200, "template describe: got {templates:?}");
    assert_eq!(
        items(&templates).len(),
        1,
        "template describe: expected one item, got {templates:?}"
    );
    let foreign = templates
        .member("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a token over two templates, got {templates:?}"))
        .to_string();

    // Drop the plan the stale token resumes at, so it names a plan nothing holds.
    // Written through the engine because the operation that removes a plan
    // (`DeleteRecoveryPlan`) is still a scaffold stub, so no served call can take a
    // plan out of the store.
    {
        let state = engine.state::<DrsState>();
        let mut store = state.lock();
        for arn in &created[1..] {
            let id = arn
                .split_once(":recovery-plan/")
                .unwrap_or_else(|| panic!("expected a recovery-plan ARN, got {arn:?}"))
                .1
                .to_string();
            assert!(
                store.recovery_plans.remove(&id).is_some(),
                "expected the store to hold plan {id}"
            );
        }
    }
    let stored = stored_plans(&engine);
    assert_eq!(stored, 1, "expected one plan left, got {stored}");

    let refused: Vec<(&str, String)> = vec![
        ("a token that decodes to nothing", "not a token".to_string()),
        ("an empty token", String::new()),
        (
            "a token of the greatest modelled length",
            "t".repeat(2048),
        ),
        ("a token naming a plan nothing holds", stale),
        ("a token another paginated operation minted", foreign),
    ];
    for (case, token) in &refused {
        let answer = list(&router, None, Some(token)).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400 for {token:?}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {answer:?}"
        );
        let named = answer
            .member("fieldList")
            .and_then(Value::as_array)
            .is_some_and(|fields| {
                fields
                    .iter()
                    .any(|field| field.get("name") == Some(&json!("nextToken")))
            });
        assert!(
            named,
            "case {case}: the refusal must name nextToken, got {answer:?}"
        );
        assert_eq!(
            stored_plans(&engine),
            stored,
            "case {case}: a refused read must store nothing"
        );
    }

    // `maxResults` is modelled `@range(min: 1, max: 1000)`: each published bound is
    // served and one past it is refused.
    for max_results in [1, 1000] {
        let answer = list(&router, Some(max_results), None).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults {max_results} (at the bound): got {answer:?}"
        );
    }
    for max_results in [0, 1001] {
        let answer = list(&router, Some(max_results), None).await;
        assert_eq!(
            answer.status, 400,
            "case maxResults {max_results} (past the bound): expected 400, got {answer:?}"
        );
    }

    // Every refusal left the enumeration working: the plan still held is still walked.
    let reported = walk(&router, "after the refusals", None, stored).await;
    let reported_arns: Vec<String> = reported.iter().map(arn_of).collect();
    assert_eq!(
        reported_arns,
        created[..1].to_vec(),
        "a refused read must leave the enumeration reporting the stored plans"
    );
}
