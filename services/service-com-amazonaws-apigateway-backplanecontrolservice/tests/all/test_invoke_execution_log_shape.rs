// SPDX-License-Identifier: CC0-1.0

//! The SHAPE of the execution log the `TestInvoke*` operations report.
//!
//! The rule these cases assert, over a set of simulated invocations rather than one recorded
//! call: an execution log is a log OF ONE INVOCATION — it opens by naming that invocation's
//! own request id, a value minted per call, and every line after that opening carries the
//! instant the invocation ran at, stated as `<abbreviated weekday> <abbreviated month> <day>
//! <time> <zone> <year>`, followed by ` : ` and the step it reports. So a client splits a
//! line into "when" and "what" on that separator, and correlates the log it holds with the
//! invocation it asked for.
//!
//! What the rows span: the integration the simulation performs (a `MOCK` one, one carrying no
//! integration response, one this api cannot perform), the payload a step carries (none, one
//! line, several lines, a line-terminated document, more lines than the log states), the verb,
//! the parameters the request was made with (path parameters, query parameters, neither), and
//! both operations that report a log (`TestInvokeMethod` and `TestInvokeAuthorizer`) — the
//! log's shape does not turn on any of them.
//!
//! Every case drives the real wire path — a `restJson1` request through the multi-service
//! router into the mounted service — through the crate's one wire-test harness
//! ([`Gateway`]), so what is asserted is what a client sees.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{create_api, resource_under, Gateway};

/// The abbreviated weekday names an execution log's instant opens with, in the catalog's own
/// order (`std::fmt` of a date in this form is the C/Java `EEE` field).
const WEEKDAYS: &[&str] = &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// The abbreviated month names an execution log's instant names the month with (the `MMM`
/// field of the same form).
const MONTHS: &[&str] = &[
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// What separates the instant a line reports from the step it reports.
const SEPARATOR: &str = " : ";

/// The opening of the line an execution log opens with.
const OPENS: &str = "Execution log for request ";

/// The most lines an execution log STATES, as the operation's own log builder bounds it
/// (`MAX_LOG_LINES` in `src/ops/test_invoke_method.rs`, which also states the line the log
/// carries instead of the rest once the bound is reached): a step's text is as large as the
/// request that caused it, so a log cannot grow with what one request carried.
const MAX_LOG_LINES: usize = 512;

/// What a log that reached that bound states instead of the lines it does not carry — the
/// same sentence the log builder states it with.
const BOUND_REACHED: &str = "The rest of this execution log is not stated";

/// Whether `stated` is an instant as a line of an execution log states it: the abbreviated
/// weekday, the abbreviated month, the two-digit day of the month, the time of day, the zone
/// and the four-digit year, separated by single spaces.
fn is_stated_instant(stated: &str) -> bool {
    let fields: Vec<&str> = stated.split(' ').collect();
    let [weekday, month, day, time, zone, year] = fields.as_slice() else {
        return false;
    };
    let digits = |text: &str, width: usize| {
        text.len() == width && text.bytes().all(|byte| byte.is_ascii_digit())
    };
    let clock: Vec<&str> = time.split(':').collect();
    WEEKDAYS.contains(weekday)
        && MONTHS.contains(month)
        && digits(day, 2)
        && clock.len() == 3
        && clock.iter().all(|field| digits(field, 2))
        && !zone.is_empty()
        && zone.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && digits(year, 4)
}

/// The request id an execution log names, asserting the whole log's shape: the opening line
/// names a request by an id of the form a request id has, and every line after it states the
/// invocation's instant, the separator and a step.
///
/// Answers the id so a case can assert what two logs agree and differ on.
fn request_id_of(log: &str, case: &str) -> String {
    let mut lines = log.split('\n');
    let opening = lines
        .next()
        .unwrap_or_else(|| panic!("case {case}: the log carries no line at all"));
    let id = opening.strip_prefix(OPENS).unwrap_or_else(|| {
        panic!("case {case}: expected the log to open with {OPENS:?}, got {opening:?}")
    });
    assert!(
        ::service_common::query_error_envelope::is_request_id(id),
        "case {case}: expected the log to name the invocation by a request id, got {id:?}"
    );
    let mut steps = 0usize;
    for line in lines {
        let (stated, step) = line.split_once(SEPARATOR).unwrap_or_else(|| {
            panic!(
                "case {case}: expected every line to state its instant and a step separated by \
                 {SEPARATOR:?}, got {line:?}"
            )
        });
        assert!(
            is_stated_instant(stated),
            "case {case}: expected the line to state the invocation's instant, got {stated:?} \
             in {line:?}"
        );
        assert!(
            !step.is_empty(),
            "case {case}: expected the line to report a step, got {line:?}"
        );
        steps += 1;
    }
    assert!(
        steps >= 2,
        "case {case}: expected the log to report the steps it took, got {log:?}"
    );
    assert!(
        steps <= MAX_LOG_LINES,
        "case {case}: expected at most {MAX_LOG_LINES} stated lines, got {steps}"
    );
    assert!(
        log.contains(&format!("Starting execution for request: {id}")),
        "case {case}: expected the log to name the request it started, got {log:?}"
    );
    id.to_string()
}

/// `PutMethod` with open authorization, then `PutIntegration` and (when the row names one) a
/// `PutIntegrationResponse` for 200 — the records a simulated invocation is performed from.
async fn method_with(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    integration: Value,
    integration_response: Option<Value>,
) {
    for (what, path, request) in [
        (
            "PutMethod",
            format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
            Some(json!({ "authorizationType": "NONE" })),
        ),
        (
            "PutIntegration",
            format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
            Some(integration.clone()),
        ),
    ] {
        let answer = gateway.call("PUT", &path, request).await;
        assert_eq!(
            answer.status, 201,
            "case {what}({verb}): expected 201, got {} with {:?}",
            answer.status, answer.body
        );
    }
    if let Some(mapped) = integration_response {
        let answer = gateway
            .call(
                "PUT",
                &format!(
                    "/restapis/{api}/resources/{resource}/methods/{verb}/integration/responses/200"
                ),
                Some(mapped),
            )
            .await;
        assert_eq!(
            answer.status, 201,
            "case PutIntegrationResponse({verb}): expected 201, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// `TestInvokeMethod`, answering the whole reported body.
async fn simulate(gateway: &Gateway, api: &str, resource: &str, verb: &str, request: Value) -> Value {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
            Some(request),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case TestInvokeMethod({verb}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// The `log` member of a reported simulation.
fn reported_log(body: &Value, case: &str) -> String {
    body.get("log")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: the simulation reports a log, got {body:?}"))
        .to_string()
}

/// One row: a method configuration and the simulated request to perform against it.
struct Row {
    /// What makes this row its own case.
    case: &'static str,
    /// The path part of the resource this row stands its method up on.
    path_part: &'static str,
    /// The verb the method is put under.
    verb: &'static str,
    /// The `PutIntegration` request.
    integration: Value,
    /// The `PutIntegrationResponse` for 200, where the row needs one.
    integration_response: Option<Value>,
    /// The simulated request.
    request: Value,
}

#[tokio::test]
async fn every_line_of_a_simulated_invocations_log_states_its_instant_and_one_step() {
    // The rows differ in what the log has to REPORT: the integration performed, the payloads
    // that cross the transformation points (none, one line, several lines, a line-terminated
    // document, more lines than the log states), and whether the call was performed at all.
    let rows: &[Row] = &[
        Row {
            case: "a mock integration answering one line",
            path_part: "one-line",
            verb: "GET",
            integration: json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
            }),
            integration_response: Some(json!({
                "responseTemplates": { "application/json": "{\"status\":\"ACTIVE\"}" },
            })),
            request: json!({ "pathWithQueryString": "/one-line" }),
        },
        Row {
            case: "a mock integration answering several lines",
            path_part: "many-lines",
            verb: "GET",
            integration: json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
            }),
            integration_response: Some(json!({
                "responseTemplates": {
                    "application/json": "{\n  \"status\" : \"ACTIVE\",\n  \"lines\" : 2\n}",
                },
            })),
            request: json!({ "pathWithQueryString": "/many-lines?view=full" }),
        },
        Row {
            case: "a request body of several lines",
            path_part: "posted",
            verb: "POST",
            integration: json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
            }),
            integration_response: Some(json!({
                "responseTemplates": { "application/json": "{\"stored\":true}" },
            })),
            request: json!({
                "pathWithQueryString": "/posted",
                "headers": { "Content-Type": "application/json" },
                "body": "{\n  \"name\" : \"analytics\",\n  \"terms\" : [\n    \"a\"\n  ]\n}",
            }),
        },
        Row {
            case: "a mock integration answering a line-terminated document",
            path_part: "terminated",
            verb: "GET",
            integration: json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
            }),
            integration_response: Some(json!({
                "responseTemplates": {
                    "application/json": "{\n  \"status\" : \"ACTIVE\"\n}\n",
                },
            })),
            request: json!({ "pathWithQueryString": "/terminated" }),
        },
        Row {
            case: "a request body of more lines than the log states",
            path_part: "many",
            verb: "POST",
            integration: json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
            }),
            integration_response: Some(json!({
                "responseTemplates": { "application/json": "{\"stored\":true}" },
            })),
            request: json!({
                "pathWithQueryString": "/many",
                "headers": { "Content-Type": "application/json" },
                // One line per element: more lines than the log states, and inside the bound a
                // traced payload is stated under, so the case drives the log's own line bound
                // rather than the payload's.
                "body": format!("[{}\n]", "\n \"x\",".repeat(600)),
            }),
        },
        Row {
            case: "a mock integration with no response template at all",
            path_part: "no-template",
            verb: "DELETE",
            integration: json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
            }),
            integration_response: None,
            request: json!({ "pathWithQueryString": "/no-template" }),
        },
        Row {
            case: "an integration this api cannot perform",
            path_part: "unperformable",
            verb: "PUT",
            integration: json!({
                "type": "HTTP",
                "httpMethod": "PUT",
                "uri": "https://backend.invalid/agreements",
            }),
            integration_response: None,
            request: json!({ "pathWithQueryString": "/unperformable", "body": "{}" }),
        },
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "test-invoke-execution-log").await;

    let mut named: Vec<String> = Vec::new();
    for row in rows {
        let resource = resource_under(&gateway, &api, &root, row.path_part).await;
        method_with(
            &gateway,
            &api,
            &resource,
            row.verb,
            row.integration.clone(),
            row.integration_response.clone(),
        )
        .await;
        let reported = simulate(&gateway, &api, &resource, row.verb, row.request.clone()).await;
        let log = reported_log(&reported, row.case);
        named.push(request_id_of(&log, row.case));
        // The one row whose step text is longer than the log states says so, in a line of the
        // same shape as every other: a bounded log is still a log a client can read.
        if row.case == "a request body of more lines than the log states" {
            assert!(
                log.contains(BOUND_REACHED),
                "case {}: expected the log to state that it is bounded, got {log:?}",
                row.case
            );
        }
    }

    // One log per invocation: the id a log names is the id of the call it is the log of, so
    // no two of these simulations are correlated with one another.
    for (index, id) in named.iter().enumerate() {
        assert!(
            named.iter().skip(index + 1).all(|other| other != id),
            "case {}: expected a request id of its own per invocation, got {named:?}",
            rows[index].case
        );
    }
}

#[tokio::test]
async fn a_simulated_invocations_log_states_the_requests_parameters_and_the_steps_in_order() {
    // Each row is a simulated request and the two parameter maps the log must state for it:
    // the path parameters the resource's template binds out of the address, and the query
    // parameters its query string carries — the EMPTY map where the request carries none of a
    // kind, so a reader tells "nothing was carried" from a line it can read.
    let rows: &[(&str, &str, &str, Value, &str, &str)] = &[
        (
            "a path parameter and a query parameter",
            "agreements",
            "{agreementId}",
            json!({ "pathWithQueryString": "/agreements/dua-001?view=full" }),
            "{agreementId=dua-001}",
            "{view=full}",
        ),
        (
            "a path parameter and two query parameters",
            "plans",
            "{planId}",
            json!({ "pathWithQueryString": "/plans/basic?view=full&page=2" }),
            "{planId=basic}",
            "{page=2, view=full}",
        ),
        (
            "neither a path parameter nor a query string",
            "health",
            "status",
            json!({ "pathWithQueryString": "/health/status" }),
            "{}",
            "{}",
        ),
        (
            "a path parameter and no query string",
            "owners",
            "{ownerId}",
            json!({ "pathWithQueryString": "/owners/ada" }),
            "{ownerId=ada}",
            "{}",
        ),
    ];

    for (case, parent_part, child_part, request, path_map, query_map) in rows {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, "test-invoke-log-parameters").await;
        let parent = resource_under(&gateway, &api, &root, parent_part).await;
        let resource = resource_under(&gateway, &api, &parent, child_part).await;
        method_with(
            &gateway,
            &api,
            &resource,
            "GET",
            json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
            }),
            Some(json!({
                "responseTemplates": { "application/json": "{\"status\":\"ACTIVE\"}" },
            })),
        )
        .await;

        let reported = simulate(&gateway, &api, &resource, "GET", request.clone()).await;
        let log = reported_log(&reported, case);
        request_id_of(&log, case);
        let steps: Vec<&str> = log
            .split('\n')
            .skip(1)
            .filter_map(|line| line.split_once(SEPARATOR).map(|(_, step)| step))
            .collect();
        for expected in [
            format!("Method request path: {path_map}"),
            format!("Method request query string: {query_map}"),
        ] {
            assert!(
                steps.contains(&expected.as_str()),
                "case {case}: expected the step {expected:?}, got {steps:?}"
            );
        }
        // The steps an execution log states about the method RESPONSE, in the order it states
        // them: the body it answered, the headers it answered with, how the execution ended
        // and the status it completed with.
        let at = |step: &str| {
            steps
                .iter()
                .position(|stated| stated.starts_with(step))
                .unwrap_or_else(|| panic!("case {case}: expected a {step:?} step, got {steps:?}"))
        };
        let body = at("Method response body after transformations:");
        let response_headers = at("Method response headers:");
        let completed = at("Successfully completed execution");
        let status = at("Method completed with status:");
        assert!(
            body < response_headers && response_headers < completed && completed < status,
            "case {case}: expected the response body, the response headers, the completed \
             execution and the status in that order, got {steps:?}"
        );
    }
}

#[tokio::test]
async fn a_second_simulation_of_one_method_is_its_own_logged_invocation() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "test-invoke-log-per-call").await;
    let resource = resource_under(&gateway, &api, &root, "agreements").await;
    method_with(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({
            "type": "MOCK",
            "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
        }),
        Some(json!({
            "responseTemplates": { "application/json": "{\"status\":\"ACTIVE\"}" },
        })),
    )
    .await;

    let request = json!({ "pathWithQueryString": "/agreements" });
    let first = simulate(&gateway, &api, &resource, "GET", request.clone()).await;
    let second = simulate(&gateway, &api, &resource, "GET", request).await;
    let first = request_id_of(&reported_log(&first, "the first simulation"), "the first simulation");
    let second = request_id_of(
        &reported_log(&second, "the second simulation"),
        "the second simulation",
    );
    assert_ne!(
        first, second,
        "case two simulations of one method: expected each invocation to be named by its own \
         request id, got {first} twice"
    );
}

#[tokio::test]
async fn a_simulated_authorizer_execution_is_logged_the_same_way() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "test-invoke-authorizer-log").await;
    let created = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/authorizers"),
            Some(json!({
                "name": "tokenAuthorizer",
                "type": "TOKEN",
                "identitySource": "method.request.header.Authorization",
                "authorizerUri": "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/\
                                  arn:aws:lambda:us-east-1:000000000000:function:authorize/invocations",
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case CreateAuthorizer: expected 201, got {} with {:?}",
        created.status, created.body
    );
    let authorizer = created
        .body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the created authorizer carries an id: {:?}", created.body))
        .to_string();

    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/authorizers/{authorizer}"),
            Some(json!({ "headers": { "Authorization": "Bearer token" } })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case TestInvokeAuthorizer: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    request_id_of(
        &reported_log(&answer.body, "a simulated authorizer execution"),
        "a simulated authorizer execution",
    );
}
