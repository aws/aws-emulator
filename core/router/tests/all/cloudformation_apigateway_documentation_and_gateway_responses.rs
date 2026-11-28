// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack creates the Amazon API Gateway records that a client READS BACK off
//! an api — its documentation and the customizations of the responses API Gateway itself
//! returns — in API Gateway itself, and publishes for each of them the identifier that service
//! addresses it by.
//!
//! The rules under test, each over a TABLE of inputs rather than one recorded deployment:
//!
//! 1. **A resource whose owning service addresses it by MORE THAN ONE value is published under
//!    all of those values**, joined by the separator its own type publishes, so `Ref` — and the
//!    type's own `Fn::GetAtt` identifier attribute, where it has one — resolves to a value a
//!    client can call that service with: an `AWS::ApiGateway::GatewayResponse` by the api and
//!    the response type (`GetGatewayResponse` binds both as `@httpLabel`s), an
//!    `AWS::ApiGateway::DocumentationVersion` by the version and the api, and an
//!    `AWS::ApiGateway::UsagePlanKey` by the key and the plan (`GetUsagePlanKey` binds both).
//!    A resource a single SERVICE-ASSIGNED id names — a documentation part — keeps that id.
//! 2. **A declared record really exists in the service that owns it**: every one of them reads
//!    back through API Gateway's own read with the configuration the template declared, a
//!    gateway response reporting `defaultResponse: false` because a customization is the
//!    developer's and not the one that service generates — and the stack's delete takes each
//!    one away again, which is what makes the api answer its own generated response once more.
//! 3. **A property outside what its resource type's schema admits fails the operation BEFORE
//!    any provider is invoked**: the stack rolls back carrying CloudFormation's own summary over
//!    the stage's errors, the failure's own diagnosis is on a `VALIDATION_ERROR` operation event
//!    `DescribeEvents` reports at that member's path, and NOTHING of the template is created in
//!    API Gateway — while a value the schema admits is deployed and read back there.
//! 4. **A value only the OWNING service can judge is refused by that service**: a gateway
//!    response whose `StatusCode` is no HTTP status code fails the RESOURCE, the stack rolls
//!    back, and the api the declaration named keeps answering the response API Gateway
//!    generates.
//!
//! Two crates have to agree for any of it to hold (CloudFormation makes the calls, API Gateway
//! serves them and then the reads), so the suite lives here rather than in either of them.
//!
//! Nothing here waits out a span: every stack is polled until it rests in a terminal status or
//! a generous deadline passes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, members_of, opt_element, outputs_of};

/// The Region every request here is signed for, and so the Region the stack's resources belong
/// to.
const REGION: &str = "us-east-1";

/// The terminal `StackStatus`es the rows are stated about (the model's `StackStatus` enum).
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";
const DELETE_COMPLETE: &str = "DELETE_COMPLETE";

/// `ResourceStatus` of the stack-level event a failed create records, and the `EventType` a
/// validation result is reported under (the model's `EventType` enum).
const CREATE_FAILED: &str = "CREATE_FAILED";
const VALIDATION_ERROR: &str = "VALIDATION_ERROR";

/// The validation CloudFormation names for the check that holds a declaration's properties to
/// its type's schema, the `ValidationStatus` of one a declaration did not pass, and the
/// `ValidationFailureMode` of one whose failure was handled by FAILING the operation.
const PROPERTY_VALIDATION: &str = "PROPERTY_VALIDATION";
const FAILED: &str = "FAILED";
const FAIL: &str = "FAIL";

/// How long a wait for a stack to reach a terminal status may run before the suite calls it a
/// failure. Generous by design: what the rows are about is WHICH status the stack rests in, not
/// how fast the host reached it.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(120);

/// How long to wait before looking at the stack again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How many characters an identifier API Gateway mints carries, and the alphabet every minted
/// id draws from: ten lowercase alphanumerics.
const MINTED_ID_LEN: usize = 10;

/// The emulator as it is ASSEMBLED, on one engine: the resources CloudFormation creates are the
/// resources this suite then reads.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request and what each handler reads its Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one API Gateway control-plane operation over its restJson1 wire: that service signs as
/// `apigateway` and binds every REST operation under its own URI.
async fn apigateway(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| Bytes::from(value.to_string()))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("apigateway"))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// One API Gateway read, asserting it was served.
async fn read(router: &MultiServiceRouter, case: &str, uri: &str) -> Value {
    let (status, answered) = apigateway(router, "GET", uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GET {uri}, got {status} with {answered}"
    );
    answered
}

/// The stack as `DescribeStacks` answers it once its operation has reached a terminal status —
/// looked at again until it rests there or [`TERMINAL_DEADLINE`] passes, so what the suite
/// asserts never turns on how fast the host ran.
async fn settled_stack(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let deadline = Instant::now() + TERMINAL_DEADLINE;
    loop {
        let (code, described) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: DescribeStacks must answer 200, got {code} {described}"
        );
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        if !reported.is_empty() && !reported.ends_with("_IN_PROGRESS") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {stack} to rest in a terminal status within \
             {TERMINAL_DEADLINE:?}, last read {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Submit `template` as `stack` and answer the terminal status it settles in, with the
/// `DescribeStacks` answer it was read from.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &Value,
) -> (String, String) {
    let body = template.to_string();
    let (code, created) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &body)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack must answer 200 — the request is valid and the stack exists \
         — got {code} {created}"
    );
    let described = settled_stack(router, case, stack).await;
    let settled = opt_element(&described, "StackStatus").unwrap_or_default();
    (settled, described)
}

/// What a stack reports for a failure of any of CloudFormation's pre-deployment validations:
/// the count of errors the stage found and where their detail is read. Source:
/// `state::early_validation::validation_failure_summary`.
fn summary(errors: usize) -> String {
    format!(
        "Validation failed with {errors} error(s). Call DescribeEvents to retrieve the full \
         list of issues with resource and property details, resolve each error, then retry \
         the operation."
    )
}

/// The `ResourceStatusReason` of every stack-level `CREATE_FAILED` event of `stack`.
async fn stack_failure_reasons(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> Vec<String> {
    let (code, body) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: the stack's events must be readable, got {code} {body}"
    );
    members_of(&body)
        .into_iter()
        .filter(|event| {
            opt_element(event, "LogicalResourceId").as_deref() == Some(stack)
                && opt_element(event, "ResourceStatus").as_deref() == Some(CREATE_FAILED)
        })
        .filter_map(|event| opt_element(event, "ResourceStatusReason"))
        .collect()
}

/// The validation RESULTS `DescribeEvents` reports for `stack`: one tuple per operation event
/// carrying a validation, as a client reads them.
async fn validation_results(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> Vec<(String, String, String, String, String, String)> {
    let (code, body) = cloudformation(router, "DescribeEvents", &[("StackName", stack)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: the operation's events must be readable, got {code} {body}"
    );
    members_of(&body)
        .into_iter()
        .filter(|event| opt_element(event, "ValidationName").is_some())
        .map(|event| {
            let read = |name: &str| opt_element(event, name).unwrap_or_default();
            (
                read("LogicalResourceId"),
                read("EventType"),
                read("ValidationName"),
                read("ValidationStatus"),
                read("ValidationFailureMode"),
                read("ValidationPath"),
            )
        })
        .collect()
}

/// Whether `id` is of the shape API Gateway MINTS an identifier in.
fn is_minted_id(id: &str) -> bool {
    id.chars().count() == MINTED_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

// ---------------------------------------------------------------------------
// Rule 1 and rule 2: the records a stack really creates, and what it publishes for them
// ---------------------------------------------------------------------------

/// The response types the first table customizes: a CLASS-WIDE default, which API Gateway
/// generates with no status code of its own, and ONE condition's response, which has one — so
/// the rows span both halves of `GatewayResponseType`
/// (`smithy_query(query="shape", shape="GatewayResponseType")`).
const CLASS_DEFAULT: &str = "DEFAULT_4XX";
const ONE_CONDITION: &str = "MISSING_AUTHENTICATION_TOKEN";

/// The documentation version the first table snapshots the api's parts under.
const VERSION: &str = "1.0";

/// The declarations the round-trip stack deploys: an api with one method, two gateway response
/// customizations, two documentation parts and the version that snapshots them.
fn round_trip_template(name: &str) -> Value {
    json!({
        "Resources": {
            "Api": {
                "Type": "AWS::ApiGateway::RestApi",
                "Properties": {
                    "Name": name,
                    "EndpointConfiguration": { "Types": ["REGIONAL"] }
                }
            },
            "Items": {
                "Type": "AWS::ApiGateway::Resource",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "ParentId": { "Fn::GetAtt": ["Api", "RootResourceId"] },
                    "PathPart": "items"
                }
            },
            "Method": {
                "Type": "AWS::ApiGateway::Method",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "ResourceId": { "Ref": "Items" },
                    "HttpMethod": "GET",
                    "AuthorizationType": "NONE",
                    "Integration": { "Type": "MOCK" }
                }
            },
            "ClassResponse": {
                "Type": "AWS::ApiGateway::GatewayResponse",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "ResponseType": CLASS_DEFAULT,
                    "StatusCode": "444",
                    "ResponseParameters": {
                        "gatewayresponse.header.Access-Control-Allow-Origin": "'*'"
                    },
                    "ResponseTemplates": {
                        "application/json": "{\"message\":\"refused\"}"
                    }
                }
            },
            "ConditionResponse": {
                "Type": "AWS::ApiGateway::GatewayResponse",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "ResponseType": ONE_CONDITION,
                    "StatusCode": "401"
                }
            },
            "ApiDocs": {
                "Type": "AWS::ApiGateway::DocumentationPart",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "Location": { "Type": "API" },
                    "Properties": "{\"info\":{\"description\":\"the items api\"}}"
                }
            },
            "MethodDocs": {
                "Type": "AWS::ApiGateway::DocumentationPart",
                "DependsOn": ["Method"],
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "Location": { "Type": "METHOD", "Path": "/items", "Method": "GET" },
                    "Properties": "{\"summary\":\"list the items\"}"
                }
            },
            "Docs": {
                "Type": "AWS::ApiGateway::DocumentationVersion",
                "DependsOn": ["ApiDocs", "MethodDocs"],
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "DocumentationVersion": VERSION,
                    "Description": "the first snapshot"
                }
            }
        },
        "Outputs": {
            "ApiId": { "Value": { "Ref": "Api" } },
            "ClassResponseRef": { "Value": { "Ref": "ClassResponse" } },
            "ConditionResponseRef": { "Value": { "Ref": "ConditionResponse" } },
            "ApiDocsRef": { "Value": { "Ref": "ApiDocs" } },
            "MethodDocsRef": { "Value": { "Ref": "MethodDocs" } },
            "DocsRef": { "Value": { "Ref": "Docs" } }
        }
    })
}

#[tokio::test]
async fn a_stacks_documentation_and_gateway_responses_are_created_in_api_gateway() {
    let router = build_router();
    let case = "an api with two gateway responses, two documentation parts and a version";
    let stack = "apigw-docs-and-responses";
    let (settled, described) = deploy(
        &router,
        case,
        stack,
        &round_trip_template(&format!("{stack}-api")),
    )
    .await;
    let failures = {
        let (_, events) =
            cloudformation(&router, "DescribeStackEvents", &[("StackName", stack)]).await;
        elements(&events, "ResourceStatusReason")
    };
    assert_eq!(
        settled, CREATE_COMPLETE,
        "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {settled} with \
         resource failures {failures:?}"
    );

    let outputs = outputs_of(&described);
    let published = |name: &str| -> String {
        outputs.get(name).cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the stack published no {name} output in {described}")
        })
    };
    let api_id = published("ApiId");

    // RULE 1: a resource addressed by more than one value is published under all of them, in
    // the form its own type publishes; one a single assigned id names keeps that id.
    let rows: &[(&str, &str, String)] = &[
        (
            "a class-wide gateway response",
            "ClassResponseRef",
            format!("{api_id}:{CLASS_DEFAULT}"),
        ),
        (
            "one condition's gateway response",
            "ConditionResponseRef",
            format!("{api_id}:{ONE_CONDITION}"),
        ),
        (
            "the documentation snapshot",
            "DocsRef",
            format!("{VERSION}|{api_id}"),
        ),
    ];
    for (what, output, expected) in rows {
        let reference = published(output);
        assert_eq!(
            reference, *expected,
            "case {case:?}: expected {what}'s Ref to be the identifier its own calls take \
             ({expected}), got {reference:?}"
        );
    }
    for (what, output) in [
        ("the api-level documentation part", "ApiDocsRef"),
        ("the method-level documentation part", "MethodDocsRef"),
    ] {
        let reference = published(output);
        assert!(
            is_minted_id(&reference),
            "case {case:?}: expected {what}'s Ref to be an identifier API Gateway minted \
             ({MINTED_ID_LEN} lowercase alphanumerics), got {reference:?}"
        );
    }

    // RULE 2: each record really exists in API Gateway, with what the template declared.
    let class = read(
        &router,
        case,
        &format!("/restapis/{api_id}/gatewayresponses/{CLASS_DEFAULT}"),
    )
    .await;
    assert_eq!(
        (
            class.get("statusCode").and_then(Value::as_str),
            class.get("defaultResponse").and_then(Value::as_bool),
        ),
        (Some("444"), Some(false)),
        "case {case:?}: expected the class-wide response to carry the declared status code and \
         to report itself a developer's customization, got {class}"
    );
    assert_eq!(
        class
            .get("responseParameters")
            .and_then(|map| map.get("gatewayresponse.header.Access-Control-Allow-Origin"))
            .and_then(Value::as_str),
        Some("'*'"),
        "case {case:?}: expected the declared response parameter, got {class}"
    );
    assert_eq!(
        class
            .get("responseTemplates")
            .and_then(|map| map.get("application/json"))
            .and_then(Value::as_str),
        Some("{\"message\":\"refused\"}"),
        "case {case:?}: expected the declared response template, got {class}"
    );

    let condition = read(
        &router,
        case,
        &format!("/restapis/{api_id}/gatewayresponses/{ONE_CONDITION}"),
    )
    .await;
    assert_eq!(
        (
            condition.get("statusCode").and_then(Value::as_str),
            condition.get("defaultResponse").and_then(Value::as_bool),
        ),
        (Some("401"), Some(false)),
        "case {case:?}: expected one condition's response to carry the declared status code and \
         to report itself a developer's customization, got {condition}"
    );

    // A response type the stack customized NOTHING for still reports the one API Gateway
    // generates, so the two rows above are the stack's own records and not a blanket answer.
    let untouched = read(
        &router,
        case,
        &format!("/restapis/{api_id}/gatewayresponses/DEFAULT_5XX"),
    )
    .await;
    assert_eq!(
        untouched.get("defaultResponse").and_then(Value::as_bool),
        Some(true),
        "case {case:?}: expected an uncustomized response type to report API Gateway's own \
         generated response, got {untouched}"
    );

    for (what, output, properties) in [
        (
            "the api-level documentation part",
            "ApiDocsRef",
            "{\"info\":{\"description\":\"the items api\"}}",
        ),
        (
            "the method-level documentation part",
            "MethodDocsRef",
            "{\"summary\":\"list the items\"}",
        ),
    ] {
        let part_id = published(output);
        let part = read(
            &router,
            case,
            &format!("/restapis/{api_id}/documentation/parts/{part_id}"),
        )
        .await;
        assert_eq!(
            part.get("properties").and_then(Value::as_str),
            Some(properties),
            "case {case:?}: expected {what} to carry the declared content, got {part}"
        );
        assert_eq!(
            part.get("id").and_then(Value::as_str),
            Some(part_id.as_str()),
            "case {case:?}: expected {what} to report the id the stack published, got {part}"
        );
    }
    let api_part = read(
        &router,
        case,
        &format!("/restapis/{api_id}/documentation/parts/{}", published("ApiDocsRef")),
    )
    .await;
    assert_eq!(
        api_part
            .get("location")
            .and_then(|location| location.get("type"))
            .and_then(Value::as_str),
        Some("API"),
        "case {case:?}: expected the api-level part to document the API entity, got {api_part}"
    );
    let method_part = read(
        &router,
        case,
        &format!(
            "/restapis/{api_id}/documentation/parts/{}",
            published("MethodDocsRef")
        ),
    )
    .await;
    for (member, expected) in [("type", "METHOD"), ("path", "/items"), ("method", "GET")] {
        assert_eq!(
            method_part
                .get("location")
                .and_then(|location| location.get(member))
                .and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected the method-level part's location {member} to be \
             {expected:?}, got {method_part}"
        );
    }

    let version = read(
        &router,
        case,
        &format!("/restapis/{api_id}/documentation/versions/{VERSION}"),
    )
    .await;
    assert_eq!(
        (
            version.get("version").and_then(Value::as_str),
            version.get("description").and_then(Value::as_str),
        ),
        (Some(VERSION), Some("the first snapshot")),
        "case {case:?}: expected the snapshot the template declared, got {version}"
    );

    // RULE 2, the other half: the stack's delete takes each record away, so the api answers its
    // own generated response again and neither the part nor the snapshot resolves.
    let (code, deleted) = cloudformation(&router, "DeleteStack", &[("StackName", stack)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DeleteStack must answer 200, got {code} {deleted}"
    );
    let deadline = Instant::now() + TERMINAL_DEADLINE;
    loop {
        let (_, events) =
            cloudformation(&router, "DescribeStackEvents", &[("StackName", stack)]).await;
        if elements(&events, "ResourceStatus").contains(&DELETE_COMPLETE.to_string()) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the stack to finish deleting within {TERMINAL_DEADLINE:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    // The api went with the stack, so every read under it is refused — which is also what the
    // teardown of the records inside it leaves.
    for uri in [
        format!("/restapis/{api_id}/gatewayresponses/{CLASS_DEFAULT}"),
        format!("/restapis/{api_id}/documentation/versions/{VERSION}"),
    ] {
        let (status, answered) = apigateway(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GET {uri} to be refused once the stack is gone, got \
             {status} with {answered}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a status code only API Gateway can judge
// ---------------------------------------------------------------------------

/// The template one invalid-status-code row deploys: ONE gateway response, on an api that
/// EXISTS already, so what the row leaves behind is readable after the rollback.
fn gateway_response_template(api_id: &str, status_code: &str) -> Value {
    json!({
        "Resources": {
            "Response": {
                "Type": "AWS::ApiGateway::GatewayResponse",
                "Properties": {
                    "RestApiId": api_id,
                    "ResponseType": CLASS_DEFAULT,
                    "StatusCode": status_code
                }
            }
        }
    })
}

#[tokio::test]
async fn a_gateway_response_status_code_no_http_status_rolls_the_stack_back() {
    // Rows differ along HOW the declared status code is no HTTP status code: a class outside
    // the ones a status code has, too few digits, too many, and text that is no number at all
    // — the shape `StatusCode` publishes being `[1-5]\d\d`
    // (`smithy_query(query="shape", shape="StatusCode")`). The controls are each end of what
    // that shape admits.
    let refused = ["999", "99", "4044", "ok"];
    let admitted = ["100", "599"];

    let router = build_router();
    // The api is created in API Gateway itself, so the rows below declare only the
    // customization and the api survives every rollback for the read that follows it.
    let (code, api) = apigateway(
        &router,
        "POST",
        "/restapis",
        Some(json!({ "name": "gateway-response-status-codes" })),
    )
    .await;
    assert_eq!(
        code, 201,
        "expected the api to be created, got {code} with {api}"
    );
    let api_id = api
        .get("id")
        .and_then(Value::as_str)
        .expect("the api's own id")
        .to_string();

    for (index, status_code) in refused.iter().enumerate() {
        let case = format!("a declared StatusCode of {status_code:?}");
        let stack = format!("apigw-bad-status-{index}");
        let (settled, described) = deploy(
            &router,
            &case,
            &stack,
            &gateway_response_template(&api_id, status_code),
        )
        .await;
        assert_eq!(
            settled, ROLLBACK_COMPLETE,
            "case {case:?}: expected the stack to roll back, got {settled} ({described})"
        );
        // The refusal is the OWNING service's, so it is reported on the resource's own failed
        // event and names the member it judged.
        let (_, events) =
            cloudformation(&router, "DescribeStackEvents", &[("StackName", &stack)]).await;
        let reasons = elements(&events, "ResourceStatusReason");
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains("statusCode") && reason.contains(status_code)),
            "case {case:?}: expected a failure reason naming the status code it judged, got \
             {reasons:?}"
        );
        // ...and the api holds no customization of that response type: what it answers is the
        // response API Gateway generates.
        let answered = read(
            &router,
            &case,
            &format!("/restapis/{api_id}/gatewayresponses/{CLASS_DEFAULT}"),
        )
        .await;
        assert_eq!(
            answered.get("defaultResponse").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: expected the api to hold no customization, got {answered}"
        );
    }

    // The controls: a status code the shape admits is deployed and read back, so the rows above
    // are refused for the value they declared and nothing else.
    for (index, status_code) in admitted.iter().enumerate() {
        let case = format!("a declared StatusCode of {status_code:?}");
        let stack = format!("apigw-good-status-{index}");
        let (settled, described) = deploy(
            &router,
            &case,
            &stack,
            &gateway_response_template(&api_id, status_code),
        )
        .await;
        assert_eq!(
            settled, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {settled} \
             ({described})"
        );
        let answered = read(
            &router,
            &case,
            &format!("/restapis/{api_id}/gatewayresponses/{CLASS_DEFAULT}"),
        )
        .await;
        assert_eq!(
            (
                answered.get("statusCode").and_then(Value::as_str),
                answered.get("defaultResponse").and_then(Value::as_bool),
            ),
            (Some(*status_code), Some(false)),
            "case {case:?}: expected the customization the stack put there, got {answered}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a property outside what the type's schema admits
// ---------------------------------------------------------------------------

/// One row of the schema-validation table: what the declaration carries, which member the
/// validator reports it at, and the sentence that validator words the failure in.
struct SchemaRow {
    case: &'static str,
    /// The resources the row declares, keyed by logical id.
    resources: Value,
    /// The logical id of the resource the validation fails on, and the member path inside its
    /// property document — together the `ValidationPath` the event carries.
    logical_id: &'static str,
    member: &'static str,
    /// The schema validator's own sentence for the constraint that failed, or `None` for a row
    /// the schema admits.
    reason: Option<String>,
}

/// The one value `AWS::ApiGateway::UsagePlanKey.KeyType` admits, and the values its quota's
/// `Period` admits: the sets those properties' schemas state (the published allowed values of
/// each, which are API Gateway's own `CreateUsagePlanKey$keyType` and `QuotaPeriodType`).
const KEY_TYPE: &str = "API_KEY";
const QUOTA_PERIODS: &[&str] = &["DAY", "WEEK", "MONTH"];

/// The floor the `Quota` of a usage plan admits its request counts in.
const MIN_QUOTA_REQUESTS: i64 = 0;

/// How the schema validator reports a value outside the set its property's schema states.
fn not_a_valid_enum_value(value: &str, values: &[&str]) -> String {
    format!(
        "{value} is not a valid enum value. Supported values: [{}]",
        values.join(", ")
    )
}

/// The same validator's sentence for a number below the `minimum` its property's schema states.
fn not_greater_or_equal(member: &str, value: &str, minimum: i64) -> String {
    format!("#/{member}: {value} is not greater or equal to {minimum}")
}

/// An `AWS::ApiGateway::UsagePlanKey` declaration (with the key and the plan it needs) whose
/// `KeyType` is `key_type`.
fn association_resources(name: &str, key_type: &str) -> Value {
    json!({
        "Key": {
            "Type": "AWS::ApiGateway::ApiKey",
            "Properties": { "Name": format!("{name}-key"), "Enabled": true }
        },
        "Plan": {
            "Type": "AWS::ApiGateway::UsagePlan",
            "Properties": { "UsagePlanName": format!("{name}-plan") }
        },
        "PlanKey": {
            "Type": "AWS::ApiGateway::UsagePlanKey",
            "Properties": {
                "KeyId": { "Ref": "Key" },
                "KeyType": key_type,
                "UsagePlanId": { "Ref": "Plan" }
            }
        }
    })
}

/// An `AWS::ApiGateway::UsagePlan` declaration carrying `quota`.
fn plan_resources(name: &str, quota: Value) -> Value {
    json!({
        "Plan": {
            "Type": "AWS::ApiGateway::UsagePlan",
            "Properties": { "UsagePlanName": format!("{name}-plan"), "Quota": quota }
        }
    })
}

/// The rows: a `KeyType` outside the one value its schema states (in two forms), a quota
/// request count below the floor (each of the two members that carries one), a quota `Period`
/// outside the set its schema states, and — as the controls — the floor itself and each value
/// of that set.
fn schema_rows() -> Vec<SchemaRow> {
    let mut rows = vec![
        SchemaRow {
            case: "a KeyType in the owning API's own casing",
            resources: association_resources("keytype-cased", "api_key"),
            logical_id: "PlanKey",
            member: "KeyType",
            reason: Some(not_a_valid_enum_value("api_key", &[KEY_TYPE])),
        },
        SchemaRow {
            case: "a KeyType that is no key type at all",
            resources: association_resources("keytype-unknown", "APIKEY"),
            logical_id: "PlanKey",
            member: "KeyType",
            reason: Some(not_a_valid_enum_value("APIKEY", &[KEY_TYPE])),
        },
        SchemaRow {
            case: "a quota limit one below the floor",
            resources: plan_resources(
                "quota-limit",
                json!({ "Limit": -1, "Offset": 0, "Period": "DAY" }),
            ),
            logical_id: "Plan",
            member: "Quota/Limit",
            reason: Some(not_greater_or_equal(
                "Quota/Limit",
                "-1",
                MIN_QUOTA_REQUESTS,
            )),
        },
        SchemaRow {
            case: "a quota offset far below the floor",
            resources: plan_resources(
                "quota-offset",
                json!({ "Limit": 100, "Offset": -500, "Period": "WEEK" }),
            ),
            logical_id: "Plan",
            member: "Quota/Offset",
            reason: Some(not_greater_or_equal(
                "Quota/Offset",
                "-500",
                MIN_QUOTA_REQUESTS,
            )),
        },
        SchemaRow {
            case: "a quota period that is no period at all",
            resources: plan_resources(
                "quota-period",
                json!({ "Limit": 100, "Offset": 0, "Period": "FORTNIGHT" }),
            ),
            logical_id: "Plan",
            member: "Quota/Period",
            reason: Some(not_a_valid_enum_value("FORTNIGHT", QUOTA_PERIODS)),
        },
        SchemaRow {
            case: "the one KeyType the schema states",
            resources: association_resources("keytype-admitted", KEY_TYPE),
            logical_id: "PlanKey",
            member: "KeyType",
            reason: None,
        },
        SchemaRow {
            case: "a quota at the floor its schema states",
            resources: plan_resources(
                "quota-floor",
                json!({ "Limit": MIN_QUOTA_REQUESTS, "Offset": MIN_QUOTA_REQUESTS,
                        "Period": "MONTH" }),
            ),
            logical_id: "Plan",
            member: "Quota/Limit",
            reason: None,
        },
    ];
    // Every value the period's set admits, including the ones the rows above did not cite.
    for period in QUOTA_PERIODS {
        rows.push(SchemaRow {
            case: "a quota period the schema states",
            resources: plan_resources(
                &format!("quota-{}", period.to_ascii_lowercase()),
                json!({ "Limit": 10, "Offset": 0, "Period": period }),
            ),
            logical_id: "Plan",
            member: "Quota/Period",
            reason: None,
        });
    }
    rows
}

/// The usage plan names API Gateway holds right now.
async fn usage_plan_names(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let page = read(router, case, "/usageplans?limit=500").await;
    page.get("item")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| entry.get("name").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_property_outside_its_schema_fails_the_stack_before_api_gateway_is_called() {
    let router = build_router();
    for (index, row) in schema_rows().into_iter().enumerate() {
        let case = row.case;
        let stack = format!("apigw-schema-{index}");
        let template = json!({ "Resources": row.resources });
        let (settled, described) = deploy(&router, case, &stack, &template).await;
        let plan_name = row
            .resources
            .get("Plan")
            .and_then(|plan| plan.get("Properties"))
            .and_then(|properties| properties.get("UsagePlanName"))
            .and_then(Value::as_str)
            .expect("every row declares a usage plan")
            .to_string();

        match &row.reason {
            Some(reason) => {
                assert_eq!(
                    settled, ROLLBACK_COMPLETE,
                    "case {case:?}: expected the stack to roll back, got {settled} ({described})"
                );
                // What the STACK reports is the summary over the stage's one error.
                let failures = stack_failure_reasons(&router, case, &stack).await;
                assert_eq!(
                    failures,
                    vec![summary(1)],
                    "case {case:?}: expected the stack's own failure reason to be the \
                     pre-deployment summary, got {failures:?}"
                );
                // ...and the DETAIL is one validation result about the resource nothing was
                // done to, at the member's own path, in the schema validator's own sentence.
                let results = validation_results(&router, case, &stack).await;
                assert_eq!(
                    results,
                    vec![(
                        row.logical_id.to_string(),
                        VALIDATION_ERROR.to_string(),
                        PROPERTY_VALIDATION.to_string(),
                        FAILED.to_string(),
                        FAIL.to_string(),
                        format!("/Resources/{}/Properties/{}", row.logical_id, row.member),
                    )],
                    "case {case:?}: expected one validation result at the member's own path"
                );
                let (_, events) =
                    cloudformation(&router, "DescribeEvents", &[("StackName", &stack)]).await;
                let reported = elements(&events, "ValidationStatusReason");
                assert_eq!(
                    reported,
                    vec![reason.clone()],
                    "case {case:?}: expected the validator's own sentence {reason:?}"
                );
                // Nothing of the template reached API Gateway: the plan the row declared is
                // not there.
                let held = usage_plan_names(&router, case).await;
                assert!(
                    !held.contains(&plan_name),
                    "case {case:?}: expected API Gateway to hold no plan {plan_name:?}, got \
                     {held:?}"
                );
            }
            None => {
                assert_eq!(
                    settled, CREATE_COMPLETE,
                    "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got \
                     {settled} ({described})"
                );
                let held = usage_plan_names(&router, case).await;
                assert!(
                    held.contains(&plan_name),
                    "case {case:?}: expected API Gateway to hold the plan {plan_name:?} the \
                     stack created, got {held:?}"
                );
            }
        }
    }
}
