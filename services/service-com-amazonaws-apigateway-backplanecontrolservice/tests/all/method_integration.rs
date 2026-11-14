// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a method's integration: `PutIntegration` and `GetIntegration`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource` helpers are this crate's existing harness, reused
//! from `rest_api_lifecycle`.
//!
//! The tests assert RULES over sets of inputs, not one recorded call: the create/read
//! round trip over a table spanning every `IntegrationType` the model declares and the
//! optional members each of them takes, the members the service fills in when a request
//! sets none, the published value sets and bounds (`passthroughBehavior`'s three values,
//! `timeoutInMillis` on each bound and one past it), the type-dependent requirements the
//! `Integration` members document, and every identifier these operations resolve over
//! the records that do and do not exist.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::put_integration::{
    DEFAULT_CONNECTION_TYPE, DEFAULT_PASSTHROUGH_BEHAVIOR, DEFAULT_RESPONSE_TRANSFER_MODE,
    DEFAULT_TIMEOUT_IN_MILLIS, MAX_TIMEOUT_IN_MILLIS, MIN_TIMEOUT_IN_MILLIS, PASSTHROUGH_BEHAVIORS,
    VPC_LINK_CONNECTION_TYPE,
};

use crate::rest_api_lifecycle::{
    assert_failure, create_api, create_resource, string_member, Answer, Gateway,
};

/// A fresh resource under `root` carrying one `verb` method, answering its resource id.
/// `request_parameters` is the method's `requestParameters` map, which is what a
/// `cacheKeyParameters` entry has to name.
///
/// Shared with the sibling `method_response` suite, which puts method responses on the
/// methods it builds, so both suites stand a method up the same way.
pub(crate) async fn resource_with_a_method(
    gateway: &Gateway,
    api: &str,
    root: &str,
    path_part: &str,
    verb: &str,
    request_parameters: Value,
) -> String {
    let resource = create_resource(gateway, api, root, path_part).await;
    let resource_id = string_member(&resource, "id", path_part);
    let mut request = json!({ "authorizationType": "NONE" });
    if let (Some(target), Some(parameters)) = (request.as_object_mut(), request_parameters.as_object())
    {
        if !parameters.is_empty() {
            target.insert("requestParameters".to_string(), request_parameters.clone());
        }
    }
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}"),
            Some(request),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutMethod({path_part}/{verb}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    resource_id
}

/// `PutMethod` for `verb` on `resource` of `api`, with nothing but the authorization
/// type the method needs, asserting the modelled 201.
///
/// This crate's one copy of that call, shared with the sibling `delete_lifecycle` and
/// `delete_integration_response` suites, which stand several verbs up on ONE resource —
/// which is what [`resource_with_a_method`], one method per fresh resource, does not do.
pub(crate) async fn put_method(gateway: &Gateway, api: &str, resource: &str, verb: &str) {
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

/// `PutIntegration` for `verb` on `resource` of `api`.
///
/// The integration's own method is the input's `integrationHttpMethod`, whose `jsonName`
/// is `httpMethod` (`smithy_query(query="operation",operation="PutIntegration")`), so
/// that is the member name a request body carries — the method's verb is the path label.
///
/// Shared with the sibling `integration_response` suite, which puts integration responses
/// on the integrations it stands up, so both suites create an integration the same way.
pub(crate) async fn put_integration(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    request: Value,
) -> Answer {
    gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
            Some(request),
        )
        .await
}

/// `DeleteIntegration` for `verb` on `resource` of `api`.
///
/// This crate's one copy of that call, shared with the sibling `delete_integration` and
/// `test_invoke_method_no_integration` suites, so every suite removes an integration the
/// same way; see [`put_integration`].
pub(crate) async fn delete_integration(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
            None,
        )
        .await
}

/// `GetIntegration` for `verb` on `resource` of `api`. Shared with the sibling
/// `integration_response` suite, which reads the stored responses back through the
/// integration that owns them.
pub(crate) async fn get_integration(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
            None,
        )
        .await
}

/// `GetMethod` for `verb` on `resource` of `api`, which reports the same stored
/// integration as its `methodIntegration` and the stored method responses as its
/// `methodResponses`. Shared with the sibling `method_response` suite.
pub(crate) async fn get_method(gateway: &Gateway, api: &str, resource: &str, verb: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
            None,
        )
        .await
}

/// An AWS integration ARN, the form `uri` documents for the `AWS`/`AWS_PROXY` types:
/// `arn:{partition}:apigateway:{region}:{service}:path|action/{service_api}`.
///
/// Shared with the sibling `delete_integration` suite, which stands AWS integrations up
/// to delete them.
pub(crate) fn aws_integration_uri(tail: &str) -> String {
    format!("arn:aws:apigateway:us-east-1:{tail}")
}

#[tokio::test]
async fn an_integration_put_on_a_method_is_persisted_and_read_back_member_for_member() {
    // Rows differ along what the rule generalises over: the integration type (every
    // value of the modelled `IntegrationType`), which optional members the request
    // carries, the connection type, and the timeout on each published bound. The
    // expected body carries `"cacheNamespace": null` where the service fills the member
    // in from the resource id, which the loop substitutes, and every row carries the
    // `responseTransferMode` the service reports on an integration that asked for none.
    let cases: Vec<(&str, &str, &str, Value, Value)> = vec![
        (
            "a mock integration with nothing else set",
            "mock",
            "GET",
            json!({ "type": "MOCK" }),
            json!({
                "type": "MOCK",
                "connectionType": DEFAULT_CONNECTION_TYPE,
                "responseTransferMode": DEFAULT_RESPONSE_TRANSFER_MODE,
                "passthroughBehavior": DEFAULT_PASSTHROUGH_BEHAVIOR,
                "timeoutInMillis": DEFAULT_TIMEOUT_IN_MILLIS,
                "cacheNamespace": null,
                "cacheKeyParameters": []
            }),
        ),
        (
            "a mock integration with a mapping template and the lowest timeout",
            "mock-template",
            "POST",
            json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
                "timeoutInMillis": MIN_TIMEOUT_IN_MILLIS,
                "passthroughBehavior": "NEVER"
            }),
            json!({
                "type": "MOCK",
                "connectionType": DEFAULT_CONNECTION_TYPE,
                "responseTransferMode": DEFAULT_RESPONSE_TRANSFER_MODE,
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
                "passthroughBehavior": "NEVER",
                "timeoutInMillis": MIN_TIMEOUT_IN_MILLIS,
                "cacheNamespace": null,
                "cacheKeyParameters": []
            }),
        ),
        (
            "an http integration naming a formed https endpoint",
            "http",
            "GET",
            json!({
                "type": "HTTP",
                "httpMethod": "GET",
                "uri": "https://example.com/pets",
                "requestParameters": { "integration.request.querystring.breed": "method.request.querystring.breed" }
            }),
            json!({
                "type": "HTTP",
                "httpMethod": "GET",
                "uri": "https://example.com/pets",
                "connectionType": DEFAULT_CONNECTION_TYPE,
                "responseTransferMode": DEFAULT_RESPONSE_TRANSFER_MODE,
                "requestParameters": { "integration.request.querystring.breed": "method.request.querystring.breed" },
                "passthroughBehavior": DEFAULT_PASSTHROUGH_BEHAVIOR,
                "timeoutInMillis": DEFAULT_TIMEOUT_IN_MILLIS,
                "cacheNamespace": null,
                "cacheKeyParameters": []
            }),
        ),
        (
            "an http proxy integration with content handling and the highest timeout",
            "http-proxy",
            "PUT",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "put",
                "uri": "http://example.com/pets",
                "contentHandling": "CONVERT_TO_TEXT",
                "timeoutInMillis": MAX_TIMEOUT_IN_MILLIS,
                "tlsConfig": { "insecureSkipVerification": true }
            }),
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "PUT",
                "uri": "http://example.com/pets",
                "connectionType": DEFAULT_CONNECTION_TYPE,
                "responseTransferMode": DEFAULT_RESPONSE_TRANSFER_MODE,
                "contentHandling": "CONVERT_TO_TEXT",
                "passthroughBehavior": DEFAULT_PASSTHROUGH_BEHAVIOR,
                "timeoutInMillis": MAX_TIMEOUT_IN_MILLIS,
                "cacheNamespace": null,
                "cacheKeyParameters": [],
                "tlsConfig": { "insecureSkipVerification": true }
            }),
        ),
        (
            "an aws service action with credentials and its own cache namespace",
            "aws",
            "POST",
            json!({
                "type": "AWS",
                "httpMethod": "POST",
                "uri": aws_integration_uri("dynamodb:action/PutItem"),
                "credentials": "arn:aws:iam::123456789012:role/apigateway-dynamodb",
                "cacheNamespace": "pets-cache",
                "passthroughBehavior": "WHEN_NO_TEMPLATES"
            }),
            json!({
                "type": "AWS",
                "httpMethod": "POST",
                "uri": aws_integration_uri("dynamodb:action/PutItem"),
                "connectionType": DEFAULT_CONNECTION_TYPE,
                "responseTransferMode": DEFAULT_RESPONSE_TRANSFER_MODE,
                "credentials": "arn:aws:iam::123456789012:role/apigateway-dynamodb",
                "passthroughBehavior": "WHEN_NO_TEMPLATES",
                "timeoutInMillis": DEFAULT_TIMEOUT_IN_MILLIS,
                "cacheNamespace": "pets-cache",
                "cacheKeyParameters": []
            }),
        ),
        (
            "a lambda proxy integration",
            "lambda",
            "ANY",
            json!({
                "type": "AWS_PROXY",
                "httpMethod": "POST",
                "uri": aws_integration_uri(
                    "lambda:path/2015-03-31/functions/arn:aws:lambda:us-east-1:123456789012:function:pets/invocations"
                )
            }),
            json!({
                "type": "AWS_PROXY",
                "httpMethod": "POST",
                "uri": aws_integration_uri(
                    "lambda:path/2015-03-31/functions/arn:aws:lambda:us-east-1:123456789012:function:pets/invocations"
                ),
                "connectionType": DEFAULT_CONNECTION_TYPE,
                "responseTransferMode": DEFAULT_RESPONSE_TRANSFER_MODE,
                "passthroughBehavior": DEFAULT_PASSTHROUGH_BEHAVIOR,
                "timeoutInMillis": DEFAULT_TIMEOUT_IN_MILLIS,
                "cacheNamespace": null,
                "cacheKeyParameters": []
            }),
        ),
        (
            "a private integration through a vpc link",
            "private",
            "HEAD",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "HEAD",
                "uri": "nlb-pets.elb.us-east-1.amazonaws.com",
                "connectionType": VPC_LINK_CONNECTION_TYPE,
                "connectionId": "abc123"
            }),
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "HEAD",
                "uri": "nlb-pets.elb.us-east-1.amazonaws.com",
                "connectionType": VPC_LINK_CONNECTION_TYPE,
                "connectionId": "abc123",
                "responseTransferMode": DEFAULT_RESPONSE_TRANSFER_MODE,
                "passthroughBehavior": DEFAULT_PASSTHROUGH_BEHAVIOR,
                "timeoutInMillis": DEFAULT_TIMEOUT_IN_MILLIS,
                "cacheNamespace": null,
                "cacheKeyParameters": []
            }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-round-trip").await;

    for (case, path_part, verb, request, expected) in &cases {
        let resource =
            resource_with_a_method(&gateway, &api, &root, path_part, verb, json!({})).await;
        let created = put_integration(&gateway, &api, &resource, verb, request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from PutIntegration, got {} with {:?}",
            created.status, created.body
        );

        // The body is the stored record: every member the request set, plus the ones the
        // service fills in, and nothing else.
        let mut expected = expected.clone();
        if let Some(object) = expected.as_object_mut() {
            if object.get("cacheNamespace") == Some(&Value::Null) {
                object.insert("cacheNamespace".to_string(), json!(resource));
            }
        }
        assert_eq!(
            created.body, expected,
            "case {case}: expected the integration {expected:?}, got {:?}",
            created.body
        );

        // Read back through GetIntegration, and through the method that owns it.
        let read = get_integration(&gateway, &api, &resource, verb).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetIntegration, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created.body,
            "case {case}: GetIntegration must report the stored integration, got {:?} after {:?}",
            read.body, created.body
        );
        let method = get_method(&gateway, &api, &resource, verb).await;
        assert_eq!(
            method.body["methodIntegration"], created.body,
            "case {case}: the method must carry the stored integration, got {:?}",
            method.body["methodIntegration"]
        );
    }
}

#[tokio::test]
async fn an_integration_is_refused_outside_the_rules_its_members_publish() {
    // Rows span each published rule, one broken member at a time: the type-dependent
    // `uri` and `integrationHttpMethod`, the URI forms the two type families take, the
    // `connectionId` that belongs only to a VPC_LINK, `passthroughBehavior`'s value set,
    // and `timeoutInMillis` one past each published bound.
    let cases: Vec<(&str, Value)> = vec![
        (
            "a non-mock type with no uri",
            json!({ "type": "HTTP", "httpMethod": "GET" }),
        ),
        (
            "a non-mock type with no integrationHttpMethod",
            json!({ "type": "HTTP", "uri": "https://example.com/pets" }),
        ),
        (
            "an http type whose uri is not a formed HTTP(S) URL",
            json!({
                "type": "HTTP",
                "httpMethod": "GET",
                "uri": "example.com/pets"
            }),
        ),
        (
            "an aws type whose uri is not an integration ARN",
            json!({
                "type": "AWS",
                "httpMethod": "POST",
                "uri": "https://dynamodb.us-east-1.amazonaws.com/"
            }),
        ),
        (
            "an integration method outside the HTTP verbs",
            json!({
                "type": "HTTP",
                "httpMethod": "FETCH",
                "uri": "https://example.com/pets"
            }),
        ),
        (
            "a vpc link connection with no connectionId",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "GET",
                "uri": "nlb-pets.elb.us-east-1.amazonaws.com",
                "connectionType": VPC_LINK_CONNECTION_TYPE
            }),
        ),
        (
            "a connectionId on an internet connection",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "GET",
                "uri": "https://example.com/pets",
                "connectionType": DEFAULT_CONNECTION_TYPE,
                "connectionId": "abc123"
            }),
        ),
        (
            "a passthroughBehavior outside the documented set",
            json!({ "type": "MOCK", "passthroughBehavior": "SOMETIMES" }),
        ),
        (
            "a timeout one below the published minimum",
            json!({ "type": "MOCK", "timeoutInMillis": MIN_TIMEOUT_IN_MILLIS - 1 }),
        ),
        (
            "a timeout one above the published maximum",
            json!({ "type": "MOCK", "timeoutInMillis": MAX_TIMEOUT_IN_MILLIS + 1 }),
        ),
        (
            "a cache key parameter that is not a request parameter of the method",
            json!({
                "type": "MOCK",
                "cacheKeyParameters": ["method.request.querystring.colour"]
            }),
        ),
        (
            "an empty uri",
            json!({ "type": "HTTP", "httpMethod": "GET", "uri": "   " }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-refusals").await;
    let resource = resource_with_a_method(
        &gateway,
        &api,
        &root,
        "pets",
        "GET",
        json!({ "method.request.querystring.breed": true }),
    )
    .await;

    for (case, request) in &cases {
        let answer = put_integration(&gateway, &api, &resource, "GET", request.clone()).await;
        assert_failure(&answer, 400, "BadRequestException", case);
        // A refused request stores nothing, so the method still has no integration.
        assert_failure(
            &get_integration(&gateway, &api, &resource, "GET").await,
            404,
            "NotFoundException",
            &format!("{case} / GetIntegration after the refusal"),
        );
    }

    // Each of the three documented `passthroughBehavior` values, and a cache key
    // parameter that IS a request parameter of the method, are accepted: the refusals
    // above are about the value, not about the member.
    for behavior in PASSTHROUGH_BEHAVIORS {
        let accepted = put_integration(
            &gateway,
            &api,
            &resource,
            "GET",
            json!({
                "type": "MOCK",
                "passthroughBehavior": behavior,
                "cacheKeyParameters": ["method.request.querystring.breed"]
            }),
        )
        .await;
        assert_eq!(
            accepted.status, 201,
            "case PutIntegration(passthroughBehavior={behavior}): expected 201, got {} with {:?}",
            accepted.status, accepted.body
        );
        let read = get_integration(&gateway, &api, &resource, "GET").await;
        assert_eq!(
            read.body["passthroughBehavior"].as_str(),
            Some(*behavior),
            "case GetIntegration(passthroughBehavior={behavior}): expected the stored value, got {:?}",
            read.body["passthroughBehavior"]
        );
        assert_eq!(
            read.body["cacheKeyParameters"],
            json!(["method.request.querystring.breed"]),
            "case GetIntegration(passthroughBehavior={behavior}): expected the stored cache key parameters, got {:?}",
            read.body["cacheKeyParameters"]
        );
    }
}

#[tokio::test]
async fn an_unknown_identifier_is_a_not_found_on_every_integration_operation() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-notfound").await;
    let resource =
        resource_with_a_method(&gateway, &api, &root, "pets", "GET", json!({})).await;
    let missing = "zzzzzzzzzz";

    // Rows span each identifier these operations resolve, one unknown at a time — plus
    // the method that exists but carries no integration yet.
    let cases: Vec<(&str, String, Option<Value>)> = vec![
        (
            "PUT",
            format!("/restapis/{missing}/resources/{resource}/methods/GET/integration"),
            Some(json!({ "type": "MOCK" })),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{missing}/methods/GET/integration"),
            Some(json!({ "type": "MOCK" })),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{resource}/methods/DELETE/integration"),
            Some(json!({ "type": "MOCK" })),
        ),
        (
            "GET",
            format!("/restapis/{missing}/resources/{resource}/methods/GET/integration"),
            None,
        ),
        (
            "GET",
            format!("/restapis/{api}/resources/{missing}/methods/GET/integration"),
            None,
        ),
        (
            "GET",
            format!("/restapis/{api}/resources/{resource}/methods/DELETE/integration"),
            None,
        ),
        (
            "GET",
            format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
            None,
        ),
    ];

    for (method, uri, body) in cases {
        let case = format!("{method} {uri}");
        assert_failure(
            &gateway.call(method, &uri, body).await,
            404,
            "NotFoundException",
            &case,
        );
    }

    // The integration of a method on a deleted API goes with it.
    let created = put_integration(&gateway, &api, &resource, "GET", json!({ "type": "MOCK" })).await;
    assert_eq!(
        created.status, 201,
        "case PutIntegration(before DeleteRestApi): expected 201, got {} with {:?}",
        created.status, created.body
    );
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &get_integration(&gateway, &api, &resource, "GET").await,
        404,
        "NotFoundException",
        "GetIntegration after DeleteRestApi",
    );
}

#[tokio::test]
async fn putting_an_integration_again_replaces_the_one_the_method_had() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-replace").await;
    let resource =
        resource_with_a_method(&gateway, &api, &root, "pets", "GET", json!({})).await;

    // Each put is the integration the method has afterwards: a PUT replaces, it does not
    // merge, so a member the second request does not set is not carried over.
    let puts: Vec<(&str, Value)> = vec![
        (
            "the mock integration first",
            json!({ "type": "MOCK", "cacheNamespace": "first", "timeoutInMillis": 1_000 }),
        ),
        (
            "then an http integration",
            json!({
                "type": "HTTP",
                "httpMethod": "GET",
                "uri": "https://example.com/pets"
            }),
        ),
    ];

    for (case, request) in &puts {
        let answer = put_integration(&gateway, &api, &resource, "GET", request.clone()).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201 from PutIntegration, got {} with {:?}",
            answer.status, answer.body
        );
        let read = get_integration(&gateway, &api, &resource, "GET").await;
        assert_eq!(
            read.body, answer.body,
            "case {case}: GetIntegration must report the integration just put, got {:?} after {:?}",
            read.body, answer.body
        );
    }

    let settled = get_integration(&gateway, &api, &resource, "GET").await;
    assert_eq!(
        settled.body["type"].as_str(),
        Some("HTTP"),
        "case the second put: expected the replacing integration, got {:?}",
        settled.body
    );
    assert_eq!(
        settled.body["cacheNamespace"].as_str(),
        Some(resource.as_str()),
        "case the second put: a member the request did not set takes the service's value, got {:?}",
        settled.body["cacheNamespace"]
    );
    assert_eq!(
        settled.body["timeoutInMillis"],
        json!(DEFAULT_TIMEOUT_IN_MILLIS),
        "case the second put: a member the request did not set takes its default, got {:?}",
        settled.body["timeoutInMillis"]
    );

    // Reading the integration does not change what a later caller sees.
    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let read = get_integration(&gateway, &api, &resource, "GET").await;
        assert_eq!(
            read.status, 200,
            "case GetIntegration(round {round}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        seen.push(read.body);
    }
    assert_eq!(
        seen[0], seen[2],
        "case GetIntegration repeated: a read must not change the record, got {:?} then {:?}",
        seen[0], seen[2]
    );
}
