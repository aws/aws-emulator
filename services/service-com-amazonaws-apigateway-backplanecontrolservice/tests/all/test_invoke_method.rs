// SPDX-License-Identifier: CC0-1.0

//! `TestInvokeMethod`: that a simulated invocation of a stored `Method` reports the
//! response THAT METHOD's own records produce for the simulated request.
//!
//! The rule these cases assert, over sets of inputs rather than one recorded call: a
//! simulated invocation resolves the method from the API's live records and reports the
//! response its integration produces for the simulated request — the status, the body and
//! the headers that integration's own records render, the steps in an execution log, and
//! the latency the simulation took — answering a call whose integration cannot be performed
//! through the API's gateway response for it, and never with a backend answer it did not
//! obtain. A method that carries NO integration is refused on the operation's own error
//! channel instead, which the `test_invoke_method_no_integration` suite asserts.
//!
//! The rows differ along what that rule generalises over: the content type the simulated
//! request declares (which selects the integration request template, and so the status),
//! the media type it accepts (which selects the response template, and so the body), which
//! member the header travels in, the integration type, the verb the method was put under,
//! and which identifier of the three is unresolvable.
//!
//! Every case drives the real wire path — a `restJson1` request through the multi-service
//! router into the mounted service — through the crate's one wire-test harness
//! ([`Gateway`]), so what is asserted is what a client sees.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{create_api, resource_under, Gateway};

/// The region the integration ARNs of these cases name; one is enough, because no row
/// turns on it.
const REGION: &str = "us-east-1";

/// The body every uncustomized `API_CONFIGURATION_ERROR` gateway response carries, which
/// is what a call whose integration cannot be performed is answered with.
const INTERNAL_SERVER_ERROR: &str = "Internal server error";

/// The `GatewayResponseType` a call whose integration cannot be performed is answered
/// through — the type a developer's `PutGatewayResponse` customization of it applies to.
const API_CONFIGURATION_ERROR: &str = "API_CONFIGURATION_ERROR";

/// One simulated invocation, as the operation reports it.
///
/// `pub(crate)` with [`test_invoke`], so the sibling
/// `test_invoke_method_data_plane_parity` suite reads a simulated response through this
/// one reader rather than a second one of its own.
pub(crate) struct Simulated {
    /// The HTTP status of the `TestInvokeMethod` call itself — 200 for a performed
    /// simulation, whatever the SIMULATED status is.
    pub(crate) http_status: u16,
    /// The `x-amzn-errortype` of a refused call.
    error: Option<String>,
    /// The whole response body, so a case can assert on a member this struct does not
    /// name.
    pub(crate) body: Value,
}

impl Simulated {
    /// The simulated response's status — the `status` member.
    pub(crate) fn status(&self) -> i64 {
        self.body
            .get("status")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("the response carries a numeric status: {:?}", self.body))
    }

    /// The simulated response's body — the `body` member.
    pub(crate) fn simulated_body(&self) -> String {
        self.body
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The simulated response's headers — the `headers` member.
    pub(crate) fn headers(&self) -> Value {
        self.body.get("headers").cloned().unwrap_or(Value::Null)
    }

    /// The simulated response's multi-valued headers — the `multiValueHeaders` member.
    fn multi_value_headers(&self) -> Value {
        self.body
            .get("multiValueHeaders")
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// The execution log — the `log` member.
    fn log(&self) -> String {
        self.body
            .get("log")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The measured latency — the `latency` member.
    fn latency(&self) -> i64 {
        self.body
            .get("latency")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("the response carries a numeric latency: {:?}", self.body))
    }

    /// The `message` of a refused call.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// `TestInvokeMethod` for `verb` on `resource`, with `request` as the simulated request.
///
/// `pub(crate)` for the sibling `test_invoke_method_data_plane_parity` suite; see
/// [`Simulated`].
pub(crate) async fn test_invoke(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    request: Value,
) -> Simulated {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
            Some(request),
        )
        .await;
    Simulated {
        http_status: answer.status,
        error: answer.error,
        body: answer.body,
    }
}

/// `PutMethod` with open authorization on a resource.
async fn put_method(gateway: &Gateway, api: &str, resource: &str, verb: &str) {
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
            Some(json!({ "authorizationType": "NONE" })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutMethod({verb}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `PutIntegration` on a method.
async fn put_integration(gateway: &Gateway, api: &str, resource: &str, verb: &str, request: Value) {
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
            Some(request.clone()),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutIntegration({request}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `PutIntegrationResponse` for one status code of a method's integration.
async fn put_integration_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    request: Value,
) {
    let answer = gateway
        .call(
            "PUT",
            &format!(
                "/restapis/{api}/resources/{resource}/methods/{verb}/integration/responses/{status}"
            ),
            Some(request),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutIntegrationResponse({status}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `PutMethodResponse` for one status code of a method.
async fn put_method_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    request: Value,
) {
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}/responses/{status}"),
            Some(request),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutMethodResponse({status}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The `uri` an `AWS`/`AWS_PROXY` integration naming a Lambda function carries, in the form
/// `Integration$uri` documents for it.
///
/// `pub(crate)` for the sibling `request_validator_precedes_the_integration` suite, which
/// points a validated method at a Lambda backend: one spelling of that `uri` across the
/// suites that name one.
pub(crate) fn lambda_uri() -> String {
    format!(
        "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
         arn:aws:lambda:{REGION}:123456789012:function:handler/invocations"
    )
}

/// A `MOCK` method on a fresh resource, with two request templates naming two different
/// statuses, two integration responses, and a mapped response header the method response
/// declares — the configuration the first table simulates requests against.
async fn mock_method(gateway: &Gateway, api: &str, root: &str, path_part: &str) -> String {
    let resource = resource_under(gateway, api, root, path_part).await;
    put_method(gateway, api, &resource, "GET").await;
    put_integration(
        gateway,
        api,
        &resource,
        "GET",
        json!({
            "type": "MOCK",
            "requestTemplates": {
                "application/json": "{\"statusCode\": 200}",
                "application/xml": "{\"statusCode\": 404}"
            }
        }),
    )
    .await;
    put_method_response(
        gateway,
        api,
        &resource,
        "GET",
        "200",
        json!({ "responseParameters": { "method.response.header.X-Pets": true } }),
    )
    .await;
    put_method_response(gateway, api, &resource, "GET", "404", json!({})).await;
    put_integration_response(
        gateway,
        api,
        &resource,
        "GET",
        "200",
        json!({
            "responseTemplates": {
                "application/json": "{\"pets\":[]}",
                "text/plain": "no pets"
            },
            "responseParameters": { "method.response.header.X-Pets": "'none'" }
        }),
    )
    .await;
    put_integration_response(
        gateway,
        api,
        &resource,
        "GET",
        "404",
        json!({ "responseTemplates": { "application/json": "{\"message\":\"no such pet\"}" } }),
    )
    .await;
    resource
}

#[tokio::test]
async fn the_simulated_response_is_the_one_the_methods_own_records_render() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "test-invoke-mock").await;
    let resource = mock_method(&gateway, &api, &root, "pets").await;

    // Rows differ along the simulated request: which content type it declares (and so
    // which request template, and so which integration response), whether that type
    // carries a parameter, which case the header name is spelled in, which member the
    // header travels in, what it accepts, and whether it declares anything at all.
    let cases: Vec<(&str, Value, i64, &str, Value)> = vec![
        (
            "the json content type selects the template naming 200",
            json!({ "headers": { "Content-Type": "application/json" } }),
            200,
            "{\"pets\":[]}",
            json!({ "Content-Type": "application/json", "X-Pets": "none" }),
        ),
        (
            "the xml content type selects the template naming 404",
            json!({ "headers": { "Content-Type": "application/xml" } }),
            404,
            "{\"message\":\"no such pet\"}",
            json!({ "Content-Type": "application/json" }),
        ),
        (
            "a charset parameter is dropped before the template is selected",
            json!({ "headers": { "Content-Type": "application/json; charset=utf-8" } }),
            200,
            "{\"pets\":[]}",
            json!({ "Content-Type": "application/json", "X-Pets": "none" }),
        ),
        (
            "the header name is read case-insensitively",
            json!({ "headers": { "content-type": "application/xml" } }),
            404,
            "{\"message\":\"no such pet\"}",
            json!({ "Content-Type": "application/json" }),
        ),
        (
            "a request declaring no content type is read as the json one",
            json!({}),
            200,
            "{\"pets\":[]}",
            json!({ "Content-Type": "application/json", "X-Pets": "none" }),
        ),
        (
            "an accepted text/plain selects that response template",
            json!({
                "headers": { "Content-Type": "application/json", "Accept": "text/plain" }
            }),
            200,
            "no pets",
            // The accepted template decides the BODY; with no `Content-Type` mapping
            // declared, the media type reported stays `application/json`.
            json!({ "Content-Type": "application/json", "X-Pets": "none" }),
        ),
        (
            "a content type no template maps is passed through to the default response",
            json!({ "headers": { "Content-Type": "text/csv" } }),
            200,
            "{\"pets\":[]}",
            json!({ "Content-Type": "application/json", "X-Pets": "none" }),
        ),
        (
            "the content type travels in multiValueHeaders instead",
            json!({ "multiValueHeaders": { "Content-Type": ["application/xml"] } }),
            404,
            "{\"message\":\"no such pet\"}",
            json!({ "Content-Type": "application/json" }),
        ),
        (
            "a simulated body, path and stage variables do not change what a MOCK answers",
            json!({
                "headers": { "Content-Type": "application/json" },
                "body": "{\"name\":\"pepper\"}",
                "pathWithQueryString": "/pets?breed=corgi",
                "stageVariables": { "table": "pets" },
                "clientCertificateId": "abc123"
            }),
            200,
            "{\"pets\":[]}",
            json!({ "Content-Type": "application/json", "X-Pets": "none" }),
        ),
    ];

    for (case, request, status, body, headers) in &cases {
        let simulated = test_invoke(&gateway, &api, &resource, "GET", request.clone()).await;
        assert_eq!(
            simulated.http_status, 200,
            "case {case:?}: expected the operation to answer 200, got {} with {:?}",
            simulated.http_status, simulated.body
        );
        assert_eq!(
            simulated.status(),
            *status,
            "case {case:?}: expected simulated status {status}, got {:?}",
            simulated.body
        );
        assert_eq!(
            simulated.simulated_body(),
            *body,
            "case {case:?}: expected simulated body {body:?}, got {:?}",
            simulated.simulated_body()
        );
        assert_eq!(
            simulated.headers(),
            *headers,
            "case {case:?}: expected headers {headers}, got {:?}",
            simulated.headers()
        );
        // The multi-valued view of the same headers: each name mapped to the
        // single-element list of its value.
        let expected_multi: Value = Value::Object(
            headers
                .as_object()
                .expect("the expected headers are an object")
                .iter()
                .map(|(name, value)| (name.clone(), json!([value])))
                .collect(),
        );
        assert_eq!(
            simulated.multi_value_headers(),
            expected_multi,
            "case {case:?}: expected multiValueHeaders {expected_multi}, got {:?}",
            simulated.multi_value_headers()
        );
        let log = simulated.log();
        assert!(
            log.starts_with("Execution log for request "),
            "case {case:?}: expected the log to open with the request it is for, got {log:?}"
        );
        assert!(
            log.contains(&format!("Method completed with status: {status}")),
            "case {case:?}: expected the log to report the status it completed with, got {log:?}"
        );
        assert!(
            log.contains("Integration type: MOCK"),
            "case {case:?}: expected the log to name the integration performed, got {log:?}"
        );
        let latency = simulated.latency();
        assert!(
            (0..60_000).contains(&latency),
            "case {case:?}: expected a measured latency in ms, got {latency:?}"
        );
    }
}

/// One configuration a simulated invocation cannot be performed through, and what the
/// operation reports for it. A struct rather than a tuple: the row carries eight fields,
/// and a reader of the assertions should not have to count positions.
struct Unservable {
    /// What makes this row its own case.
    case: &'static str,
    /// The path part of the resource this row stands its method up on.
    path_part: &'static str,
    /// The `PutIntegration` request this row's method carries. Every row has one: a
    /// method with NO integration is not an unperformable configuration but a refused
    /// call, which the `test_invoke_method_no_integration` suite asserts.
    integration: Value,
    /// The `PutIntegrationResponse` request for status `200`, when the row needs one.
    integration_response: Option<Value>,
    /// The simulated request.
    request: Value,
    /// The status the simulated response must carry.
    status: i64,
    /// The `message` the gateway response body must carry.
    message: &'static str,
    /// The `GatewayResponseType` the execution log must name.
    response_type: &'static str,
}

#[tokio::test]
async fn an_integration_that_cannot_be_performed_is_reported_through_the_gateway_response() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "test-invoke-unservable").await;

    // Rows differ along the configuration that cannot be performed: each integration type
    // that names a backend, a MOCK whose only integration response is patterned (so no
    // response is selected), a MOCK whose response template is Velocity rather than a
    // literal body, and a MOCK that refuses an unmapped content type outright.
    let cases: Vec<Unservable> = vec![
        Unservable {
            case: "a Lambda proxy integration",
            path_part: "proxy",
            integration: json!({
                "type": "AWS_PROXY", "httpMethod": "POST", "uri": lambda_uri()
            }),
            integration_response: None,
            request: json!({}),
            status: 500,
            message: INTERNAL_SERVER_ERROR,
            response_type: API_CONFIGURATION_ERROR,
        },
        Unservable {
            case: "an AWS service action integration",
            path_part: "ddb",
            integration: json!({
                "type": "AWS",
                "httpMethod": "POST",
                "uri": format!("arn:aws:apigateway:{REGION}:dynamodb:action/PutItem")
            }),
            integration_response: None,
            request: json!({}),
            status: 500,
            message: INTERNAL_SERVER_ERROR,
            response_type: API_CONFIGURATION_ERROR,
        },
        Unservable {
            case: "an HTTP proxy integration",
            path_part: "upstream-proxy",
            integration: json!({
                "type": "HTTP_PROXY",
                "httpMethod": "GET",
                "uri": "https://example.com/upstream"
            }),
            integration_response: None,
            request: json!({}),
            status: 500,
            message: INTERNAL_SERVER_ERROR,
            response_type: API_CONFIGURATION_ERROR,
        },
        Unservable {
            case: "an HTTP integration",
            path_part: "upstream",
            integration: json!({
                "type": "HTTP",
                "httpMethod": "GET",
                "uri": "https://example.com/upstream"
            }),
            integration_response: None,
            request: json!({}),
            status: 500,
            message: INTERNAL_SERVER_ERROR,
            response_type: API_CONFIGURATION_ERROR,
        },
        Unservable {
            case: "a MOCK whose only integration response is patterned",
            path_part: "patterned",
            integration: json!({ "type": "MOCK" }),
            integration_response: Some(json!({ "selectionPattern": "4\\d\\d" })),
            request: json!({}),
            status: 500,
            message: INTERNAL_SERVER_ERROR,
            response_type: API_CONFIGURATION_ERROR,
        },
        Unservable {
            case: "a MOCK whose response template is a Velocity template",
            path_part: "velocity",
            integration: json!({ "type": "MOCK" }),
            integration_response: Some(json!({
                "responseTemplates": {
                    "application/json": "#set($root = $input.path('$'))\n{\"id\":\"$root.id\"}"
                }
            })),
            request: json!({}),
            status: 500,
            message: INTERNAL_SERVER_ERROR,
            response_type: API_CONFIGURATION_ERROR,
        },
        Unservable {
            case: "a MOCK that passes no unmapped content type through",
            path_part: "strict",
            integration: json!({
                "type": "MOCK",
                "passthroughBehavior": "NEVER",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" }
            }),
            integration_response: Some(json!({})),
            request: json!({ "headers": { "Content-Type": "text/csv" } }),
            status: 415,
            message: "Unsupported Media Type",
            response_type: "UNSUPPORTED_MEDIA_TYPE",
        },
    ];

    for row in &cases {
        let case = row.case;
        let status = row.status;
        let resource = resource_under(&gateway, &api, &root, row.path_part).await;
        put_method(&gateway, &api, &resource, "GET").await;
        put_integration(&gateway, &api, &resource, "GET", row.integration.clone()).await;
        if let Some(integration_response) = &row.integration_response {
            put_integration_response(
                &gateway,
                &api,
                &resource,
                "GET",
                "200",
                integration_response.clone(),
            )
            .await;
        }
        let simulated = test_invoke(&gateway, &api, &resource, "GET", row.request.clone()).await;
        assert_eq!(
            simulated.http_status, 200,
            "case {case:?}: expected the operation to answer 200, got {} with {:?}",
            simulated.http_status, simulated.body
        );
        assert_eq!(
            simulated.status(),
            status,
            "case {case:?}: expected simulated status {status}, got {:?}",
            simulated.body
        );
        assert_eq!(
            simulated.simulated_body(),
            json!({ "message": row.message }).to_string(),
            "case {case:?}: expected the gateway response body for {:?}, got {:?}",
            row.message,
            simulated.simulated_body()
        );
        let log = simulated.log();
        assert!(
            log.contains("Execution failed due to configuration error:"),
            "case {case:?}: expected the log to state the execution failed, got {log:?}"
        );
        assert!(
            log.contains(row.response_type),
            "case {case:?}: expected the log to name the {} gateway response, got {log:?}",
            row.response_type
        );
        assert!(
            log.contains(&format!("Method completed with status: {status}")),
            "case {case:?}: expected the log to report the status it completed with, got {log:?}"
        );
    }
}

#[tokio::test]
async fn a_customized_gateway_response_is_what_the_simulated_call_reports() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "test-invoke-customized").await;
    let resource = resource_under(&gateway, &api, &root, "proxy").await;
    put_method(&gateway, &api, &resource, "GET").await;
    put_integration(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({ "type": "AWS_PROXY", "httpMethod": "POST", "uri": lambda_uri() }),
    )
    .await;

    // Rows differ along the customization the developer put on the response type the
    // refusal is answered through: the status alone, the status and a literal template,
    // and the whole 5xx class rather than the one type.
    let cases: Vec<(&str, &str, Value, i64, &str)> = vec![
        (
            "no customization at all",
            "",
            json!({}),
            500,
            "{\"message\":\"Internal server error\"}",
        ),
        (
            "the response type's status customized",
            API_CONFIGURATION_ERROR,
            json!({ "statusCode": "502" }),
            502,
            "{\"message\":\"Internal server error\"}",
        ),
        (
            "the response type's status and body customized",
            API_CONFIGURATION_ERROR,
            json!({
                "statusCode": "503",
                "responseTemplates": { "application/json": "{\"fault\":\"backend\"}" }
            }),
            503,
            "{\"fault\":\"backend\"}",
        ),
        (
            "the whole 5xx class customized",
            "DEFAULT_5XX",
            json!({
                "statusCode": "504",
                "responseTemplates": { "application/json": "{\"fault\":\"class\"}" }
            }),
            504,
            "{\"fault\":\"class\"}",
        ),
    ];

    for (case, response_type, customization, status, body) in &cases {
        if !response_type.is_empty() {
            // A later row's customization replaces the earlier one for the same type, so
            // the rows are independent of each other: the API_CONFIGURATION_ERROR rows
            // overwrite, and the class row is read only because no specific one applies
            // after it is deleted.
            let answer = gateway
                .call(
                    "PUT",
                    &format!("/restapis/{api}/gatewayresponses/{response_type}"),
                    Some(customization.clone()),
                )
                .await;
            assert_eq!(
                answer.status, 201,
                "case {case:?}: expected PutGatewayResponse to answer 201, got {} with {:?}",
                answer.status, answer.body
            );
            if *response_type == "DEFAULT_5XX" {
                let deleted = gateway
                    .call(
                        "DELETE",
                        &format!("/restapis/{api}/gatewayresponses/{API_CONFIGURATION_ERROR}"),
                        None,
                    )
                    .await;
                assert_eq!(
                    deleted.status, 202,
                    "case {case:?}: expected DeleteGatewayResponse to answer 202, got {} with {:?}",
                    deleted.status, deleted.body
                );
            }
        }
        let simulated = test_invoke(&gateway, &api, &resource, "GET", json!({})).await;
        assert_eq!(
            simulated.status(),
            *status,
            "case {case:?}: expected simulated status {status}, got {:?}",
            simulated.body
        );
        assert_eq!(
            simulated.simulated_body(),
            *body,
            "case {case:?}: expected simulated body {body:?}, got {:?}",
            simulated.simulated_body()
        );
    }
}

#[tokio::test]
async fn the_method_is_resolved_by_the_verb_it_was_put_under() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "test-invoke-verbs").await;
    let resource = resource_under(&gateway, &api, &root, "items").await;

    // One method per verb, each answering its OWN status, so a simulated invocation that
    // resolved the wrong method would report another row's status. The verbs are the
    // catalog `crate::ops::put_method::HTTP_METHODS` lists for a method, the `ANY`
    // catch-all among them.
    let verbs = ["ANY", "DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT"];
    for (index, verb) in verbs.iter().enumerate() {
        let status = format!("2{:02}", index);
        put_method(&gateway, &api, &resource, verb).await;
        put_integration(
            &gateway,
            &api,
            &resource,
            verb,
            json!({
                "type": "MOCK",
                "requestTemplates": {
                    "application/json": format!("{{\"statusCode\": {status}}}")
                }
            }),
        )
        .await;
        put_integration_response(
            &gateway,
            &api,
            &resource,
            verb,
            &status,
            json!({ "responseTemplates": { "application/json": format!("{{\"verb\":\"{verb}\"}}") } }),
        )
        .await;
    }

    for (index, verb) in verbs.iter().enumerate() {
        let expected_status = 200 + i64::try_from(index).expect("the index fits an i64");
        // A method is resolved by the verb it was PUT under, which is the identifier the
        // request names: a lowercase spelling names no method, so the call is refused
        // rather than simulated against the verb's own method.
        let lowercase = verb.to_lowercase();
        let unresolved = test_invoke(&gateway, &api, &resource, &lowercase, json!({})).await;
        assert_eq!(
            unresolved.http_status, 404,
            "case {lowercase:?}: expected the modelled 404, got {} with {:?}",
            unresolved.http_status, unresolved.body
        );
        {
            let spelling = (*verb).to_string();
            let simulated = test_invoke(&gateway, &api, &resource, &spelling, json!({})).await;
            assert_eq!(
                simulated.status(),
                expected_status,
                "case {spelling:?}: expected simulated status {expected_status}, got {:?}",
                simulated.body
            );
            assert_eq!(
                simulated.simulated_body(),
                json!({ "verb": verb }).to_string(),
                "case {spelling:?}: expected the body of the method put under {verb}, got {:?}",
                simulated.simulated_body()
            );
            assert!(
                simulated
                    .log()
                    .contains(&format!("HTTP Method: {verb}, Resource Path: /items")),
                "case {spelling:?}: expected the log to name the method and path it ran, got {:?}",
                simulated.log()
            );
        }
    }
}

#[tokio::test]
async fn an_identifier_no_record_carries_is_answered_not_found() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "test-invoke-not-found").await;
    let resource = resource_under(&gateway, &api, &root, "pets").await;
    put_method(&gateway, &api, &resource, "GET").await;
    put_integration(&gateway, &api, &resource, "GET", json!({ "type": "MOCK" })).await;

    // Rows differ along WHICH of the three identifiers is unresolvable, each worded as
    // every other reader of the same records words it.
    let cases: Vec<(&str, String, String, &str, String)> = vec![
        (
            "an api id no API carries",
            "ghostapi".to_string(),
            resource.clone(),
            "GET",
            format!(
                "Invalid API identifier specified {}:ghostapi",
                engine::emulator_account_id()
            ),
        ),
        (
            "a resource id no resource of the api carries",
            api.clone(),
            "ghostres".to_string(),
            "GET",
            "Invalid Resource identifier specified".to_string(),
        ),
        (
            "a verb no method of the resource was put under",
            api.clone(),
            resource.clone(),
            "DELETE",
            "Invalid Method identifier specified".to_string(),
        ),
    ];

    for (case, rest_api, resource_id, verb, message) in &cases {
        let simulated = test_invoke(&gateway, rest_api, resource_id, verb, json!({})).await;
        assert_eq!(
            simulated.http_status, 404,
            "case {case:?}: expected 404, got {} with {:?}",
            simulated.http_status, simulated.body
        );
        assert_eq!(
            simulated.error.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {:?}",
            simulated.error
        );
        assert_eq!(
            simulated.message(),
            *message,
            "case {case:?}: expected the message {message:?}, got {:?}",
            simulated.message()
        );
    }
}

#[tokio::test]
async fn a_simulated_invocation_changes_nothing_a_later_caller_sees() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "test-invoke-is-a-read").await;
    let resource = mock_method(&gateway, &api, &root, "pets").await;

    let method_before = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/resources/{resource}/methods/GET"),
            None,
        )
        .await;

    // The same simulation repeated, and a call the operation REFUSES in between: neither
    // may move a record the next reader sees.
    let first = test_invoke(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({ "headers": { "Content-Type": "application/xml" }, "body": "{}" }),
    )
    .await;
    let bare = resource_under(&gateway, &api, &root, "bare").await;
    put_method(&gateway, &api, &bare, "GET").await;
    let refused = test_invoke(&gateway, &api, &bare, "GET", json!({})).await;
    assert_eq!(
        refused.http_status, 400,
        "case the refused call: expected the operation to answer 400, got {} with {:?}",
        refused.http_status, refused.body
    );
    let second = test_invoke(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({ "headers": { "Content-Type": "application/xml" }, "body": "{}" }),
    )
    .await;

    assert_eq!(
        (second.status(), second.simulated_body(), second.headers()),
        (first.status(), first.simulated_body(), first.headers()),
        "case the repeated simulation: expected the same answer twice, got {:?} then {:?}",
        first.body,
        second.body
    );

    let method_after = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/resources/{resource}/methods/GET"),
            None,
        )
        .await;
    assert_eq!(
        method_after.body, method_before.body,
        "case GetMethod after the simulations: expected the method unchanged, got {:?} \
         instead of {:?}",
        method_after.body, method_before.body
    );
}
