// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2's `ProtocolType`-conditional request
//! validation.
//!
//! The rule these cases assert: **a create request that configures something the model
//! documents as supported only for APIs of one `ProtocolType` is accepted on an api of
//! that protocol and refused 400 `BadRequestException` on an api of the other, before
//! anything is written** — so the refused api still holds nothing, and the resource AWS
//! has no behaviour for never exists.
//!
//! What the rule generalises over is the restricted thing itself, and the rows span it:
//!
//! * the direction of the restriction — "Supported only for WebSocket APIs" and
//!   "Supported only for HTTP APIs";
//! * its level — a whole child resource (a `Model`), a single member
//!   (`CreateRoute$RouteResponseSelectionExpression`), one VALUE of a member
//!   (`IntegrationType` `MOCK`, which the member's own doc marks WebSocket-only), a
//!   member of a NESTED shape (`RouteSettings$LoggingLevel`), and a member of the api
//!   itself (`CreateApi$CorsConfiguration`, which has no parent api to read);
//! * its type — a flag, a string, a map, a list, a structure;
//! * the operation it reaches — `CreateApi`, `CreateRoute`, `CreateIntegration`,
//!   `CreateStage`, `CreateModel`.
//!
//! The recorded case that filed the issue — `CreateModel` on an HTTP api, refused 400
//! with `GetModels` then reporting none — is ONE row of the table.
//!
//! Every member named here is one whose own documentation carries the clause; the
//! catalog of WebSocket-only `IntegrationType` values is read from the service rather
//! than copied, and pinned against the modelled enum.

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::ops::types::INTEGRATION_TYPE_VALUES;
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    HTTP_PROTOCOL_TYPE, WEBSOCKET_ONLY_INTEGRATION_TYPES, WEBSOCKET_PROTOCOL_TYPE,
};

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// How the api that was REFUSED is proved to hold nothing, and the one that accepted the
/// request is proved to hold the resource: through the collection reader, or — where
/// that reader is still a 501 stub — through the single-resource reader.
enum ReadBack {
    /// `GET <collection>` lists exactly the resources the api holds.
    Collection,
    /// `GET <collection>/<key>` finds the one resource, by the key the request named.
    Resource(&'static str),
}

/// One row: a create request carrying exactly one protocol-restricted thing.
struct Row {
    case: &'static str,
    /// The child collection the create posts to, under `/v2/apis/{ApiId}/`.
    collection: &'static str,
    /// The request body. Everything in it but the restricted thing is valid for BOTH
    /// protocols, so the only reason the other protocol refuses it is the gate.
    request: Value,
    /// The one `ProtocolType` whose APIs support what this request configures.
    supported_by: &'static str,
    read_back: ReadBack,
}

/// An `HTTP_PROXY` integration, the one integration type both protocols support, with
/// the members each protocol needs: a proxy to an upstream URL, and the
/// `PayloadFormatVersion` an HTTP api requires.
fn proxy_integration() -> Value {
    json!({
        "integrationType": "HTTP_PROXY",
        "integrationUri": "https://example.com/upstream",
        "payloadFormatVersion": "1.0"
    })
}

/// `base` with `member` set to `value`.
fn with(base: Value, member: &str, value: Value) -> Value {
    let mut request = base;
    request[member] = value;
    request
}

fn rows() -> Vec<Row> {
    vec![
        // --- a whole child resource: "Models are not permitted for APIs with a
        // protocol type of HTTP". Everything that selects a model is WebSocket-only.
        Row {
            case: "a Model, a resource only a WebSocket api holds",
            collection: "models",
            request: json!({
                "name": "ProbeModel",
                "contentType": "application/json",
                "schema": "{\"$schema\":\"http://json-schema.org/draft-04/schema#\",\
                            \"title\":\"Probe\",\"type\":\"object\"}"
            }),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            read_back: ReadBack::Collection,
        },
        // --- single members of a route, all "Supported only for WebSocket APIs".
        Row {
            case: "CreateRoute$RouteResponseSelectionExpression, a string member",
            collection: "routes",
            request: json!({
                "routeKey": "GET /rrse",
                "routeResponseSelectionExpression": "$default"
            }),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            read_back: ReadBack::Collection,
        },
        Row {
            case: "CreateRoute$ApiKeyRequired, a flag, set to true",
            collection: "routes",
            request: json!({ "routeKey": "GET /apikey", "apiKeyRequired": true }),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            read_back: ReadBack::Collection,
        },
        Row {
            case: "CreateRoute$ModelSelectionExpression, a string member",
            collection: "routes",
            request: json!({
                "routeKey": "GET /mse",
                "modelSelectionExpression": "$request.body.action"
            }),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            read_back: ReadBack::Collection,
        },
        Row {
            case: "CreateRoute$RequestModels, a non-empty map",
            collection: "routes",
            request: json!({
                "routeKey": "GET /models",
                "requestModels": { "default": "Empty" }
            }),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            read_back: ReadBack::Collection,
        },
        // NOT a row of this table: `CreateRoute$RequestParameters`. Its documentation
        // carries the same "Supported only for WebSocket APIs" clause, but the service
        // accepts the member on a route of an api of EITHER protocol and reports the map
        // back, so it is no protocol restriction at all; `route_request_parameter` owns what
        // a route may declare.
        // --- single members of an integration, both directions of the restriction.
        Row {
            case: "CreateIntegration$ContentHandlingStrategy, an enum member",
            collection: "integrations",
            request: with(
                proxy_integration(),
                "contentHandlingStrategy",
                json!("CONVERT_TO_TEXT"),
            ),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            read_back: ReadBack::Collection,
        },
        Row {
            case: "CreateIntegration$PassthroughBehavior, an enum member",
            collection: "integrations",
            request: with(
                proxy_integration(),
                "passthroughBehavior",
                json!("WHEN_NO_TEMPLATES"),
            ),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            read_back: ReadBack::Collection,
        },
        Row {
            case: "CreateIntegration$RequestTemplates, a non-empty map",
            collection: "integrations",
            request: with(
                proxy_integration(),
                "requestTemplates",
                json!({ "application/json": "{\"body\": $input.json('$')}" }),
            ),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            read_back: ReadBack::Collection,
        },
        Row {
            case: "Integration$TemplateSelectionExpression, a string member",
            collection: "integrations",
            request: with(
                proxy_integration(),
                "templateSelectionExpression",
                json!("$request.body.action"),
            ),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            read_back: ReadBack::Collection,
        },
        Row {
            case: "CreateIntegration$ConnectionId, supported only for HTTP APIs",
            collection: "integrations",
            request: with(proxy_integration(), "connectionId", json!("vpclink001")),
            supported_by: HTTP_PROTOCOL_TYPE,
            read_back: ReadBack::Collection,
        },
        Row {
            case: "CreateIntegration$IntegrationSubtype, supported only for HTTP APIs",
            collection: "integrations",
            // The base is an `AWS_PROXY` integration, not the shared `HTTP_PROXY` one:
            // "Supported only for HTTP API AWS_PROXY integrations"
            // (`CreateIntegration$IntegrationSubtype`), so a subtype on any other
            // integration type is refused whatever the protocol, and the only thing
            // left for this row to isolate is the PROTOCOL half of that clause.
            // `AWS_PROXY` itself carries no protocol restriction.
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function:handler",
                "payloadFormatVersion": "1.0",
                "integrationSubtype": "SQS-SendMessage"
            }),
            supported_by: HTTP_PROTOCOL_TYPE,
            read_back: ReadBack::Collection,
        },
        Row {
            case: "CreateIntegration$ResponseParameters, a non-empty nested map",
            collection: "integrations",
            request: with(
                proxy_integration(),
                "responseParameters",
                json!({ "200": { "append:header.x-trace": "$context.requestId" } }),
            ),
            supported_by: HTTP_PROTOCOL_TYPE,
            read_back: ReadBack::Collection,
        },
        Row {
            case: "CreateIntegration$TlsConfig, a structure member",
            collection: "integrations",
            request: with(
                proxy_integration(),
                "tlsConfig",
                json!({ "serverNameToVerify": "upstream.example.com" }),
            ),
            supported_by: HTTP_PROTOCOL_TYPE,
            read_back: ReadBack::Collection,
        },
        // --- a member of a NESTED shape, and the stage's own member. `GetStages` is
        // still a 501 stub, so these read back through `GetStage`.
        Row {
            case: "CreateStage$ClientCertificateId, a string member",
            collection: "stages",
            request: json!({ "stageName": "certified", "clientCertificateId": "abcd1234ef" }),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            read_back: ReadBack::Resource("certified"),
        },
        Row {
            case: "RouteSettings$DataTraceEnabled, nested under DefaultRouteSettings",
            collection: "stages",
            request: json!({
                "stageName": "traced",
                "defaultRouteSettings": { "dataTraceEnabled": true }
            }),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            read_back: ReadBack::Resource("traced"),
        },
        Row {
            case: "RouteSettings$LoggingLevel, nested under one RouteSettings entry",
            collection: "stages",
            request: json!({
                "stageName": "logged",
                "routeSettings": { "$connect": { "loggingLevel": "INFO" } }
            }),
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            read_back: ReadBack::Resource("logged"),
        },
    ]
}

/// The exact sentence the refusing api answers each row with.
///
/// The words belong to the restricted THING, not to the operation that received it: a
/// whole child resource is named in the plural, a member that configures behaviour the
/// other protocol does not implement is named as the request spells it and "is not
/// permitted", and a selection expression — a member that only selects a resource of the
/// other protocol — is named as a feature in prose that "is not necessary". Every row is
/// listed, so a row added without its sentence fails here rather than going unchecked.
fn expected_message(case: &str) -> &'static str {
    match case {
        "a Model, a resource only a WebSocket api holds" => {
            "Currently, Models are not permitted for APIs with a protocol type of HTTP"
        }
        "CreateRoute$RouteResponseSelectionExpression, a string member" => {
            "Currently, a route response selection expression is not necessary for APIs \
             with  a protocol type of HTTP"
        }
        "CreateRoute$ApiKeyRequired, a flag, set to true" => {
            "Currently, ApiKeyRequired is not permitted for APIs with a protocol type of \
             HTTP"
        }
        "CreateRoute$ModelSelectionExpression, a string member" => {
            "Currently, a model selection expression is not necessary for APIs with  a \
             protocol type of HTTP"
        }
        "CreateRoute$RequestModels, a non-empty map" => {
            "Currently, RequestModels is not permitted for APIs with a protocol type of \
             HTTP"
        }
        "CreateRoute$RequestParameters, a non-empty map of structures" => {
            "Currently, RequestParameters is not permitted for APIs with a protocol type \
             of HTTP"
        }
        "CreateIntegration$ContentHandlingStrategy, an enum member" => {
            "Currently, ContentHandlingStrategy is not permitted for APIs with a protocol \
             type of HTTP"
        }
        "CreateIntegration$PassthroughBehavior, an enum member" => {
            "Currently, PassthroughBehavior is not permitted for APIs with a protocol type \
             of HTTP"
        }
        "CreateIntegration$RequestTemplates, a non-empty map" => {
            "Currently, RequestTemplates is not permitted for APIs with a protocol type of \
             HTTP"
        }
        "Integration$TemplateSelectionExpression, a string member" => {
            "Currently, a template selection expression is not necessary for APIs with  a \
             protocol type of HTTP"
        }
        "CreateIntegration$ConnectionId, supported only for HTTP APIs" => {
            "Currently, ConnectionId is not permitted for APIs with a protocol type of \
             WEBSOCKET"
        }
        "CreateIntegration$IntegrationSubtype, supported only for HTTP APIs" => {
            "Currently, IntegrationSubtype is not permitted for APIs with a protocol type \
             of WEBSOCKET"
        }
        "CreateIntegration$ResponseParameters, a non-empty nested map" => {
            "Currently, ResponseParameters is not permitted for APIs with a protocol type \
             of WEBSOCKET"
        }
        "CreateIntegration$TlsConfig, a structure member" => {
            "Currently, TlsConfig is not permitted for APIs with a protocol type of \
             WEBSOCKET"
        }
        "CreateStage$ClientCertificateId, a string member" => {
            "Currently, ClientCertificateId is not permitted for APIs with a protocol type \
             of HTTP"
        }
        "RouteSettings$DataTraceEnabled, nested under DefaultRouteSettings" => {
            "Currently, RouteSettings DataTraceEnabled is not permitted for APIs with a \
             protocol type of HTTP"
        }
        "RouteSettings$LoggingLevel, nested under one RouteSettings entry" => {
            "Currently, RouteSettings LoggingLevel is not permitted for APIs with a \
             protocol type of HTTP"
        }
        other => panic!("case {other:?} carries no expected refusal sentence"),
    }
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
    emulator: &smithy_server_runtime::MultiServiceRouter,
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

/// The number of items `GET <uri>` lists.
async fn listed(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    uri: &str,
) -> usize {
    let (status, body) = call(emulator, "GET", uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GET {uri}, got {status} {body}"
    );
    body["items"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_else(|| panic!("case {case:?}: expected GET {uri} to report items, got {body}"))
}

#[tokio::test]
async fn a_protocol_restricted_request_is_accepted_only_on_an_api_of_its_own_protocol() {
    let emulator = emulator();

    for (index, row) in rows().into_iter().enumerate() {
        // A fresh pair of apis per row, so each api sees exactly one create and what it
        // holds afterwards is attributable to that one call.
        let supported = create_api(
            &emulator,
            row.case,
            &format!("gating-supported-{index}"),
            row.supported_by,
        )
        .await;
        let refused = create_api(
            &emulator,
            row.case,
            &format!("gating-refused-{index}"),
            other_protocol(row.supported_by),
        )
        .await;

        // An api of the supporting protocol takes the request.
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{supported}/{}", row.collection),
            Some(row.request.clone()),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 on a {} api, got {status} {created}",
            row.case, row.supported_by
        );

        // An api of the other protocol refuses it with the modelled error.
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{refused}/{}", row.collection),
            Some(row.request.clone()),
        )
        .await;
        assert_eq!(
            status,
            400,
            "case {:?}: expected 400 on a {} api, got {status} {body}",
            row.case,
            other_protocol(row.supported_by)
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {:?}: expected BadRequestException, got {body}",
            row.case
        );
        // The message is built from the request: it names what the request configured
        // and the protocol type of the api it was aimed at, in the sentence the
        // restricted thing itself carries (see `expected_message`).
        let message = as_str(&body, "message");
        assert_eq!(
            message,
            expected_message(row.case),
            "case {:?}: expected the restricted thing's own sentence, got {message:?}",
            row.case
        );

        // The accepted create stored exactly one resource; the refused one stored none.
        match row.read_back {
            ReadBack::Collection => {
                assert_eq!(
                    listed(
                        &emulator,
                        row.case,
                        &format!("/v2/apis/{supported}/{}", row.collection)
                    )
                    .await,
                    1,
                    "case {:?}: expected the {} api to hold the one resource it accepted",
                    row.case,
                    row.supported_by
                );
                assert_eq!(
                    listed(
                        &emulator,
                        row.case,
                        &format!("/v2/apis/{refused}/{}", row.collection)
                    )
                    .await,
                    0,
                    "case {:?}: expected the refused api to hold nothing",
                    row.case
                );
            }
            ReadBack::Resource(key) => {
                let (status, fetched) = call(
                    &emulator,
                    "GET",
                    &format!("/v2/apis/{supported}/{}/{key}", row.collection),
                    None,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: expected the accepted resource to be readable back, got \
                     {status} {fetched}",
                    row.case
                );
                let (status, body) = call(
                    &emulator,
                    "GET",
                    &format!("/v2/apis/{refused}/{}/{key}", row.collection),
                    None,
                )
                .await;
                assert_eq!(
                    status, 404,
                    "case {:?}: expected the refused api to hold nothing, got {status} {body}",
                    row.case
                );
                assert_eq!(
                    error_code(&body),
                    "NotFoundException",
                    "case {:?}: expected NotFoundException, got {body}",
                    row.case
                );
            }
        }
    }
}

/// Every value of the modelled `IntegrationType` enum, against the catalog of values the
/// member's own doc marks WebSocket-only: an HTTP api takes exactly the values outside
/// that catalog, and a WebSocket api takes them all.
#[tokio::test]
async fn an_http_api_takes_exactly_the_integration_types_that_are_not_websocket_only() {
    // The catalog is the service's own, not a copy: every value in it is a value of the
    // modelled enum, and the enum has values outside it (so the gate is not "refuse
    // everything").
    for value in WEBSOCKET_ONLY_INTEGRATION_TYPES {
        assert!(
            INTEGRATION_TYPE_VALUES.contains(value),
            "the WebSocket-only catalog names {value:?}, which is not a value of the \
             modelled IntegrationType enum {INTEGRATION_TYPE_VALUES:?}"
        );
    }
    assert!(
        INTEGRATION_TYPE_VALUES.len() > WEBSOCKET_ONLY_INTEGRATION_TYPES.len(),
        "expected the modelled enum {INTEGRATION_TYPE_VALUES:?} to carry values outside \
         the WebSocket-only catalog {WEBSOCKET_ONLY_INTEGRATION_TYPES:?}"
    );

    let emulator = emulator();
    let http = create_api(&emulator, "the http api", "types-http", HTTP_PROTOCOL_TYPE).await;
    let socket = create_api(
        &emulator,
        "the websocket api",
        "types-socket",
        WEBSOCKET_PROTOCOL_TYPE,
    )
    .await;

    let mut accepted_by_http = 0usize;
    for integration_type in INTEGRATION_TYPE_VALUES {
        let websocket_only = WEBSOCKET_ONLY_INTEGRATION_TYPES.contains(integration_type);
        let mut request = json!({
            "integrationType": integration_type,
            "payloadFormatVersion": "1.0"
        });
        // "MOCK: for integrating the route or method request with API Gateway as a
        // 'loopback' endpoint without invoking any backend", so it is the one type that
        // names no endpoint; every other type needs one. `AWS` and `AWS_PROXY` integrate
        // "with an AWS service action", so their URI names the resource that action runs
        // on — an ARN — while `HTTP`/`HTTP_PROXY` take "a fully-qualified URL"
        // (`Integration$IntegrationUri`).
        if *integration_type != "MOCK" {
            request["integrationUri"] = if matches!(*integration_type, "AWS" | "AWS_PROXY") {
                json!("arn:aws:lambda:eu-west-1:000000000000:function:handler")
            } else {
                json!("https://example.com/upstream")
            };
        }

        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{http}/integrations"),
            Some(request.clone()),
        )
        .await;
        if websocket_only {
            assert_eq!(
                status, 400,
                "IntegrationType {integration_type:?} is documented WebSocket-only, so an \
                 HTTP api must refuse it: got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "IntegrationType {integration_type:?}: expected BadRequestException, got {body}"
            );
        } else {
            assert_eq!(
                status, 201,
                "IntegrationType {integration_type:?} carries no protocol restriction, so an \
                 HTTP api must accept it: got {status} {body}"
            );
            accepted_by_http += 1;
        }

        // A WebSocket api supports every value of the enum.
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{socket}/integrations"),
            Some(request),
        )
        .await;
        assert_eq!(
            status, 201,
            "IntegrationType {integration_type:?}: a WebSocket api must accept every \
             integration type, got {status} {body}"
        );
    }

    // The HTTP api holds exactly the integrations it accepted, and the refusals stored
    // nothing.
    assert_eq!(
        listed(&emulator, "the http api", &format!("/v2/apis/{http}/integrations")).await,
        accepted_by_http,
        "expected the HTTP api to hold only the integration types it accepted"
    );
    assert_eq!(
        listed(
            &emulator,
            "the websocket api",
            &format!("/v2/apis/{socket}/integrations")
        )
        .await,
        INTEGRATION_TYPE_VALUES.len(),
        "expected the WebSocket api to hold one integration per modelled type"
    );
}

/// `CreateApi` has no parent api to read, so the protocol it gates against is the
/// `ProtocolType` of the request itself.
///
/// `ApiKeySelectionExpression` and `DisableSchemaValidation` carry the same clause and
/// are deliberately absent: the modelled `Api` shape reports both for an api of either
/// protocol, so neither names something an api could not apply (see the note in
/// `create_api.rs`). So are the quick-create members (`Target`, `RouteKey`,
/// `CredentialsArn`), which API Gateway accepts on an api of either protocol and applies
/// only where quick create runs — the table in `quick_create_is_http_only.rs` holds that
/// rule.
#[tokio::test]
async fn an_api_s_own_protocol_restricted_members_are_gated_by_its_protocol_type() {
    // (case, member, value, the ProtocolType that supports it)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "CreateApi$CorsConfiguration, supported only for HTTP APIs",
            "corsConfiguration",
            json!({ "allowOrigins": ["https://example.com"] }),
            HTTP_PROTOCOL_TYPE,
        ),
    ];

    for (index, (case, member, value, supported_by)) in rows.into_iter().enumerate() {
        // One emulator per row, so `GetApis` lists exactly this row's apis.
        let emulator = emulator();

        for protocol_type in [supported_by, other_protocol(supported_by)] {
            let mut request = json!({
                "name": format!("api-gating-{index}-{protocol_type}"),
                "protocolType": protocol_type
            });
            if protocol_type == WEBSOCKET_PROTOCOL_TYPE {
                request["routeSelectionExpression"] = json!("$request.body.action");
            }
            request[member] = value.clone();

            let (status, body) = call(&emulator, "POST", "/v2/apis", Some(request)).await;
            if protocol_type == supported_by {
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected 201 on a {protocol_type} api, got {status} {body}"
                );
                assert_eq!(
                    as_str(&body, "protocolType"),
                    protocol_type,
                    "case {case:?}: expected the stored api to carry its own protocol type, \
                     got {body}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400 on a {protocol_type} api, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "BadRequestException",
                    "case {case:?}: expected BadRequestException, got {body}"
                );
            }
        }

        // Exactly one api was minted: the refused CreateApi stored nothing.
        assert_eq!(
            listed(&emulator, case, "/v2/apis").await,
            1,
            "case {case:?}: expected only the accepted CreateApi to have minted an api"
        );
    }
}
