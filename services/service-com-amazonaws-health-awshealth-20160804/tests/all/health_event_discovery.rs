// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the AWS Health event-discovery read loop.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over an
//! awsJson1_1 `POST /` with `X-Amz-Target: AWSHealth_20160804.<Op>` — so what is
//! asserted is what a client sees.
//!
//! The suite asserts RULES, not the seeded catalog's literal contents: the expected
//! answer for each case is derived at runtime from an unfiltered `DescribeEvents`
//! and the same predicate the service is supposed to apply. That way the tests keep
//! holding if the catalog grows, and they fail if a handler ever answers from
//! anything other than the stored events.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The service's own `eventStatusCode` / `entityStatusCode` / `eventTypeCategory`
/// value sets, as this crate's generated types expose them (the model's enums).
use service_com_amazonaws_health_awshealth_20160804::ops::types::{
    ACCOUNT_TAG_TYPE_VALUES, ENTITY_STATUS_CODE_VALUES, EVENT_AGGREGATE_FIELD_VALUES,
    EVENT_STATUS_CODE_VALUES, EVENT_TYPE_CATEGORY_VALUES, ORGANIZATION_TAG_TYPE_VALUES,
    TAG_KEY_STATUS_VALUES,
};

/// A router serving only this service, over one shared engine.
///
/// The account holds [`SUBSCRIBED_PLAN`], because the Health API is served only to an
/// account whose Amazon Web Services Support plan qualifies (see
/// `subscription_gate.rs` for that rule) and these suites exercise the operations
/// behind that gate.
fn health_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    set_support_plan(&engine, Some(SUBSCRIBED_PLAN));
    health_router_over(engine)
}

/// An Amazon Web Services Support plan that may use this API. Recorded in the store
/// rather than read from the environment, so one suite's account cannot change
/// another's.
pub(crate) const SUBSCRIBED_PLAN: &str = "enterprise";

/// Record `plan` as the account's Support plan on `engine` — `None` for an account
/// with no Support plan at all. Takes effect on a running emulator, leaving every
/// stored event where it is.
pub(crate) fn set_support_plan(engine: &engine::InMemoryEngine, plan: Option<&str>) {
    service_com_amazonaws_health_awshealth_20160804::state::service_state(engine)
        .expect("the health store is reachable from the shared in-memory engine")
        .lock()
        .support_plan = plan.map(str::to_string);
}

/// A router serving only this service over `engine`, built the way the production
/// router builds it.
pub(crate) fn health_router_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Pin the harness independent of the process-wide switch: these tests exercise
    // the operations, not SigV4 verification.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_health_awshealth_20160804::EmulatedService::new(engine),
    );
    router
}

/// One awsJson1_1 Health request. The SigV4 credential scope names `health`, which
/// is the routing key the framework resolves a request by.
fn health_request(operation: &str, body: &Value) -> http::Request<bytes::Bytes> {
    http::Request::builder()
        .method("POST")
        .uri("https://health.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSHealth_20160804.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEMULATORTEST/20260101/us-east-1/health/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request")
}

/// Call one operation and return its HTTP status and parsed body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let response = router.dispatch(health_request(operation, &body)).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered {status} with a body that is not JSON: {error}: {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Call one operation, requiring a 200.
pub(crate) async fn call_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status}: {parsed}"
    );
    parsed
}

/// The wire error code of an error response (`__type`, possibly namespace-qualified).
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(value: &Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

fn list(value: &Value, member: &str) -> Vec<String> {
    value[member]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn arns(events: &[Value]) -> Vec<String> {
    events.iter().map(|event| text(event, "arn")).collect()
}

/// Every event in the catalog, as an unfiltered `DescribeEvents` reports it.
async fn whole_catalog(router: &MultiServiceRouter) -> Vec<Value> {
    let body = call_ok(router, "DescribeEvents", json!({})).await;
    let events = body["events"]
        .as_array()
        .expect("DescribeEvents returns an events list")
        .clone();
    assert!(
        !events.is_empty(),
        "the service must serve a non-empty stored event catalog, got {body}"
    );
    assert!(
        body.get("nextToken").and_then(Value::as_str).is_none(),
        "an unfiltered, unpaginated DescribeEvents is one page, got nextToken in {body}"
    );
    events
}

/// The rule the `DescribeEvents` doc states: "Results are sorted by
/// lastModifiedTime, starting with the most recent event."
fn assert_newest_first(case: &str, events: &[Value]) {
    let times: Vec<i64> = events
        .iter()
        .map(|event| event["lastUpdatedTime"].as_f64().unwrap_or_default() as i64)
        .collect();
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(
        times, sorted,
        "case {case}: events must be newest first, got {times:?}"
    );
}

// ---------------------------------------------------------------------------
// DescribeEvents: filtering
// ---------------------------------------------------------------------------

/// One row of the filter table: the filter sent, and the predicate the service is
/// supposed to apply, evaluated client-side over the whole catalog.
struct FilterCase {
    name: &'static str,
    filter: fn() -> Value,
    admits: fn(&Value) -> bool,
}

/// The table spans the dimensions the rule generalises over: which member is
/// constrained, how many values it lists, whether the member is optional on the
/// event, a conjunction of two members, a value no event carries, and an empty list.
const FILTER_CASES: &[FilterCase] = &[
    FilterCase {
        name: "no filter at all",
        filter: || json!({}),
        admits: |_| true,
    },
    FilterCase {
        name: "services, one value",
        filter: || json!({"services": ["EC2"]}),
        admits: |event| text(event, "service") == "EC2",
    },
    FilterCase {
        name: "services, two values",
        filter: || json!({"services": ["EC2", "RDS"]}),
        admits: |event| ["EC2", "RDS"].contains(&text(event, "service").as_str()),
    },
    FilterCase {
        name: "services, a value no event carries",
        filter: || json!({"services": ["LAMBDA"]}),
        admits: |_| false,
    },
    FilterCase {
        name: "regions",
        filter: || json!({"regions": ["us-west-2"]}),
        admits: |event| text(event, "region") == "us-west-2",
    },
    FilterCase {
        name: "eventTypeCategories, one value",
        filter: || json!({"eventTypeCategories": ["issue"]}),
        admits: |event| text(event, "eventTypeCategory") == "issue",
    },
    FilterCase {
        name: "eventTypeCategories, two values",
        filter: || json!({"eventTypeCategories": ["accountNotification", "scheduledChange"]}),
        admits: |event| {
            ["accountNotification", "scheduledChange"]
                .contains(&text(event, "eventTypeCategory").as_str())
        },
    },
    FilterCase {
        name: "eventStatusCodes",
        filter: || json!({"eventStatusCodes": ["open"]}),
        admits: |event| text(event, "statusCode") == "open",
    },
    FilterCase {
        name: "eventTypeCodes",
        filter: || json!({"eventTypeCodes": ["AWS_RDS_OPERATIONAL_ISSUE"]}),
        admits: |event| text(event, "eventTypeCode") == "AWS_RDS_OPERATIONAL_ISSUE",
    },
    FilterCase {
        name: "availabilityZones, a member many events omit",
        filter: || json!({"availabilityZones": ["us-east-1a"]}),
        admits: |event| text(event, "availabilityZone") == "us-east-1a",
    },
    FilterCase {
        name: "actionabilities",
        filter: || json!({"actionabilities": ["ACTION_REQUIRED"]}),
        admits: |event| text(event, "actionability") == "ACTION_REQUIRED",
    },
    FilterCase {
        name: "personas, matched against a list member",
        filter: || json!({"personas": ["BILLING"]}),
        admits: |event| list(event, "personas").iter().any(|p| p == "BILLING"),
    },
    FilterCase {
        name: "two members conjoined",
        filter: || json!({"services": ["EC2"], "eventTypeCategories": ["issue"]}),
        admits: |event| {
            text(event, "service") == "EC2" && text(event, "eventTypeCategory") == "issue"
        },
    },
    FilterCase {
        name: "two members whose conjunction is empty",
        filter: || json!({"services": ["RDS"], "eventTypeCategories": ["accountNotification"]}),
        admits: |_| false,
    },
];

#[tokio::test]
async fn describe_events_returns_exactly_the_stored_events_the_filter_admits() {
    let router = health_router();
    let catalog = whole_catalog(&router).await;

    for case in FILTER_CASES {
        let body = call_ok(&router, "DescribeEvents", json!({"filter": (case.filter)()})).await;
        let events = body["events"].as_array().cloned().unwrap_or_default();

        let mut expected: Vec<String> = catalog
            .iter()
            .filter(|event| (case.admits)(event))
            .map(|event| text(event, "arn"))
            .collect();
        expected.sort();
        let mut actual = arns(&events);
        actual.sort();

        assert_eq!(
            actual, expected,
            "case {}: DescribeEvents must return exactly the admitted stored events, \
             got {actual:?}",
            case.name
        );
        assert_newest_first(case.name, &events);
        // Every returned event is the stored record, not a value derived from the
        // request: it must be byte-identical to the catalog entry with that ARN.
        for event in &events {
            let stored = catalog
                .iter()
                .find(|candidate| text(candidate, "arn") == text(event, "arn"))
                .unwrap_or_else(|| panic!("case {}: {event} is not in the catalog", case.name));
            assert_eq!(
                event, stored,
                "case {}: the filtered summary must equal the stored summary, got {event}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn describe_events_filters_on_values_taken_from_the_catalog_itself() {
    let router = health_router();
    let catalog = whole_catalog(&router).await;

    // By ARN: every single-ARN request resolves to exactly that one event.
    for event in &catalog {
        let arn = text(event, "arn");
        let body = call_ok(
            &router,
            "DescribeEvents",
            json!({"filter": {"eventArns": [arn.clone()]}}),
        )
        .await;
        let events = body["events"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            arns(&events),
            vec![arn.clone()],
            "case eventArns=[{arn}]: exactly that event must come back, got {events:?}"
        );
    }

    // All ARNs at once: the whole catalog, still newest first.
    let every_arn: Vec<String> = arns(&catalog);
    let body = call_ok(
        &router,
        "DescribeEvents",
        json!({"filter": {"eventArns": every_arn.clone()}}),
    )
    .await;
    let events = body["events"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        arns(&events),
        every_arn,
        "case eventArns=<all>: every event must come back in catalog order"
    );

    // By lastUpdatedTimes range, on each published boundary of the catalog's own
    // timestamps and on one instant past the newest.
    let mut times: Vec<i64> = catalog
        .iter()
        .map(|event| event["lastUpdatedTime"].as_f64().unwrap_or_default() as i64)
        .collect();
    times.sort_unstable();
    let newest = *times.last().expect("a non-empty catalog");
    let mut bounds = times.clone();
    bounds.push(newest + 1);
    for from in bounds {
        let body = call_ok(
            &router,
            "DescribeEvents",
            json!({"filter": {"lastUpdatedTimes": [{"from": from}]}}),
        )
        .await;
        let events = body["events"].as_array().cloned().unwrap_or_default();
        let expected = catalog
            .iter()
            .filter(|event| event["lastUpdatedTime"].as_f64().unwrap_or_default() as i64 >= from)
            .count();
        assert_eq!(
            events.len(),
            expected,
            "case lastUpdatedTimes.from={from}: {expected} events are at or after it, \
             got {}",
            events.len()
        );
    }

    // By an entity the event affects: EventFilter's entity clauses must read the same
    // stored entity list DescribeAffectedEntities serves.
    for event in &catalog {
        let arn = text(event, "arn");
        let entities = call_ok(
            &router,
            "DescribeAffectedEntities",
            json!({"filter": {"eventArns": [arn.clone()]}}),
        )
        .await;
        for entity in entities["entities"].as_array().cloned().unwrap_or_default() {
            let value = text(&entity, "entityValue");
            let body = call_ok(
                &router,
                "DescribeEvents",
                json!({"filter": {"entityValues": [value.clone()]}}),
            )
            .await;
            let matched = arns(&body["events"].as_array().cloned().unwrap_or_default());
            assert!(
                matched.contains(&arn),
                "case entityValues=[{value}]: the event affecting it must be admitted, \
                 got {matched:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// The modelled `nextToken` pattern, `^[a-zA-Z0-9=/+_.-]{4,10000}$`.
fn is_modelled_token(token: &str) -> bool {
    (4..=10_000).contains(&token.len())
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "=/+_.-".contains(c))
}

#[tokio::test]
async fn paginating_walks_every_match_exactly_once_whatever_the_page_size() {
    let router = health_router();
    let catalog = whole_catalog(&router).await;

    // `maxResults` spans the modelled range's lower bound, the sizes that split the
    // catalog, and a size larger than it.
    let page_sizes: Vec<usize> = (1..=catalog.len() + 1).collect();
    let filters = [
        ("no filter", json!({})),
        ("eventTypeCategories=[issue]", json!({"eventTypeCategories": ["issue"]})),
        ("services=[EC2]", json!({"services": ["EC2"]})),
    ];

    for (filter_name, filter) in &filters {
        let unpaginated = call_ok(&router, "DescribeEvents", json!({"filter": filter})).await;
        let expected = arns(&unpaginated["events"].as_array().cloned().unwrap_or_default());

        for page_size in &page_sizes {
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let mut request = json!({"filter": filter, "maxResults": page_size});
                if let Some(token) = &token {
                    request["nextToken"] = json!(token);
                }
                let body = call_ok(&router, "DescribeEvents", request).await;
                let events = body["events"].as_array().cloned().unwrap_or_default();
                let case = format!("filter {filter_name}, maxResults {page_size}, page {pages}");
                assert!(
                    events.len() <= *page_size,
                    "case {case}: a page must not exceed maxResults, got {}",
                    events.len()
                );
                walked.extend(arns(&events));
                pages += 1;
                assert!(
                    pages <= expected.len() + 2,
                    "case {case}: the walk does not terminate, walked {walked:?}"
                );
                match body.get("nextToken").and_then(Value::as_str) {
                    Some(next) => {
                        assert!(
                            is_modelled_token(next),
                            "case {case}: nextToken must match the modelled pattern, got {next}"
                        );
                        assert_eq!(
                            events.len(),
                            *page_size,
                            "case {case}: a non-final page must be full, got {}",
                            events.len()
                        );
                        token = Some(next.to_string());
                    }
                    None => break,
                }
            }
            assert_eq!(
                walked, expected,
                "case filter {filter_name}, maxResults {page_size}: the walk must visit \
                 every match once, in order"
            );
        }
    }
}

#[tokio::test]
async fn a_pagination_token_this_request_did_not_issue_is_refused() {
    let router = health_router();
    let catalog = whole_catalog(&router).await;
    assert!(
        catalog.len() >= 2,
        "this case needs a catalog that paginates, got {}",
        catalog.len()
    );

    // A token this operation really issued, for the unfiltered walk.
    let first_page = call_ok(&router, "DescribeEvents", json!({"maxResults": 1})).await;
    let issued = first_page["nextToken"]
        .as_str()
        .expect("a truncated DescribeEvents issues a nextToken")
        .to_string();

    // Positive control: replayed against the request that issued it, it continues.
    let second_page = call_ok(
        &router,
        "DescribeEvents",
        json!({"maxResults": 1, "nextToken": issued.clone()}),
    )
    .await;
    assert_eq!(
        arns(&second_page["events"].as_array().cloned().unwrap_or_default()),
        vec![text(&catalog[1], "arn")],
        "the issued token must continue the same walk"
    );

    // A token issued for a different result set, and one issued by another operation.
    let other_filter = call_ok(
        &router,
        "DescribeEvents",
        json!({"filter": {"eventTypeCategories": ["issue"]}, "maxResults": 1}),
    )
    .await;
    let other_filter_token = other_filter["nextToken"]
        .as_str()
        .expect("a truncated filtered DescribeEvents issues a nextToken")
        .to_string();
    let entity_page = call_ok(
        &router,
        "DescribeAffectedEntities",
        json!({
            "filter": {"eventArns": arns(&catalog)},
            "maxResults": 1
        }),
    )
    .await;
    let other_operation_token = entity_page["nextToken"]
        .as_str()
        .expect("a truncated DescribeAffectedEntities issues a nextToken")
        .to_string();

    // A token of the right shape whose authenticator does not re-derive.
    let mut corrupted: Vec<char> = issued.chars().collect();
    let last = corrupted.len() - 1;
    corrupted[last] = if corrupted[last] == '0' { 'f' } else { '0' };
    let corrupted: String = corrupted.into_iter().collect();

    let cases: Vec<(&str, Value)> = vec![
        ("a token the server never issued", json!({"nextToken": "bogustoken"})),
        (
            "a token issued for a different filter",
            json!({"maxResults": 1, "nextToken": other_filter_token}),
        ),
        (
            "a token issued by another operation",
            json!({"maxResults": 1, "nextToken": other_operation_token}),
        ),
        (
            "a token whose authenticator was tampered with",
            json!({"maxResults": 1, "nextToken": corrupted}),
        ),
        (
            "a token-shaped value with no separator",
            json!({"nextToken": "aaaabbbbcccc"}),
        ),
    ];

    for (name, request) in cases {
        let (status, body) = call(&router, "DescribeEvents", request).await;
        assert_eq!(
            status, 400,
            "case {name}: an unissued pagination token is a 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {name}: the error code must be InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Locale
// ---------------------------------------------------------------------------

#[tokio::test]
async fn only_the_documented_locale_is_served() {
    let router = health_router();
    let catalog = whole_catalog(&router).await;
    let one_arn = text(&catalog[0], "arn");

    // Each operation whose model declares UnsupportedLocale, with a request that is
    // otherwise valid.
    let operations: Vec<(&str, Value)> = vec![
        ("DescribeEvents", json!({})),
        ("DescribeEventTypes", json!({})),
        ("DescribeEventDetails", json!({"eventArns": [one_arn.clone()]})),
        (
            "DescribeAffectedEntities",
            json!({"filter": {"eventArns": [one_arn.clone()]}}),
        ),
    ];
    // "English (en) is the default and the only supported value at this time."
    let accepted = ["en", "EN", "eN"];
    let refused = ["fr", "de", "es", "en-US", "en_GB", "zz"];

    for (operation, base) in &operations {
        for locale in accepted {
            let mut request = base.clone();
            request["locale"] = json!(locale);
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, 200,
                "case {operation} locale={locale}: the supported locale must be served, \
                 got {status}: {body}"
            );
        }
        for locale in refused {
            let mut request = base.clone();
            request["locale"] = json!(locale);
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, 400,
                "case {operation} locale={locale}: an unsupported locale is a 400, \
                 got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "UnsupportedLocale",
                "case {operation} locale={locale}: the error code must be \
                 UnsupportedLocale, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// DescribeEventDetails
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_event_details_resolves_known_arns_and_reports_unknown_ones_per_item() {
    let router = health_router();
    let catalog = whole_catalog(&router).await;
    let known: Vec<String> = arns(&catalog);
    let absent = "arn:aws:health:us-east-1::event/EC2/AWS_EC2_NO_SUCH_EVENT/AWS_EC2_NO_SUCH_EVENT_0";

    // Rows differ along count, ordering, the known/unknown mix, and repetition.
    let cases: Vec<(&str, Vec<String>, usize, usize)> = vec![
        ("one known ARN", vec![known[0].clone()], 1, 0),
        (
            "every known ARN",
            known.clone(),
            known.len(),
            0,
        ),
        (
            "every known ARN, reversed",
            known.iter().rev().cloned().collect(),
            known.len(),
            0,
        ),
        ("one unknown ARN", vec![absent.to_string()], 0, 1),
        (
            "a known and an unknown ARN",
            vec![known[0].clone(), absent.to_string()],
            1,
            1,
        ),
        (
            "the same known ARN twice",
            vec![known[0].clone(), known[0].clone()],
            1,
            0,
        ),
    ];

    for (name, event_arns, successes, failures) in cases {
        let body = call_ok(
            &router,
            "DescribeEventDetails",
            json!({"eventArns": event_arns}),
        )
        .await;
        let successful = body["successfulSet"].as_array().cloned().unwrap_or_default();
        let failed = body["failedSet"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            successful.len(),
            successes,
            "case {name}: expected {successes} resolved events, got {successful:?}"
        );
        assert_eq!(
            failed.len(),
            failures,
            "case {name}: expected {failures} failures, got {failed:?}"
        );

        for details in &successful {
            let event = &details["event"];
            let stored = catalog
                .iter()
                .find(|candidate| text(candidate, "arn") == text(event, "arn"))
                .unwrap_or_else(|| panic!("case {name}: {event} is not in the catalog"));
            assert_eq!(
                event, stored,
                "case {name}: the detailed event must be the SAME summary DescribeEvents \
                 returned, got {event}"
            );
            let description = text(&details["eventDescription"], "latestDescription");
            assert!(
                !description.is_empty(),
                "case {name}: {} must carry a latestDescription, got {details}",
                text(event, "arn")
            );
        }
        for failure in &failed {
            assert_eq!(
                text(failure, "eventArn"),
                absent,
                "case {name}: only the absent ARN may fail, got {failure}"
            );
            assert_eq!(
                text(failure, "errorName"),
                "EventNotFound",
                "case {name}: an absent event's errorName must be EventNotFound, got {failure}"
            );
            assert!(
                !text(failure, "errorMessage").is_empty(),
                "case {name}: a failure must say why, got {failure}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// DescribeAffectedEntities and DescribeEntityAggregates
// ---------------------------------------------------------------------------

/// Every entity of one event, walked through pagination at `page_size`.
async fn walk_entities(
    router: &MultiServiceRouter,
    filter: &Value,
    page_size: usize,
) -> Vec<Value> {
    let mut walked = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({"filter": filter, "maxResults": page_size});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let body = call_ok(router, "DescribeAffectedEntities", request).await;
        walked.extend(body["entities"].as_array().cloned().unwrap_or_default());
        match body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => return walked,
        }
        assert!(
            walked.len() < 1_000,
            "the entity walk for {filter} does not terminate"
        );
    }
}

#[tokio::test]
async fn entity_aggregates_agree_with_the_entities_describe_affected_entities_serves() {
    let router = health_router();
    let catalog = whole_catalog(&router).await;

    for event in &catalog {
        let arn = text(event, "arn");
        let filter = json!({"eventArns": [arn.clone()]});
        // The same records, reached one page at a time and all at once.
        let paged = walk_entities(&router, &filter, 1).await;
        let whole = walk_entities(&router, &filter, 100).await;
        assert_eq!(
            paged, whole,
            "case {arn}: paging the entities must not change them"
        );
        for entity in &whole {
            assert_eq!(
                text(entity, "eventArn"),
                arn,
                "case {arn}: an entity must belong to the event it was asked for, got {entity}"
            );
            assert!(
                !text(entity, "entityArn").is_empty()
                    && !text(entity, "entityValue").is_empty()
                    && !text(entity, "awsAccountId").is_empty(),
                "case {arn}: a stored entity carries arn/value/account, got {entity}"
            );
            assert!(
                ENTITY_STATUS_CODE_VALUES.contains(&text(entity, "statusCode").as_str()),
                "case {arn}: statusCode must be a modelled entityStatusCode, got {entity}"
            );
        }

        let mut expected_statuses: BTreeMap<String, i64> = BTreeMap::new();
        for entity in &whole {
            *expected_statuses
                .entry(text(entity, "statusCode"))
                .or_insert(0) += 1;
        }

        let aggregates = call_ok(
            &router,
            "DescribeEntityAggregates",
            json!({"eventArns": [arn.clone()]}),
        )
        .await;
        let rows = aggregates["entityAggregates"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            rows.len(),
            1,
            "case {arn}: one aggregate row per known event, got {rows:?}"
        );
        assert_eq!(
            rows[0]["count"].as_i64().unwrap_or(-1),
            whole.len() as i64,
            "case {arn}: count must equal the {} entities served, got {}",
            whole.len(),
            rows[0]["count"]
        );
        let actual_statuses: BTreeMap<String, i64> = rows[0]["statuses"]
            .as_object()
            .map(|map| {
                map.iter()
                    .map(|(k, v)| (k.clone(), v.as_i64().unwrap_or(-1)))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            actual_statuses, expected_statuses,
            "case {arn}: statuses must tally the entities served"
        );
    }

    // All events at once: one row per known ARN, and none for an ARN the catalog
    // does not hold.
    let absent = "arn:aws:health:us-east-1::event/EC2/AWS_EC2_NO_SUCH_EVENT/AWS_EC2_NO_SUCH_EVENT_0";
    let mut requested = arns(&catalog);
    requested.push(absent.to_string());
    let aggregates = call_ok(
        &router,
        "DescribeEntityAggregates",
        json!({"eventArns": requested}),
    )
    .await;
    let rows = aggregates["entityAggregates"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut returned: Vec<String> = rows.iter().map(|row| text(row, "eventArn")).collect();
    returned.sort();
    let mut expected = arns(&catalog);
    expected.sort();
    assert_eq!(
        returned, expected,
        "case every ARN plus an absent one: only known events aggregate, got {returned:?}"
    );
}

#[tokio::test]
async fn the_entity_filter_narrows_the_entities_served() {
    let router = health_router();
    let catalog = whole_catalog(&router).await;
    let every_arn = arns(&catalog);
    let all = walk_entities(&router, &json!({"eventArns": every_arn.clone()}), 100).await;
    assert!(
        !all.is_empty(),
        "the catalog must store affected entities for its events"
    );

    // statusCodes: every modelled entityStatusCode, including ones no stored entity
    // carries, plus the two-value and empty-list forms.
    for status in ENTITY_STATUS_CODE_VALUES {
        let got = walk_entities(
            &router,
            &json!({"eventArns": every_arn.clone(), "statusCodes": [status]}),
            100,
        )
        .await;
        let expected: Vec<&Value> = all
            .iter()
            .filter(|entity| text(entity, "statusCode") == *status)
            .collect();
        assert_eq!(
            got.len(),
            expected.len(),
            "case statusCodes=[{status}]: {} entities carry it, got {}",
            expected.len(),
            got.len()
        );
        for entity in &got {
            assert_eq!(
                text(entity, "statusCode"),
                *status,
                "case statusCodes=[{status}]: got {entity}"
            );
        }
    }

    let pair: Vec<&str> = ENTITY_STATUS_CODE_VALUES.iter().take(2).copied().collect();
    let got = walk_entities(
        &router,
        &json!({"eventArns": every_arn.clone(), "statusCodes": pair.clone()}),
        100,
    )
    .await;
    let expected = all
        .iter()
        .filter(|entity| pair.contains(&text(entity, "statusCode").as_str()))
        .count();
    assert_eq!(
        got.len(),
        expected,
        "case statusCodes={pair:?}: expected {expected}, got {}",
        got.len()
    );

    // The model gives every filter list a minimum length of 1, so an empty list is a
    // malformed request, not "no constraint": the boundary validator refuses it.
    let (status, body) = call(
        &router,
        "DescribeAffectedEntities",
        json!({"filter": {"eventArns": every_arn.clone(), "statusCodes": []}}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case statusCodes=[]: an empty list violates the modelled minimum length, \
         got {status}: {body}"
    );

    // entityValues and entityArns: each stored entity is reachable by its own value
    // and by its own ARN, and by nothing else.
    for entity in &all {
        for (member, stored_member) in [
            ("entityValues", "entityValue"),
            ("entityArns", "entityArn"),
        ] {
            let value = text(entity, stored_member);
            let mut filter = json!({"eventArns": every_arn.clone()});
            filter[member] = json!([value.clone()]);
            let got = walk_entities(&router, &filter, 100).await;
            let expected = all
                .iter()
                .filter(|candidate| text(candidate, stored_member) == value)
                .count();
            assert_eq!(
                got.len(),
                expected,
                "case {member}=[{value}]: expected {expected} entities, got {}",
                got.len()
            );
        }
    }

    // An event ARN the catalog does not hold contributes no entity.
    let got = walk_entities(
        &router,
        &json!({"eventArns": ["arn:aws:health:us-east-1::event/EC2/AWS_EC2_NO_SUCH/AWS_EC2_NO_SUCH_0"]}),
        100,
    )
    .await;
    assert!(
        got.is_empty(),
        "case an absent event ARN: no entities, got {got:?}"
    );
}

#[tokio::test]
async fn the_entity_walk_does_not_depend_on_the_order_the_request_lists_its_events() {
    let router = health_router();
    let catalog = whole_catalog(&router).await;
    let forward = arns(&catalog);
    let reversed: Vec<String> = forward.iter().rev().cloned().collect();

    // The page token's request digest hashes list members order-independently, so the
    // result it indexes into must be ordered the same way for both spellings — else a
    // continued walk repeats or omits a row.
    let a = walk_entities(&router, &json!({"eventArns": forward.clone()}), 100).await;
    let b = walk_entities(&router, &json!({"eventArns": reversed.clone()}), 100).await;
    assert_eq!(
        a, b,
        "case eventArns forward vs reversed: the entity list must not depend on the \
         request's event order"
    );

    // And a token issued under one spelling continues the same walk under the other,
    // one row at a time, visiting every entity exactly once.
    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut flip = false;
    loop {
        let order = if flip { &reversed } else { &forward };
        flip = !flip;
        let mut request = json!({"filter": {"eventArns": order}, "maxResults": 1});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let body = call_ok(&router, "DescribeAffectedEntities", request).await;
        walked.extend(body["entities"].as_array().cloned().unwrap_or_default());
        assert!(
            walked.len() <= a.len(),
            "case alternating order: the walk visited more rows than exist, got {}",
            walked.len()
        );
        match body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        walked, a,
        "case alternating order: a token issued under one event order must continue \
         the same walk under the other"
    );
}

// ---------------------------------------------------------------------------
// DescribeEventAggregates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn event_aggregates_group_the_same_event_set_describe_events_returns() {
    let router = health_router();

    for case in FILTER_CASES {
        let filter = (case.filter)();
        let events = call_ok(&router, "DescribeEvents", json!({"filter": filter.clone()})).await
            ["events"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut expected: BTreeMap<String, i64> = BTreeMap::new();
        for event in &events {
            *expected
                .entry(text(event, "eventTypeCategory"))
                .or_insert(0) += 1;
        }

        let body = call_ok(
            &router,
            "DescribeEventAggregates",
            json!({"filter": filter, "aggregateField": "eventTypeCategory"}),
        )
        .await;
        let rows = body["eventAggregates"].as_array().cloned().unwrap_or_default();
        let actual: BTreeMap<String, i64> = rows
            .iter()
            .map(|row| (text(row, "aggregateValue"), row["count"].as_i64().unwrap_or(-1)))
            .collect();

        assert_eq!(
            actual, expected,
            "case {}: the per-category counts must group exactly the events \
             DescribeEvents returns",
            case.name
        );
        let total: i64 = actual.values().sum();
        assert_eq!(
            total,
            events.len() as i64,
            "case {}: the counts must sum to the {} matching events, got {total}",
            case.name,
            events.len()
        );
        for value in actual.keys() {
            assert!(
                EVENT_TYPE_CATEGORY_VALUES.contains(&value.as_str()),
                "case {}: aggregateValue must be a modelled eventTypeCategory, got {value}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn an_aggregate_field_outside_the_modelled_enum_is_refused() {
    let router = health_router();

    for field in EVENT_AGGREGATE_FIELD_VALUES {
        let (status, body) = call(
            &router,
            "DescribeEventAggregates",
            json!({"aggregateField": field}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case aggregateField={field}: a modelled value must be served, got {status}: {body}"
        );
    }

    for field in ["eventTypeCategories", "service", "region", ""] {
        let (status, body) = call(
            &router,
            "DescribeEventAggregates",
            json!({"aggregateField": field}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case aggregateField={field:?}: a value outside the enum is a 400, \
             got {status}: {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Cross-operation self-consistency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_event_type_catalog_is_derived_from_the_stored_events() {
    let router = health_router();
    let catalog = whole_catalog(&router).await;

    /// (service, code, category) triples the stored events carry.
    fn triples(events: &[Value]) -> Vec<(String, String, String)> {
        let mut seen: Vec<(String, String, String)> = events
            .iter()
            .map(|event| {
                (
                    text(event, "service"),
                    text(event, "eventTypeCode"),
                    text(event, "eventTypeCategory"),
                )
            })
            .collect();
        seen.sort();
        seen.dedup();
        seen
    }

    // Rows differ along which member is constrained, how many values it lists, and
    // whether any event carries them.
    let cases: Vec<(&str, Value)> = vec![
        ("no filter", json!({})),
        ("services=[EC2]", json!({"filter": {"services": ["EC2"]}})),
        (
            "services=[EC2,RDS]",
            json!({"filter": {"services": ["EC2", "RDS"]}}),
        ),
        (
            "services=[LAMBDA], absent from the catalog",
            json!({"filter": {"services": ["LAMBDA"]}}),
        ),
        (
            "eventTypeCategories=[issue]",
            json!({"filter": {"eventTypeCategories": ["issue"]}}),
        ),
        (
            "eventTypeCodes=[AWS_BILLING_NOTICE]",
            json!({"filter": {"eventTypeCodes": ["AWS_BILLING_NOTICE"]}}),
        ),
        (
            "actionabilities=[INFORMATIONAL]",
            json!({"filter": {"actionabilities": ["INFORMATIONAL"]}}),
        ),
        (
            "personas=[OPERATIONS]",
            json!({"filter": {"personas": ["OPERATIONS"]}}),
        ),
    ];

    for (name, request) in cases {
        let body = call_ok(&router, "DescribeEventTypes", request.clone()).await;
        let returned = body["eventTypes"].as_array().cloned().unwrap_or_default();

        // The same filter, answered through DescribeEvents, names the events whose
        // triples the event types must be.
        let mut events_request = json!({});
        if let Some(filter) = request.get("filter") {
            events_request["filter"] = filter.clone();
        }
        let matching_events = call_ok(&router, "DescribeEvents", events_request).await["events"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut actual: Vec<(String, String, String)> = returned
            .iter()
            .map(|event_type| {
                (
                    text(event_type, "service"),
                    text(event_type, "code"),
                    text(event_type, "category"),
                )
            })
            .collect();
        actual.sort();
        assert_eq!(
            actual,
            triples(&matching_events),
            "case {name}: the event types must be the distinct triples of the events the \
             same filter admits, got {actual:?}"
        );
        assert!(
            body.get("nextToken").and_then(Value::as_str).is_none(),
            "case {name}: a catalog smaller than one page carries no nextToken, got {body}"
        );
    }

    // And every triple of the whole catalog is reported by the unfiltered call.
    let all_types = call_ok(&router, "DescribeEventTypes", json!({})).await["eventTypes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        all_types.len(),
        triples(&catalog).len(),
        "the unfiltered event types must be exactly the catalog's distinct triples, \
         got {all_types:?}"
    );
}

#[tokio::test]
async fn every_listed_event_is_internally_consistent_across_the_read_loop() {
    let router = health_router();
    let catalog = whole_catalog(&router).await;

    for event in &catalog {
        let arn = text(event, "arn");
        assert!(
            arn.starts_with("arn:aws:health:")
                && arn.contains("::event/")
                && arn.matches('/').count() == 3,
            "the event ARN must have the documented \
             arn:aws:health:<region>::event/<SERVICE>/<CODE>/<ID> shape, got {arn}"
        );
        assert!(
            arn.contains(&format!(
                "::event/{}/{}/",
                text(event, "service"),
                text(event, "eventTypeCode")
            )),
            "the ARN must name the event's own service and type code, got {arn}"
        );
        assert!(
            EVENT_STATUS_CODE_VALUES.contains(&text(event, "statusCode").as_str()),
            "statusCode must be a modelled eventStatusCode, got {event}"
        );
        assert!(
            EVENT_TYPE_CATEGORY_VALUES.contains(&text(event, "eventTypeCategory").as_str()),
            "eventTypeCategory must be modelled, got {event}"
        );
        assert!(
            ["PUBLIC", "ACCOUNT_SPECIFIC", "NONE"]
                .contains(&text(event, "eventScopeCode").as_str()),
            "eventScopeCode must be modelled, got {event}"
        );

        // The ARN a client reads from DescribeEvents resolves in DescribeEventDetails,
        // its entities appear in DescribeAffectedEntities, and the aggregate agrees.
        let details = call_ok(
            &router,
            "DescribeEventDetails",
            json!({"eventArns": [arn.clone()]}),
        )
        .await;
        assert_eq!(
            details["successfulSet"][0]["event"], *event,
            "the detailed event must equal the listed summary for {arn}"
        );
        let entities = walk_entities(&router, &json!({"eventArns": [arn.clone()]}), 100).await;
        let aggregate = call_ok(
            &router,
            "DescribeEntityAggregates",
            json!({"eventArns": [arn.clone()]}),
        )
        .await;
        assert_eq!(
            aggregate["entityAggregates"][0]["count"].as_i64().unwrap_or(-1),
            entities.len() as i64,
            "the aggregate count must equal the entities served for {arn}"
        );
    }
}

#[tokio::test]
async fn reading_the_catalog_does_not_change_what_a_later_caller_sees() {
    let router = health_router();
    let before = whole_catalog(&router).await;

    // Drive the whole read loop, including paginated walks and refusals.
    let arns_all = arns(&before);
    let _ = call(&router, "DescribeEvents", json!({"maxResults": 1})).await;
    let _ = call(&router, "DescribeEvents", json!({"nextToken": "bogustoken"})).await;
    let _ = call(&router, "DescribeEventTypes", json!({})).await;
    let _ = call(
        &router,
        "DescribeEventDetails",
        json!({"eventArns": [arns_all[0].clone()]}),
    )
    .await;
    let _ = walk_entities(&router, &json!({"eventArns": arns_all.clone()}), 1).await;
    let _ = call(
        &router,
        "DescribeEntityAggregates",
        json!({"eventArns": arns_all.clone()}),
    )
    .await;
    let _ = call(
        &router,
        "DescribeEventAggregates",
        json!({"aggregateField": "eventTypeCategory"}),
    )
    .await;

    let after = whole_catalog(&router).await;
    assert_eq!(
        before, after,
        "AWS Health is a reporting API: no Describe* call may change the catalog a \
         later caller sees"
    );
}

// ---------------------------------------------------------------------------
// Tag tracking: ConfigureTags, ConfigureTagsForOrganization, DescribeTags,
// DescribeTagsForOrganization and the `tags` filter
// ---------------------------------------------------------------------------

/// The one `AccountTagType` the model declares, and the two `OrganizationTagType`
/// values, as the readers below must label their rows. Each is checked against the
/// crate's generated value set, so a model change that renames one fails here rather
/// than silently.
const ACCOUNT_RESOURCE_TAG: &str = "ACCOUNT_RESOURCE_TAG";
const ORGANIZATION_RESOURCE_TAG: &str = "ORGANIZATION_RESOURCE_TAG";
const ORGANIZATION_ACCOUNT_TAG: &str = "ORGANIZATION_ACCOUNT_TAG";

/// A tag key matching the modelled `^[a-zA-Z0-9 _.:\/=+\-@]{1,127}$` pattern that
/// nothing in the catalog carries, so configuring it tracks a key with no pair behind
/// it.
const UNUSED_TAG_KEY: &str = "NoSuchTagKey";

/// A router plus the engine behind it, so a case can read the stored records its wire
/// answers have to agree with instead of restating the catalog.
fn health_router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    set_support_plan(&engine, Some(SUBSCRIBED_PLAN));
    (health_router_over(Arc::clone(&engine)), engine)
}

/// Every resource tag pair the stored affected entities carry, whether or not the
/// account currently tracks its key.
fn stored_resource_tags(engine: &engine::InMemoryEngine) -> BTreeSet<(String, String)> {
    let state = service_com_amazonaws_health_awshealth_20160804::state::service_state(engine)
        .expect("the health store is reachable from the shared in-memory engine");
    let store = state.lock();
    store
        .entities
        .values()
        .flatten()
        .flat_map(|entity| {
            entity
                .resource_tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
        })
        .collect()
}

/// The tags attached to the account those entities belong to.
fn stored_account_tags(engine: &engine::InMemoryEngine) -> BTreeSet<(String, String)> {
    let state = service_com_amazonaws_health_awshealth_20160804::state::service_state(engine)
        .expect("the health store is reachable from the shared in-memory engine");
    let store = state.lock();
    store
        .account_tags
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// The distinct keys of a pair set, in a stable order.
fn keys_of(pairs: &BTreeSet<(String, String)>) -> Vec<String> {
    let mut keys: Vec<String> = pairs.iter().map(|(key, _)| key.clone()).collect();
    keys.dedup();
    keys
}

/// Configure one scope's tracked tag keys, through the wire path, and return the
/// response.
async fn configure_keys(router: &MultiServiceRouter, operation: &str, keys: &[String]) -> Value {
    let tag_keys: Vec<Value> = keys.iter().map(|key| json!({"key": key})).collect();
    call_ok(router, operation, json!({"tagKeys": tag_keys})).await
}

/// The `key` of each configured tag key a `Configure*` response reports, in order.
fn configured_keys(body: &Value) -> Vec<String> {
    body["configuredTagKeys"]
        .as_array()
        .map(|keys| keys.iter().map(|key| text(key, "key")).collect())
        .unwrap_or_default()
}

/// One page-at-a-time walk of a paginated tag reader.
async fn walk_tags(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
    page_size: usize,
) -> Vec<Value> {
    let mut walked = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut page_request = request.clone();
        page_request["maxResults"] = json!(page_size);
        if let Some(token) = &token {
            page_request["nextToken"] = json!(token);
        }
        let body = call_ok(router, operation, page_request).await;
        walked.extend(body["tags"].as_array().cloned().unwrap_or_default());
        match body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => return walked,
        }
        assert!(
            walked.len() < 1_000,
            "the {operation} walk of {request} does not terminate"
        );
    }
}

/// The (key, value, type) triples a tag reader returned.
fn tag_triples(tags: &[Value]) -> TagTriples {
    tags.iter()
        .map(|tag| (text(tag, "key"), text(tag, "value"), text(tag, "type")))
        .collect()
}

/// A set of (key, value, type) tag rows, as a reader reports them.
type TagTriples = BTreeSet<(String, String, String)>;

/// The same triples, built from stored pairs and the type they must carry.
fn expected_triples(
    pairs: impl IntoIterator<Item = (String, String)>,
    tag_type: &str,
) -> TagTriples {
    pairs
        .into_iter()
        .map(|(key, value)| (key, value, tag_type.to_string()))
        .collect()
}

/// The resource tags one affected entity reports.
fn entity_tags(entity: &Value) -> BTreeMap<String, String> {
    entity["resourceTags"]
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every resource tag pair a set of affected entities reports.
fn reported_entity_tags(entities: &[Value]) -> BTreeSet<(String, String)> {
    entities
        .iter()
        .flat_map(|entity| entity_tags(entity).into_iter())
        .collect()
}

/// RULE: `Configure*` replaces the whole tracked configuration with the keys the
/// request names — a key it omits stops being tracked — reporting one entry per
/// distinct key, in request order, with the `enabledForEventBridge` it was given and a
/// modelled `TagKeyStatus`.
///
/// Rows span what the rule generalises over: how many keys are configured (one, two,
/// the modelled maximum of ten), whether a key repeats, whether it is a key the
/// catalog carries, whether `enabledForEventBridge` is sent, and a later call that
/// shrinks the configuration. Both scopes are driven, because both operations state
/// the same sentence.
#[tokio::test]
async fn configuring_tag_keys_replaces_the_whole_configuration() {
    let (router, engine) = health_router_and_engine();
    let catalog_keys = keys_of(&stored_resource_tags(&engine));
    assert!(
        catalog_keys.len() >= 2,
        "the catalog must carry at least two distinct resource tag keys, got \
         {catalog_keys:?}"
    );
    let ten: Vec<String> = (0..10).map(|index| format!("Key{index}")).collect();

    // (case, the keys sent, the distinct keys that must be left configured)
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "one key",
            vec![catalog_keys[0].clone()],
            vec![catalog_keys[0].clone()],
        ),
        (
            "two keys, in request order",
            vec![catalog_keys[0].clone(), catalog_keys[1].clone()],
            vec![catalog_keys[0].clone(), catalog_keys[1].clone()],
        ),
        (
            "a key named twice is one entry",
            vec![catalog_keys[0].clone(), catalog_keys[0].clone()],
            vec![catalog_keys[0].clone()],
        ),
        (
            "a key the catalog does not carry",
            vec![UNUSED_TAG_KEY.to_string()],
            vec![UNUSED_TAG_KEY.to_string()],
        ),
        (
            "the modelled maximum of ten keys",
            ten.clone(),
            ten.clone(),
        ),
        (
            "back to one key: the configuration is replaced, not merged",
            vec![catalog_keys[1].clone()],
            vec![catalog_keys[1].clone()],
        ),
    ];

    for operation in ["ConfigureTags", "ConfigureTagsForOrganization"] {
        for (case, sent, expected) in &rows {
            let body = configure_keys(&router, operation, sent).await;
            let actual = configured_keys(&body);
            assert_eq!(
                &actual, expected,
                "case {operation} {case:?}: expected the configuration {expected:?}, \
                 got {actual:?} in {body}"
            );
            for key in body["configuredTagKeys"].as_array().unwrap_or(&Vec::new()) {
                assert!(
                    TAG_KEY_STATUS_VALUES.contains(&text(key, "status").as_str()),
                    "case {operation} {case:?}: status must be a modelled TagKeyStatus, \
                     got {key}"
                );
                assert_eq!(
                    key["enabledForEventBridge"],
                    json!(false),
                    "case {operation} {case:?}: an unsent enabledForEventBridge is the \
                     modelled default of false, got {key}"
                );
            }
        }

        // enabledForEventBridge is carried per key, as sent.
        let body = call_ok(
            &router,
            operation,
            json!({"tagKeys": [
                {"key": catalog_keys[0].clone(), "enabledForEventBridge": true},
                {"key": catalog_keys[1].clone(), "enabledForEventBridge": false},
            ]}),
        )
        .await;
        let flags: Vec<&Value> = body["configuredTagKeys"]
            .as_array()
            .map(|keys| {
                keys.iter()
                    .map(|key| &key["enabledForEventBridge"])
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            flags,
            vec![&json!(true), &json!(false)],
            "case {operation} enabledForEventBridge: each key must report the flag it \
             was configured with, got {body}"
        );

        // A configuration above the modelled maximum is refused before the handler.
        let (status, refusal) = call(
            &router,
            operation,
            json!({"tagKeys": (0..11)
                .map(|index| json!({"key": format!("Key{index}")}))
                .collect::<Vec<Value>>()}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {operation} eleven keys: above the modelled maximum of ten, \
             got {status}: {refusal}"
        );
    }
}

/// RULE: only a configured tag key appears in the event data this API reports — the
/// `resourceTags` of an affected entity and the tags `DescribeTags` lists are the
/// pairs the stored entities carry whose key the account tracks, and nothing else.
///
/// Rows vary the configuration: none at all (a new account), one key, every key the
/// catalog carries, a key no entity carries, and a mix of the two.
#[tokio::test]
async fn only_configured_tag_keys_appear_in_the_event_data() {
    let (router, engine) = health_router_and_engine();
    let stored = stored_resource_tags(&engine);
    let catalog_keys = keys_of(&stored);
    let every_arn = arns(&whole_catalog(&router).await);
    assert!(
        ACCOUNT_TAG_TYPE_VALUES.contains(&ACCOUNT_RESOURCE_TAG),
        "the modelled AccountTagType values must include {ACCOUNT_RESOURCE_TAG}, got \
         {ACCOUNT_TAG_TYPE_VALUES:?}"
    );

    let mut rows: Vec<(&str, Vec<String>)> = vec![
        // FIRST, before anything is configured: a new account tracks no key, so none
        // of the pairs its entities carry is reported.
        ("nothing configured", Vec::new()),
        ("one key", vec![catalog_keys[0].clone()]),
        ("a key no entity carries", vec![UNUSED_TAG_KEY.to_string()]),
        ("every key the catalog carries", catalog_keys.clone()),
    ];
    let mut mixed = catalog_keys.clone();
    mixed.push(UNUSED_TAG_KEY.to_string());
    rows.push(("every key plus one no entity carries", mixed));

    for (case, configured) in rows {
        if !configured.is_empty() {
            configure_keys(&router, "ConfigureTags", &configured).await;
        }
        let expected: BTreeSet<(String, String)> = stored
            .iter()
            .filter(|(key, _)| configured.contains(key))
            .cloned()
            .collect();

        let tags = walk_tags(&router, "DescribeTags", &json!({}), 100).await;
        assert_eq!(
            tag_triples(&tags),
            expected_triples(expected.iter().cloned(), ACCOUNT_RESOURCE_TAG),
            "case {case:?}: DescribeTags must report exactly the tracked pairs the \
             stored entities carry, got {tags:?}"
        );

        let entities = walk_entities(&router, &json!({"eventArns": every_arn.clone()}), 100).await;
        assert_eq!(
            reported_entity_tags(&entities),
            expected,
            "case {case:?}: an affected entity must report exactly its tracked tags, \
             got {entities:?}"
        );
        for entity in &entities {
            for key in entity_tags(entity).keys() {
                assert!(
                    configured.contains(key),
                    "case {case:?}: {key} is not a configured tag key, got {entity}"
                );
            }
        }
    }
}

/// RULE: a `tags` filter member is a map of tag key to the values that key accepts; a
/// tag matches when its key is in the map and its value is one of the values listed
/// for that key, the comparison is case sensitive, and the filter matches when at
/// least one key in the map matches. An untracked key matches nothing, and an absent
/// map constrains nothing.
///
/// Asserted over the three operations that take such a member — `DescribeAffectedEntities`
/// (the entities), `DescribeEvents` (the events those entities belong to) and
/// `DescribeEntityAggregates` (their counts) — with rows differing along the number of
/// keys in the map, the number of values listed for a key, whether a value matches,
/// the case of the value, and whether the key is one the account tracks.
#[tokio::test]
async fn the_tags_filter_admits_a_record_whose_tracked_tag_matches_a_listed_value() {
    let (router, engine) = health_router_and_engine();
    let stored = stored_resource_tags(&engine);
    let catalog_keys = keys_of(&stored);
    configure_keys(&router, "ConfigureTags", &catalog_keys).await;

    let catalog = whole_catalog(&router).await;
    let every_arn = arns(&catalog);
    let all = walk_entities(&router, &json!({"eventArns": every_arn.clone()}), 100).await;
    let tagged: Vec<&Value> = all
        .iter()
        .filter(|entity| !entity_tags(entity).is_empty())
        .collect();
    assert!(
        tagged.len() >= 2,
        "the catalog must carry tagged entities for this rule, got {all:?}"
    );

    // A pair of each of the two keys, taken from the catalog itself.
    let (key_a, value_a) = stored
        .iter()
        .find(|(key, _)| key == &catalog_keys[0])
        .cloned()
        .expect("a stored pair of the first key");
    let (key_b, value_b) = stored
        .iter()
        .find(|(key, _)| key == &catalog_keys[1])
        .cloned()
        .expect("a stored pair of the second key");
    let shouted = value_a.to_uppercase();
    assert_ne!(
        shouted, value_a,
        "the case-sensitivity row needs a value whose upper case differs, got {value_a}"
    );

    type Admits = Box<dyn Fn(&BTreeMap<String, String>) -> bool>;
    let a = (key_a.clone(), value_a.clone());
    let b = (key_b.clone(), value_b.clone());
    let rows: Vec<(String, Value, Admits)> = vec![
        (
            "no tags member at all".to_string(),
            Value::Null,
            Box::new(|_| true),
        ),
        (
            format!("one key, one value: {key_a}={value_a}"),
            json!({key_a.clone(): [value_a.clone()]}),
            Box::new({
                let a = a.clone();
                move |tags| tags.get(&a.0) == Some(&a.1)
            }),
        ),
        (
            "one key, two values, one of them stored".to_string(),
            json!({key_a.clone(): [value_a.clone(), "no-such-value"]}),
            Box::new({
                let a = a.clone();
                move |tags| tags.get(&a.0) == Some(&a.1)
            }),
        ),
        (
            "one key, a value nothing carries".to_string(),
            json!({key_a.clone(): ["no-such-value"]}),
            Box::new(|_| false),
        ),
        (
            "two keys: at least one matching is enough".to_string(),
            json!({key_a.clone(): [value_a.clone()], key_b.clone(): [value_b.clone()]}),
            Box::new({
                let (a, b) = (a.clone(), b.clone());
                move |tags| tags.get(&a.0) == Some(&a.1) || tags.get(&b.0) == Some(&b.1)
            }),
        ),
        (
            format!("the value in another case: {shouted}"),
            json!({key_a.clone(): [shouted.clone()]}),
            Box::new(|_| false),
        ),
        (
            "a key no entity carries".to_string(),
            json!({UNUSED_TAG_KEY: [value_a.clone()]}),
            Box::new(|_| false),
        ),
    ];

    for (case, tags, admits) in &rows {
        let mut entity_filter = json!({"eventArns": every_arn.clone()});
        let mut event_filter = json!({});
        let mut aggregate_filter = json!({});
        if !tags.is_null() {
            entity_filter["tags"] = tags.clone();
            event_filter["tags"] = tags.clone();
            aggregate_filter["tags"] = tags.clone();
        }

        let expected: Vec<&Value> = all
            .iter()
            .filter(|entity| admits(&entity_tags(entity)))
            .collect();
        let expected_arns: BTreeSet<String> = expected
            .iter()
            .map(|entity| text(entity, "entityArn"))
            .collect();

        let got = walk_entities(&router, &entity_filter, 100).await;
        let got_arns: BTreeSet<String> = got
            .iter()
            .map(|entity| text(entity, "entityArn"))
            .collect();
        assert_eq!(
            got_arns, expected_arns,
            "case {case:?}: DescribeAffectedEntities must serve exactly the entities \
             the tag filter admits, got {got_arns:?}"
        );

        // The same member on DescribeEvents selects an event by the entities it
        // affects: an event is admitted when at least one of its entities is.
        let events = call_ok(&router, "DescribeEvents", json!({"filter": event_filter})).await
            ["events"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut expected_events: BTreeSet<String> = expected
            .iter()
            .map(|entity| text(entity, "eventArn"))
            .collect();
        if tags.is_null() {
            expected_events = every_arn.iter().cloned().collect();
        }
        assert_eq!(
            arns(&events).into_iter().collect::<BTreeSet<String>>(),
            expected_events,
            "case {case:?}: DescribeEvents must serve the events whose entities the tag \
             filter admits, got {events:?}"
        );

        // And the aggregates count the same entities, event by event.
        let mut aggregate_request = json!({"eventArns": every_arn.clone()});
        if !tags.is_null() {
            aggregate_request["filter"] = aggregate_filter.clone();
        }
        let aggregates = call_ok(&router, "DescribeEntityAggregates", aggregate_request).await;
        for row in aggregates["entityAggregates"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let event_arn = text(&row, "eventArn");
            let counted = expected
                .iter()
                .filter(|entity| text(entity, "eventArn") == event_arn)
                .count();
            assert_eq!(
                row["count"].as_i64().unwrap_or(-1),
                counted as i64,
                "case {case:?}: the aggregate for {event_arn} must count the {counted} \
                 entities the same filter admits, got {row}"
            );
        }
    }

    // The same filter, with the key no longer tracked: an untracked tag is not part of
    // the event data the filter runs over, so it admits nothing.
    configure_keys(&router, "ConfigureTags", std::slice::from_ref(&key_b)).await;
    let got = walk_entities(
        &router,
        &json!({"eventArns": every_arn.clone(), "tags": {key_a.clone(): [value_a.clone()]}}),
        100,
    )
    .await;
    assert!(
        got.is_empty(),
        "case an untracked key: expected no entity, got {got:?}"
    );
}

/// RULE: a tag key is `PENDING` until its configuration has propagated and `ACTIVE`
/// from then on, and re-sending an unchanged configuration (both operations are
/// `@idempotent`) neither restarts that propagation nor moves a key back to `PENDING`.
#[tokio::test]
async fn a_newly_configured_tag_key_is_pending_until_it_has_propagated() {
    let (router, engine) = health_router_and_engine();
    let keys = keys_of(&stored_resource_tags(&engine));
    let first = vec![keys[0].clone()];

    for operation in ["ConfigureTags", "ConfigureTagsForOrganization"] {
        let body = configure_keys(&router, operation, &first).await;
        assert_eq!(
            text(&body["configuredTagKeys"][0], "status"),
            "PENDING",
            "case {operation}: a key just configured has not propagated yet, got {body}"
        );

        // Wait for the CONDITION: the same idempotent call reports the key active once
        // its configuration has propagated.
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let body = configure_keys(&router, operation, &first).await;
            if text(&body["configuredTagKeys"][0], "status") == "ACTIVE" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {operation}: the configuration never became ACTIVE, got {body}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Once active it stays active: the configuration is unchanged, so nothing
        // restarts.
        for round in 0..3 {
            let body = configure_keys(&router, operation, &first).await;
            assert_eq!(
                text(&body["configuredTagKeys"][0], "status"),
                "ACTIVE",
                "case {operation} round {round}: re-sending an unchanged configuration \
                 must keep the key ACTIVE, got {body}"
            );
        }

        // A key added later is the one that is pending; the one already active is not.
        let both = vec![keys[0].clone(), keys[1].clone()];
        let body = configure_keys(&router, operation, &both).await;
        assert_eq!(
            [
                text(&body["configuredTagKeys"][0], "status"),
                text(&body["configuredTagKeys"][1], "status"),
            ],
            ["ACTIVE".to_string(), "PENDING".to_string()],
            "case {operation}: only the newly configured key is pending, got {body}"
        );
    }
}

/// RULE: `DescribeTagsForOrganization` reports the tracked tags behind the
/// organization's events, each labelled with the `OrganizationTagType` of where it is
/// attached — a tag on an affected resource, or a tag on the account that owns it —
/// and `filter.tagTypes` narrows the result to the types it lists. The organization's
/// configuration is its own: the account-scoped one does not decide what this reader
/// returns.
#[tokio::test]
async fn the_organization_tag_reader_reports_the_type_of_each_tracked_tag() {
    let (router, engine) = health_router_and_engine();
    let resource_tags = stored_resource_tags(&engine);
    let account_tags = stored_account_tags(&engine);
    assert!(
        !account_tags.is_empty(),
        "the account must carry tags for this rule, got none"
    );
    for tag_type in [ORGANIZATION_RESOURCE_TAG, ORGANIZATION_ACCOUNT_TAG] {
        assert!(
            ORGANIZATION_TAG_TYPE_VALUES.contains(&tag_type),
            "the modelled OrganizationTagType values must include {tag_type}, got \
             {ORGANIZATION_TAG_TYPE_VALUES:?}"
        );
    }

    // Track every key behind either kind of tag — within the modelled maximum of ten.
    let mut keys = keys_of(&resource_tags);
    keys.extend(keys_of(&account_tags));
    assert!(
        keys.len() <= 10,
        "this case configures one key per stored tag key, which must stay within the \
         modelled maximum of ten, got {keys:?}"
    );
    configure_keys(&router, "ConfigureTagsForOrganization", &keys).await;

    let resource_rows = expected_triples(resource_tags.iter().cloned(), ORGANIZATION_RESOURCE_TAG);
    let account_rows = expected_triples(account_tags.iter().cloned(), ORGANIZATION_ACCOUNT_TAG);
    let both: TagTriples = resource_rows.union(&account_rows).cloned().collect();

    // (case, the tagTypes filter sent, the rows it must leave)
    let rows: Vec<(&str, Value, &TagTriples)> = vec![
        ("no filter at all", json!({}), &both),
        (
            "tagTypes: resource tags only",
            json!({"filter": {"tagTypes": [ORGANIZATION_RESOURCE_TAG]}}),
            &resource_rows,
        ),
        (
            "tagTypes: account tags only",
            json!({"filter": {"tagTypes": [ORGANIZATION_ACCOUNT_TAG]}}),
            &account_rows,
        ),
        (
            "tagTypes: both",
            json!({"filter": {"tagTypes": [ORGANIZATION_RESOURCE_TAG, ORGANIZATION_ACCOUNT_TAG]}}),
            &both,
        ),
    ];
    for (case, request, expected) in rows {
        let tags = walk_tags(&router, "DescribeTagsForOrganization", &request, 100).await;
        assert_eq!(
            &tag_triples(&tags),
            expected,
            "case {case:?}: expected {expected:?}, got {tags:?}"
        );
    }

    // The two scopes are separate configurations: nothing has configured the account's,
    // so its reader still reports nothing.
    let account_view = walk_tags(&router, "DescribeTags", &json!({}), 100).await;
    assert!(
        account_view.is_empty(),
        "the organization's configuration must not decide what the account reader \
         returns, got {account_view:?}"
    );
}

/// RULE: both tag readers page through one result set — the same rows whatever the page
/// size — issue a token bound to the operation and request that produced it, refuse a
/// token they did not issue with `InvalidPaginationToken`, and change nothing a later
/// caller reads.
#[tokio::test]
async fn the_tag_readers_paginate_and_change_nothing() {
    let (router, engine) = health_router_and_engine();
    let mut keys = keys_of(&stored_resource_tags(&engine));
    keys.extend(keys_of(&stored_account_tags(&engine)));
    configure_keys(&router, "ConfigureTags", &keys).await;
    configure_keys(&router, "ConfigureTagsForOrganization", &keys).await;

    for operation in ["DescribeTags", "DescribeTagsForOrganization"] {
        let whole = walk_tags(&router, operation, &json!({}), 100).await;
        assert!(
            whole.len() >= 2,
            "case {operation}: this rule needs at least two rows to page, got {whole:?}"
        );
        for page_size in [1, 2, 3, 100] {
            let paged = walk_tags(&router, operation, &json!({}), page_size).await;
            assert_eq!(
                paged, whole,
                "case {operation} maxResults={page_size}: paging must not change the \
                 rows, got {paged:?}"
            );
        }

        // A token the reader never issued is refused rather than answered.
        let (status, body) = call(&router, operation, json!({"nextToken": "zzzz"})).await;
        assert_eq!(
            status, 400,
            "case {operation}: an invented token is refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {operation}: expected InvalidPaginationToken, got {body}"
        );

        // Reading twice gives the same rows: a reader changes nothing.
        let again = walk_tags(&router, operation, &json!({}), 100).await;
        assert_eq!(
            again, whole,
            "case {operation}: a second read must serve the same rows, got {again:?}"
        );
    }

    // A token one reader issued does not continue the other's walk.
    let first_page = call_ok(&router, "DescribeTags", json!({"maxResults": 1})).await;
    let token = first_page
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("DescribeTags issues a token when more rows remain")
        .to_string();
    let (status, body) = call(
        &router,
        "DescribeTagsForOrganization",
        json!({"nextToken": token}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a token is bound to the operation that issued it, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "expected InvalidPaginationToken for another operation's token, got {body}"
    );
}
