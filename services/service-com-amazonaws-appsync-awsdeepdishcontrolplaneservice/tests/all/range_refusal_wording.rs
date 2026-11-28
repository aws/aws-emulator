// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE this service states a value outside a member's modelled `@range` in, end
//! to end over the restJson1 wire.
//!
//! The rule under test, one level above the two requests that filed the issue:
//!
//! > A value outside a member's modelled `@range` is refused with the operation's own
//! > modelled client error — AppSync's `BadRequestException` at HTTP 400 — carrying the
//! > sentence the service states THAT member's range in, with both published bounds in it:
//! > a batch size as a value that must lie between them ("Invalid value for maxBatchSize.
//! > Must be between 0 and 2000 (inclusive)"), a page size as one that must be above the
//! > minimum and at most the maximum ("maxResults must be greater than 0 and less than or
//! > equal to 25."). A value INSIDE the range — each published bound included — is served.
//!
//! One surface words that page size differently: the `/v2/apis…` listing answers the
//! validation layer's own constraint report for it, which `crate::aws_worded_refusals`
//! asserts.
//!
//! The rows differ along what the rule generalises over: the MEMBER, the OPERATION
//! carrying it (and so the method and whether the value rides in the body or in a query
//! parameter), WHICH bound the value crosses, and how far past it the value is. The bounds
//! are read from the model, on the constant that carries each one.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, create_api, router, Answer};

/// AppSync's own modelled error for a request that is not well formed, and its status
/// (`smithy_query(query="shape", shape="BadRequestException")`, `@httpError` 400).
///
/// Shared with the suites that assert the same refusal on other operations.
pub(crate) const BAD_REQUEST: &str = "BadRequestException";
pub(crate) const BAD_REQUEST_STATUS: u16 = 400;

/// `MaxBatchSize`'s published bounds (`@range` 0..2000,
/// `smithy_query(query="shape", shape="MaxBatchSize")`).
const MAX_BATCH_SIZE_MIN: i64 = 0;
const MAX_BATCH_SIZE_MAX: i64 = 2_000;

/// `MaxResults`'s published bounds (`@range` 0..25,
/// `smithy_query(query="shape", shape="MaxResults")`).
///
/// Shared with the suites that assert the same range on the other operations carrying the
/// member, so the published bounds are stated in one place.
pub(crate) const MAX_RESULTS_MIN: i64 = 0;
pub(crate) const MAX_RESULTS_MAX: i64 = 25;

/// The sentence the service states a `maxBatchSize` outside its range in: the bounds
/// written plainly, with no thousands separator.
fn batch_size_sentence() -> String {
    format!(
        "Invalid value for maxBatchSize. Must be between {MAX_BATCH_SIZE_MIN} and \
         {MAX_BATCH_SIZE_MAX} (inclusive)"
    )
}

/// The sentence the service states a `maxResults` outside its range in.
///
/// Shared with the suites that assert the same rule on the other operations carrying the
/// member, so the wording lives in one place.
pub(crate) fn page_size_sentence() -> String {
    format!(
        "maxResults must be greater than {MAX_RESULTS_MIN} and less than or equal to \
         {MAX_RESULTS_MAX}."
    )
}

/// One GraphQL API with one data source, which is what a function and a resolver are
/// written against.
async fn fixture(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let api_id = create_api(router, name).await;
    let data_source = "orders".to_string();
    let created = call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": data_source, "type": "NONE"})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case seed the data source: expected 200, got {created:?}"
    );
    (api_id, data_source)
}

/// A `CreateFunction`/`UpdateFunction` body carrying `max_batch_size`.
fn function_body(data_source: &str, max_batch_size: i64) -> Value {
    json!({
        "name": "resolve_orders",
        "dataSourceName": data_source,
        "functionVersion": "2018-05-29",
        "maxBatchSize": max_batch_size,
    })
}

/// A `CreateResolver`/`UpdateResolver` body carrying `max_batch_size`.
fn resolver_body(data_source: &str, max_batch_size: i64) -> Value {
    json!({
        "fieldName": "orders",
        "dataSourceName": data_source,
        "maxBatchSize": max_batch_size,
    })
}

/// One operation that carries `maxBatchSize`, as the request that would set it: the
/// label, the method and the path, plus the builder for its body at a given value.
struct BatchCarrier {
    operation: &'static str,
    method: &'static str,
    path: String,
    body: fn(&str, i64) -> Value,
}

/// Every operation that carries `maxBatchSize`.
fn batch_size_carriers(api_id: &str) -> Vec<BatchCarrier> {
    vec![
        BatchCarrier {
            operation: "CreateFunction",
            method: "POST",
            path: format!("/v1/apis/{api_id}/functions"),
            body: function_body,
        },
        BatchCarrier {
            operation: "UpdateFunction",
            method: "POST",
            // A `functionId` the API does not hold: the value is refused at the boundary,
            // before any resource is resolved, which is the whole point of the row.
            path: format!("/v1/apis/{api_id}/functions/absentfunction"),
            body: function_body,
        },
        BatchCarrier {
            operation: "CreateResolver",
            method: "POST",
            path: format!("/v1/apis/{api_id}/types/Query/resolvers"),
            body: resolver_body,
        },
        BatchCarrier {
            operation: "UpdateResolver",
            method: "POST",
            path: format!("/v1/apis/{api_id}/types/Query/resolvers/orders"),
            body: resolver_body,
        },
    ]
}

/// Every list operation of the v1 control plane that carries `maxResults`, as a path to
/// page.
///
/// `ListApis` is NOT one of them: the `/v2/apis…` surface answers a page size outside the
/// range with the validation layer's own constraint report rather than with the sentence
/// this suite asserts, and that surface's refusal is asserted in
/// `crate::aws_worded_refusals` — the same split `crate::event_api_name_form` holds for
/// that surface's `name`.
fn page_size_carriers(api_id: &str) -> Vec<(&'static str, String)> {
    vec![
        ("ListGraphqlApis", "/v1/apis".to_string()),
        ("ListDomainNames", "/v1/domainnames".to_string()),
        ("ListDataSources", format!("/v1/apis/{api_id}/datasources")),
        ("ListFunctions", format!("/v1/apis/{api_id}/functions")),
        ("ListApiKeys", format!("/v1/apis/{api_id}/apikeys")),
    ]
}

/// RULE: a `maxBatchSize` outside its modelled range takes the service's sentence for
/// that member, on every operation that carries it.
#[tokio::test]
async fn a_batch_size_outside_its_range_states_the_services_sentence() {
    let router = router();
    let (api_id, data_source) = fixture(&router, "Api_batch_size_range").await;
    let expected = batch_size_sentence();

    // One below the minimum, one past the maximum, and values far past either bound.
    for value in [
        MAX_BATCH_SIZE_MIN - 1,
        MAX_BATCH_SIZE_MAX + 1,
        -1_000,
        1_000_000,
    ] {
        for carrier in batch_size_carriers(&api_id) {
            let BatchCarrier {
                operation,
                method,
                path,
                body,
            } = carrier;
            let case = format!("{operation} with maxBatchSize {value}");
            let answer: Answer =
                call(&router, method, &path, Some(body(&data_source, value))).await;
            assert_eq!(
                (answer.status, answer.error_type.as_str()),
                (BAD_REQUEST_STATUS, BAD_REQUEST),
                "case {case:?}: expected {BAD_REQUEST_STATUS} {BAD_REQUEST}, got {answer:?}"
            );
            assert_eq!(
                answer.message, expected,
                "case {case:?}: expected {expected:?}, got {answer:?}"
            );
        }
    }
}

/// RULE's other half: each published bound of `maxBatchSize` is served, so the rows above
/// are refused for the value rather than for the operation.
#[tokio::test]
async fn a_batch_size_on_each_published_bound_is_served() {
    let router = router();
    let (api_id, data_source) = fixture(&router, "Api_batch_size_bounds").await;

    for value in [MAX_BATCH_SIZE_MIN, MAX_BATCH_SIZE_MAX, 1, 999] {
        let mut body = function_body(&data_source, value);
        body.as_object_mut()
            .expect("the body is an object")
            .insert("name".to_string(), json!(format!("fn_batch_{value}")));
        let answer = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/functions"),
            Some(body),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case CreateFunction with maxBatchSize {value}: expected the value inside the \
             range to be served, got {answer:?}"
        );
        assert_eq!(
            answer.body["functionConfiguration"]["maxBatchSize"],
            json!(value),
            "case CreateFunction with maxBatchSize {value}: expected the value back, got \
             {answer:?}"
        );
    }
}

/// RULE: a `maxResults` outside its modelled range takes the service's sentence for that
/// member, on every list operation, and each published bound is served.
#[tokio::test]
async fn a_page_size_outside_its_range_states_the_services_sentence() {
    let router = router();
    let (api_id, _data_source) = fixture(&router, "Api_page_size_range").await;
    let expected = page_size_sentence();

    for (operation, base) in page_size_carriers(&api_id) {
        // One below the minimum, one past the maximum, and values far past either bound.
        for value in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1, -99, 1_000] {
            let case = format!("{operation} with maxResults {value}");
            let answer = call(&router, "GET", &format!("{base}?maxResults={value}"), None).await;
            assert_eq!(
                (answer.status, answer.error_type.as_str()),
                (BAD_REQUEST_STATUS, BAD_REQUEST),
                "case {case:?}: expected {BAD_REQUEST_STATUS} {BAD_REQUEST}, got {answer:?}"
            );
            assert_eq!(
                answer.message, expected,
                "case {case:?}: expected {expected:?}, got {answer:?}"
            );
        }

        // Each published bound, and a value between them, is served.
        for value in [MAX_RESULTS_MIN, MAX_RESULTS_MAX, 7] {
            let case = format!("{operation} with maxResults {value}");
            let answer = call(&router, "GET", &format!("{base}?maxResults={value}"), None).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the value inside the range to be served, got \
                 {answer:?}"
            );
        }
    }
}
