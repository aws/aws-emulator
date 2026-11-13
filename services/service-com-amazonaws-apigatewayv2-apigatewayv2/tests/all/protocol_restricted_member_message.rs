// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the WORDS the `ProtocolType` gate refuses with.
//!
//! The rule these cases assert: **the sentence a protocol-restricted request is refused
//! with belongs to the restricted THING, not to the operation that received it** — so
//! every operation that carries a member answers the same sentence for it, and a member
//! that merely selects a resource of the other protocol (a selection expression) is
//! reported as a feature named in prose that "is not necessary", while a member that
//! configures behaviour the other protocol does not implement is named as the request
//! spells it and "is not permitted". A whole child resource takes the resource form,
//! named in the plural.
//!
//! The rows vary what the rule generalises over:
//!
//! * which sentence the restricted thing takes — the prose "is not necessary" form, the
//!   member "is not permitted" form, and the plural resource form;
//! * the direction of the restriction — "Supported only for WebSocket APIs" and
//!   "Supported only for HTTP APIs", so the protocol token in the sentence is read from
//!   the api rather than fixed;
//! * the member's type — a flag, a string, a map, a structure;
//! * the operation that receives it — `CreateRoute`/`UpdateRoute`,
//!   `CreateIntegration`/`UpdateIntegration`, `CreateStage`/`UpdateStage`, `CreateModel`
//!   and `CreateRouteResponse`.
//!
//! Every expected sentence is the service's own text for that member; the recorded case
//! that filed the issue — a route response selection expression on an HTTP api — is ONE
//! row. Each refusal is also proved to have written nothing: the resource the update leg
//! patched still carries no value for the member it was refused for.

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    HTTP_PROTOCOL_TYPE, WEBSOCKET_PROTOCOL_TYPE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// One restricted member, driven through both the create and the update of its resource.
struct Row {
    case: &'static str,
    /// The child collection under `/v2/apis/{ApiId}/`.
    collection: &'static str,
    /// A request for that collection carrying nothing restricted: what the update leg
    /// creates before it patches.
    base: Value,
    /// The member the output reports the created resource's id under, for the patch URI.
    id_member: &'static str,
    /// The restricted member, as a request spells it, and the value this row sets.
    member: &'static str,
    /// The member's name on the wire, to read the resource back by.
    json_name: &'static str,
    value: Value,
    /// The one `ProtocolType` whose APIs support the member — the OTHER one refuses.
    supported_by: &'static str,
    /// The sentence the refusing api answers with, protocol token included.
    expected: &'static str,
}

/// An `HTTP_PROXY` integration, the one integration type both protocols support.
fn proxy_integration() -> Value {
    json!({
        "integrationType": "HTTP_PROXY",
        "integrationUri": "https://example.com/upstream",
        "payloadFormatVersion": "1.0"
    })
}

fn rows() -> Vec<Row> {
    vec![
        // --- selection expressions: the prose "is not necessary" sentence. Each of
        // these members only SELECTS a WebSocket resource (a route response, a model, a
        // request template), so an HTTP api has nothing for it to select.
        Row {
            case: "CreateRoute$RouteResponseSelectionExpression on an HTTP api",
            collection: "routes",
            base: json!({ "routeKey": "GET /rrse" }),
            id_member: "routeId",
            member: "RouteResponseSelectionExpression",
            json_name: "routeResponseSelectionExpression",
            value: json!("$default"),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            expected: "Currently, a route response selection expression is not necessary \
                       for APIs with  a protocol type of HTTP",
        },
        Row {
            case: "CreateRoute$ModelSelectionExpression on an HTTP api",
            collection: "routes",
            base: json!({ "routeKey": "GET /mse" }),
            id_member: "routeId",
            member: "ModelSelectionExpression",
            json_name: "modelSelectionExpression",
            value: json!("$request.body.action"),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            expected: "Currently, a model selection expression is not necessary for APIs \
                       with  a protocol type of HTTP",
        },
        Row {
            case: "CreateIntegration$TemplateSelectionExpression on an HTTP api",
            collection: "integrations",
            base: proxy_integration(),
            id_member: "integrationId",
            member: "TemplateSelectionExpression",
            json_name: "templateSelectionExpression",
            value: json!("$request.body.action"),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            expected: "Currently, a template selection expression is not necessary for \
                       APIs with  a protocol type of HTTP",
        },
        // --- members that configure behaviour: the member "is not permitted" sentence.
        Row {
            case: "CreateRoute$ApiKeyRequired, a flag, on an HTTP api",
            collection: "routes",
            base: json!({ "routeKey": "GET /apikey" }),
            id_member: "routeId",
            member: "ApiKeyRequired",
            json_name: "apiKeyRequired",
            value: json!(true),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            expected: "Currently, ApiKeyRequired is not permitted for APIs with a \
                       protocol type of HTTP",
        },
        Row {
            case: "CreateRoute$RequestModels, a map, on an HTTP api",
            collection: "routes",
            base: json!({ "routeKey": "GET /params" }),
            id_member: "routeId",
            member: "RequestModels",
            json_name: "requestModels",
            value: json!({ "default": "Empty" }),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            expected: "Currently, RequestModels is not permitted for APIs with a \
                       protocol type of HTTP",
        },
        Row {
            case: "CreateIntegration$PassthroughBehavior, an enum, on an HTTP api",
            collection: "integrations",
            base: proxy_integration(),
            id_member: "integrationId",
            member: "PassthroughBehavior",
            json_name: "passthroughBehavior",
            value: json!("WHEN_NO_TEMPLATES"),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            expected: "Currently, PassthroughBehavior is not permitted for APIs with a \
                       protocol type of HTTP",
        },
        Row {
            case: "CreateStage$ClientCertificateId, a string, on an HTTP api",
            collection: "stages",
            base: json!({ "stageName": "certified" }),
            id_member: "stageName",
            member: "ClientCertificateId",
            json_name: "clientCertificateId",
            value: json!("abcd1234ef"),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            expected: "Currently, ClientCertificateId is not permitted for APIs with a \
                       protocol type of HTTP",
        },
        // --- the other direction of the restriction: the protocol token in the sentence
        // is the api's, so an HTTP-only member refused by a WebSocket api names WEBSOCKET.
        Row {
            case: "CreateIntegration$ConnectionId on a WebSocket api",
            collection: "integrations",
            base: proxy_integration(),
            id_member: "integrationId",
            member: "ConnectionId",
            json_name: "connectionId",
            value: json!("vpclink001"),
            supported_by: HTTP_PROTOCOL_TYPE,
            expected: "Currently, ConnectionId is not permitted for APIs with a protocol \
                       type of WEBSOCKET",
        },
        Row {
            case: "CreateIntegration$TlsConfig, a structure, on a WebSocket api",
            collection: "integrations",
            base: proxy_integration(),
            id_member: "integrationId",
            member: "TlsConfig",
            json_name: "tlsConfig",
            value: json!({ "serverNameToVerify": "upstream.example.com" }),
            supported_by: HTTP_PROTOCOL_TYPE,
            expected: "Currently, TlsConfig is not permitted for APIs with a protocol \
                       type of WEBSOCKET",
        },
    ]
}

/// The `ProtocolType` an api of `protocol_type` is NOT: the enum is `HTTP | WEBSOCKET`.
fn other_protocol(protocol_type: &str) -> &'static str {
    if protocol_type == WEBSOCKET_PROTOCOL_TYPE {
        HTTP_PROTOCOL_TYPE
    } else {
        WEBSOCKET_PROTOCOL_TYPE
    }
}

/// Create one api of `protocol_type` and answer its `ApiId`.
async fn create_api(
    emulator: &MultiServiceRouter,
    case: &str,
    name: &str,
    protocol_type: &str,
) -> String {
    let mut request = json!({ "name": name, "protocolType": protocol_type });
    if protocol_type == WEBSOCKET_PROTOCOL_TYPE {
        // "This property is required for WebSocket APIs"
        // (`CreateApi$RouteSelectionExpression`).
        request["routeSelectionExpression"] = json!("$request.body.action");
    }
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi for a {protocol_type} api, \
         got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// The `message` of a refusal, after checking it is the modelled 400.
fn refusal_message(case: &str, leg: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        status, 400,
        "case {case:?} ({leg}): expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "BadRequestException",
        "case {case:?} ({leg}): expected BadRequestException, got {body}"
    );
    as_str(body, "message")
}

#[tokio::test]
async fn a_protocol_restricted_member_is_refused_with_its_own_sentence() {
    let emulator = emulator();

    for (index, row) in rows().into_iter().enumerate() {
        let refusing = other_protocol(row.supported_by);
        // A fresh api per row, so the resource each leg creates is the only one there.
        let api = create_api(
            &emulator,
            row.case,
            &format!("member-message-{index}"),
            refusing,
        )
        .await;
        let collection = format!("/v2/apis/{api}/{}", row.collection);

        // The create leg: the base request plus exactly the restricted member.
        let mut request = row.base.clone();
        request[row.json_name] = row.value.clone();
        let (status, body) = call(&emulator, "POST", &collection, Some(request)).await;
        let message = refusal_message(row.case, "CreateX", status, &body);
        assert_eq!(
            message, row.expected,
            "case {:?} (CreateX): expected the member's own sentence {:?}, got {message:?}",
            row.case, row.expected
        );

        // The update leg: the same member patched onto a resource the refusing api
        // accepted, which must answer the SAME sentence — the words belong to the
        // member, not to the operation.
        let (status, created) = call(&emulator, "POST", &collection, Some(row.base.clone())).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected the base request to be accepted by a {refusing} api, \
             got {status} {created}",
            row.case
        );
        let resource = format!("{collection}/{}", as_str(&created, row.id_member));
        let mut patch = serde_json::Map::new();
        patch.insert(row.json_name.to_string(), row.value.clone());
        let (status, body) = call(&emulator, "PATCH", &resource, Some(Value::Object(patch))).await;
        let message = refusal_message(row.case, "UpdateX", status, &body);
        assert_eq!(
            message, row.expected,
            "case {:?} (UpdateX): expected the same sentence as the create, {:?}, got \
             {message:?}",
            row.case, row.expected
        );

        // The refused patch wrote nothing: the resource still carries no value for the
        // member it was refused for.
        let (status, stored) = call(&emulator, "GET", &resource, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the resource to read back, got {status} {stored}",
            row.case
        );
        assert_ne!(
            stored.get(row.json_name),
            Some(&row.value),
            "case {:?}: expected the refused patch to have stored no {} on the resource, \
             got {stored}",
            row.case,
            row.member
        );
    }
}

/// A whole child resource one protocol has no behaviour for takes the resource form of
/// the sentence, named in the plural as the service's own messages name it.
#[tokio::test]
async fn a_protocol_restricted_resource_is_refused_by_its_plural_name() {
    let emulator = emulator();
    let api = create_api(&emulator, "resource form", "resource-message", HTTP_PROTOCOL_TYPE).await;

    // (case, URI under the api, request, the sentence)
    let rows: Vec<(&str, String, Value, &str)> = vec![
        (
            "CreateModel on an HTTP api",
            format!("/v2/apis/{api}/models"),
            json!({
                "name": "ProbeModel",
                "contentType": "application/json",
                "schema": "{\"$schema\":\"http://json-schema.org/draft-04/schema#\",\
                            \"title\":\"Probe\",\"type\":\"object\"}"
            }),
            "Currently, Models are not permitted for APIs with a protocol type of HTTP",
        ),
        (
            "CreateRouteResponse on an HTTP api",
            // The route id never has to resolve: the protocol gate is weighed first, so
            // an HTTP api refuses the resource before it looks for the route.
            format!("/v2/apis/{api}/routes/norouteid/routeresponses"),
            json!({ "routeResponseKey": "$default" }),
            "Currently, Route responses are not permitted for APIs with a protocol type \
             of HTTP",
        ),
    ];

    for (case, uri, request, expected) in rows {
        let (status, body) = call(&emulator, "POST", &uri, Some(request)).await;
        let message = refusal_message(case, "create", status, &body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the resource sentence {expected:?}, got {message:?}"
        );
    }
}
