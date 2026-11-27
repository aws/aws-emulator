// SPDX-License-Identifier: CC0-1.0

//! The account-level Amazon Web Services Support subscription gate, end to end over
//! the same router the emulator boots.
//!
//! RULE: the Health API is served only to an account holding a qualifying Amazon Web
//! Services Support plan; a call from an account without one is refused with HTTP 400
//! and the error code `SubscriptionRequiredException`, for EVERY operation the service
//! binds and before the request's input is validated, before any stored event is
//! looked up, and without changing anything the next caller reads.
//!
//! Source of the rule: this service's own documentation
//! (`smithy_query(query="doc",shape="AWSHealth_20160804")`) — "You must have a
//! Business, Enterprise On-Ramp, or Enterprise Support plan from Amazon Web Services
//! Support to use the Health API. If you call the Health API from an Amazon Web
//! Services account that doesn't have a Business, Enterprise On-Ramp, or Enterprise
//! Support plan, you receive a `SubscriptionRequiredException` error."
//!
//! Each suite asserts that rule over a SET of inputs: the operation (every one the
//! service binds — implemented readers and stubs alike), the shape of the input
//! (complete, empty, outside a modelled bound, naming an event that does not exist),
//! and the plan the account holds (every plan the documentation names, in several
//! spellings, plus the plans that do not qualify).

use std::sync::Arc;

use serde_json::{json, Value};

use super::health_event_discovery::{
    call, call_ok, error_code, health_router_over, set_support_plan, SUBSCRIBED_PLAN,
};

/// The wire code and status AWS answers a Health API call from an account with no
/// qualifying plan with. The shape is absent from this service's model
/// (`smithy_query(shape="SubscriptionRequiredException")` finds nothing), so this pair
/// — not the message text, which neither the model nor the documentation fixes — is
/// what a client can rely on.
const SUBSCRIPTION_REQUIRED: &str = "SubscriptionRequiredException";
const SUBSCRIPTION_REQUIRED_STATUS: u16 = 400;

/// The code a bound-but-scaffolded operation answers for a SUBSCRIBED account, so a
/// row covering one can tell "not written yet" from the subscription refusal.
const NOT_IMPLEMENTED: &str = "NotImplemented";
const NOT_IMPLEMENTED_STATUS: u16 = 501;

/// An event ARN of the shape `eventArn` documents
/// (`arn:aws:health:<region>::event/<SERVICE>/<EVENT_TYPE_CODE>/<EVENT_TYPE_PLUS_ID>`)
/// that the seeded catalog does not hold, so a row carrying it would reach the
/// per-item `EventNotFound` path — or an empty aggregate — if the gate did not precede
/// the catalog lookup.
const ABSENT_EVENT_ARN: &str =
    "arn:aws:health:us-east-1::event/EC2/AWS_EC2_ABSENT_EVENT/AWS_EC2_ABSENT_EVENT_0000";

/// Every operation this service BINDS, with an input to call it with, and whether it
/// is implemented (a subscribed account gets a 200) or still a scaffold (501).
///
/// The operation names and their required members are the model's: see `OPERATIONS.md`,
/// rendered from `smithy-model/com-amazonaws-health-awshealth-20160804.json`. The
/// eleven operations whose input is `Unit` (the two `*HealthServiceAccess*` switches
/// and `DescribeHealthServiceStatusForOrganization`, plus the tag switches and key
/// readers — `DescribeTagKeys`, `DescribeTagKeysForOrganization`,
/// `DescribeTagsOnboardingStatus`, `DescribeTagsOnboardingStatusForOrganization`,
/// `EnableTags`, `EnableTagsForOrganization`, `DisableTags`,
/// `DisableTagsForOrganization`) are NOT in this crate's generated routing table at
/// all, so they are not bound and are not listed here; that gap is its own issue (see
/// `src/ops/SKIPPED_STUBS.md`).
fn every_bound_operation() -> Vec<(&'static str, Value, bool)> {
    vec![
        (
            "ConfigureTags",
            json!({ "tagKeys": [{ "key": "Name" }] }),
            true,
        ),
        (
            "ConfigureTagsForOrganization",
            json!({ "tagKeys": [{ "key": "Name" }] }),
            true,
        ),
        (
            "DescribeAffectedAccountsForOrganization",
            json!({ "eventArn": ABSENT_EVENT_ARN }),
            false,
        ),
        (
            "DescribeAffectedEntities",
            json!({ "filter": { "eventArns": [ABSENT_EVENT_ARN] } }),
            true,
        ),
        (
            "DescribeAffectedEntitiesForOrganization",
            json!({ "organizationEntityAccountFilters": [{ "eventArn": ABSENT_EVENT_ARN }] }),
            false,
        ),
        (
            "DescribeEntityAggregates",
            json!({ "eventArns": [ABSENT_EVENT_ARN] }),
            true,
        ),
        (
            "DescribeEntityAggregatesForOrganization",
            json!({ "eventArns": [ABSENT_EVENT_ARN] }),
            false,
        ),
        (
            "DescribeEventAggregates",
            json!({ "aggregateField": "eventTypeCategory" }),
            true,
        ),
        (
            "DescribeEventDetails",
            json!({ "eventArns": [ABSENT_EVENT_ARN] }),
            true,
        ),
        (
            "DescribeEventDetailsForOrganization",
            json!({ "organizationEventDetailFilters": [{ "eventArn": ABSENT_EVENT_ARN }] }),
            false,
        ),
        ("DescribeEventTypes", json!({}), true),
        ("DescribeEvents", json!({}), true),
        ("DescribeEventsForOrganization", json!({}), false),
        ("DescribeServiceLifecycle", json!({}), false),
        ("DescribeTags", json!({}), true),
        ("DescribeTagsForOrganization", json!({}), true),
    ]
}

/// Assert one call was refused by the gate.
fn assert_refused(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, SUBSCRIPTION_REQUIRED_STATUS,
        "case {case:?}: expected HTTP {SUBSCRIPTION_REQUIRED_STATUS}, got {status} {body}"
    );
    let code = error_code(body);
    assert_eq!(
        code, SUBSCRIPTION_REQUIRED,
        "case {case:?}: expected error code {SUBSCRIPTION_REQUIRED}, got {code:?} in {body}"
    );
    assert!(
        body["message"].as_str().is_some_and(|m| !m.is_empty()),
        "case {case:?}: expected the refusal to carry a message, got {body}"
    );
}

/// One emulator over its own engine, with the account's Support plan recorded in this
/// service's store the way an operator records it with `SUPPORT_PLAN`.
fn health_with_plan(
    plan: Option<&str>,
) -> (
    smithy_server_runtime::MultiServiceRouter,
    Arc<engine::InMemoryEngine>,
) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    set_support_plan(&engine, plan);
    (health_router_over(Arc::clone(&engine)), engine)
}

/// RULE: with no qualifying plan, EVERY bound operation answers the
/// subscription-required refusal — the implemented readers and the stubs alike.
#[tokio::test]
async fn no_qualifying_plan_refuses_every_bound_operation() {
    let (router, _engine) = health_with_plan(None);
    for (operation, input, _implemented) in every_bound_operation() {
        let (status, body) = call(&router, operation, input).await;
        assert_refused(operation, status, &body);
    }
}

/// RULE: the gate decides per ACCOUNT, not per operation — so with a qualifying plan
/// every one of those same calls reaches its handler, which answers its own way (a
/// 200 for an implemented operation, the scaffold's 501 for one that is not written
/// yet) and never the subscription refusal.
#[tokio::test]
async fn a_qualifying_plan_reaches_every_bound_operation() {
    let (router, _engine) = health_with_plan(Some(SUBSCRIBED_PLAN));
    for (operation, input, implemented) in every_bound_operation() {
        let (status, body) = call(&router, operation, input).await;
        if implemented {
            assert_eq!(
                status, 200,
                "case {operation:?}: a qualifying plan must reach the handler, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, NOT_IMPLEMENTED_STATUS,
                "case {operation:?}: a scaffolded operation answers \
                 {NOT_IMPLEMENTED_STATUS}, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                NOT_IMPLEMENTED,
                "case {operation:?}: expected {NOT_IMPLEMENTED}, got {body}"
            );
        }
    }
}

/// RULE: the gate precedes input validation and the catalog lookup, so the refusal
/// does not depend on the input being well formed or on the event named existing.
/// Rows differ along the shape of the input: complete, empty (so a required member is
/// absent), a member outside its modelled bound, a locale this service does not serve,
/// a pagination token nobody issued, and an event ARN the catalog does not hold.
#[tokio::test]
async fn the_refusal_precedes_input_validation_and_the_catalog_lookup() {
    let (router, _engine) = health_with_plan(None);
    let rows = [
        ("complete input", "DescribeEvents", json!({})),
        (
            "the required filter absent",
            "DescribeAffectedEntities",
            json!({}),
        ),
        (
            "the required aggregateField absent",
            "DescribeEventAggregates",
            json!({}),
        ),
        (
            "aggregateField outside the modelled enum",
            "DescribeEventAggregates",
            json!({ "aggregateField": "notAField" }),
        ),
        (
            "maxResults below the modelled range of 10..100",
            "DescribeEventTypes",
            json!({ "maxResults": 1 }),
        ),
        (
            "eventArns longer than the modelled maximum of 10",
            "DescribeEventDetails",
            json!({ "eventArns": vec![ABSENT_EVENT_ARN; 11] }),
        ),
        (
            "a locale this service does not serve",
            "DescribeEvents",
            json!({ "locale": "fr" }),
        ),
        (
            "a nextToken this service never issued",
            "DescribeEvents",
            json!({ "nextToken": "zzzz" }),
        ),
        (
            "an eventArn the catalog does not hold",
            "DescribeEventDetails",
            json!({ "eventArns": [ABSENT_EVENT_ARN] }),
        ),
        (
            "an eventArn that is not even of the documented shape",
            "DescribeEntityAggregates",
            json!({ "eventArns": ["not-an-arn"] }),
        ),
    ];
    for (case, operation, input) in rows {
        let (status, body) = call(&router, operation, input).await;
        assert_refused(&format!("{operation}: {case}"), status, &body);
    }
}

/// RULE: which plan the account holds decides whether the API is served at all.
///
/// The qualifying plans are the ones this service's own documentation names (see
/// `state::QUALIFYING_SUPPORT_PLANS`, which cites the sentence): Business, Enterprise
/// On-Ramp and Enterprise Support, plus the current names of those same premium tiers
/// (Business Support+, Unified Operations) that the Support API's documentation spells
/// out and that list cites. Rows vary the plan, its spelling (case, separators, the
/// "Amazon Web Services" / "AWS" prefix the documentation writes) and whether it
/// qualifies; Basic and Developer are the plans AWS publishes that do not.
///
/// Each row is checked on an implemented reader AND on a scaffolded operation, so what
/// a row proves is that the GATE let the call through, not that one handler happens to
/// answer.
#[tokio::test]
async fn only_a_qualifying_plan_reaches_the_operations() {
    let rows: [(Option<&str>, bool); 15] = [
        (Some("business"), true),
        (Some("Business"), true),
        (Some("Business Support"), true),
        (Some("AWS Business Support"), true),
        (Some("Business Support+"), true),
        (Some("enterprise"), true),
        (Some("Amazon Web Services Enterprise Support"), true),
        (Some("Enterprise On-Ramp"), true),
        (Some("enterprise_on_ramp"), true),
        (Some("Unified Operations"), true),
        (Some("basic"), false),
        (Some("Developer"), false),
        (Some("Business Partner"), false),
        (Some(""), false),
        (None, false),
    ];
    for (plan, qualifies) in rows {
        let (router, _engine) = health_with_plan(plan);
        let (status, body) = call(&router, "DescribeEventTypes", json!({})).await;
        let (stub_status, stub_body) = call(&router, "DescribeServiceLifecycle", json!({})).await;
        if qualifies {
            assert_eq!(
                status, 200,
                "plan {plan:?}: a qualifying plan must reach DescribeEventTypes, \
                 got {status} {body}"
            );
            assert!(
                body["eventTypes"].as_array().is_some_and(|t| !t.is_empty()),
                "plan {plan:?}: expected the stored event types, got {body}"
            );
            assert_eq!(
                stub_status, NOT_IMPLEMENTED_STATUS,
                "plan {plan:?}: a qualifying plan reaches the scaffold's own answer, \
                 got {stub_status} {stub_body}"
            );
        } else {
            assert_refused(&format!("plan {plan:?} on DescribeEventTypes"), status, &body);
            assert_refused(
                &format!("plan {plan:?} on DescribeServiceLifecycle"),
                stub_status,
                &stub_body,
            );
        }
    }
}

/// RULE: the gate fronts the operations this service BINDS. A request naming none is
/// still answered by the routing path — the refusal is about the account's plan, not a
/// catch-all for every request the service cannot serve.
#[tokio::test]
async fn a_request_naming_no_bound_operation_is_not_a_subscription_refusal() {
    for plan in [None, Some(SUBSCRIBED_PLAN)] {
        let (router, _engine) = health_with_plan(plan);
        for operation in ["NoSuchOperation", "DescribeHealthServiceStatusForOrganization"] {
            let (status, body) = call(&router, operation, json!({})).await;
            assert_ne!(
                error_code(&body),
                SUBSCRIPTION_REQUIRED,
                "plan {plan:?}, {operation}: an unbound operation is a routing failure, \
                 got {status} {body}"
            );
        }
    }
}

/// RULE: a refused call runs nothing — so the catalog a later caller reads is exactly
/// the one that was there before, and the account regaining a qualifying plan serves
/// the same stored events. Proved by reading the store back through the whole
/// implemented read loop on the SAME emulator, in both directions.
#[tokio::test]
async fn a_refused_call_changes_nothing_a_later_caller_reads() {
    let (router, engine) = health_with_plan(Some(SUBSCRIBED_PLAN));

    // What the subscribed account reads, through every implemented reader.
    let before = read_loop(&router).await;

    // Lose the plan: every one of those reads is refused.
    set_support_plan(&engine, Some("basic"));
    for (operation, input, _implemented) in every_bound_operation() {
        let (status, body) = call(&router, operation, input).await;
        assert_refused(&format!("{operation} under a non-qualifying plan"), status, &body);
    }

    // Get it back: the same stored events, entities and aggregates, unchanged.
    set_support_plan(&engine, Some(SUBSCRIBED_PLAN));
    let after = read_loop(&router).await;
    assert_eq!(
        after, before,
        "a refused call must change nothing: expected {before}, got {after}"
    );
}

/// Every implemented reader's answer, as one value to compare.
async fn read_loop(router: &smithy_server_runtime::MultiServiceRouter) -> Value {
    let events = call_ok(router, "DescribeEvents", json!({})).await;
    let arns: Vec<String> = events["events"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|event| event["arn"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !arns.is_empty(),
        "the subscribed account must read a non-empty catalog, got {events}"
    );
    json!({
        "events": events,
        "eventTypes": call_ok(router, "DescribeEventTypes", json!({})).await,
        "eventAggregates": call_ok(
            router,
            "DescribeEventAggregates",
            json!({ "aggregateField": "eventTypeCategory" }),
        )
        .await,
        "entityAggregates": call_ok(
            router,
            "DescribeEntityAggregates",
            json!({ "eventArns": arns.clone() }),
        )
        .await,
        "eventDetails": call_ok(
            router,
            "DescribeEventDetails",
            json!({ "eventArns": arns.iter().take(10).collect::<Vec<_>>() }),
        )
        .await,
        "entities": call_ok(
            router,
            "DescribeAffectedEntities",
            json!({ "filter": { "eventArns": arns.iter().take(10).collect::<Vec<_>>() } }),
        )
        .await,
    })
}
