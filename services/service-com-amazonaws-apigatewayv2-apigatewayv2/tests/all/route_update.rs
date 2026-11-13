// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateRoute`.
//!
//! Two rules are asserted here, each over a table rather than over one request:
//!
//! * `PATCH /v2/apis/{ApiId}/routes/{RouteId}` is a patch of the STORED route: a member
//!   the request carries replaces the stored value, a member it omits leaves the stored
//!   value as it was, and what the call reports is what a later `GetRoute` reports.
//! * A route whose `AuthorizationType` hands the decision to an `Authorizer` resource
//!   (`CUSTOM` or `JWT`) must be associated with one — `Route$AuthorizerId` is what
//!   names it — so a request that leaves the merged record without one is refused with
//!   the modelled `BadRequestException` (400) and changes nothing. The same requirement
//!   holds for `CreateRoute`, and both operations are driven here.
//!
//! The rows vary what each rule generalises over: the member's type (string, boolean,
//! list, map), the number of members patched (none, one, several), list and map size
//! (shorter, empty, different keys), the route key, every value of the modelled
//! `AuthorizationType` enum, and the three forms a request can take on `AuthorizerId`
//! (a minted id, absent, empty). The recorded case that filed the issue — an update to
//! `JWT` with no authorizer — is one row of the second table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

fn http_api(name: &str) -> Value {
    json!({ "name": name, "protocolType": "HTTP" })
}

/// A well-formed Lambda authorizer URI, the form `AuthorizerUri` documents: "For REQUEST
/// authorizers, this must be a well-formed Lambda function URI, for example,
/// arn:aws:apigateway:{region}:lambda:path/2015-03-31/functions/{function-arn}/invocations".
fn lambda_uri() -> String {
    "arn:aws:apigateway:eu-west-1:lambda:path/2015-03-31/functions/\
     arn:aws:lambda:eu-west-1:123456789012:function:route-authorizer/invocations"
        .to_string()
}

/// Create one api and answer its `ApiId`.
async fn create_api(emulator: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Create one route under `api_id` and answer its `RouteId`.
async fn create_route(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: &Value,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateRoute, got {status} {created}"
    );
    as_str(&created, "routeId")
}

/// Create one `REQUEST` authorizer under `api_id` and answer its `AuthorizerId`.
///
/// The id a route names here is one the service actually minted, not a literal.
/// `AuthorizerPayloadFormatVersion` is "Required for HTTP API Lambda authorizers", so
/// it is sent only for an HTTP api.
async fn create_authorizer(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    protocol: &str,
) -> String {
    let mut request = json!({
        "name": "route-authorizer",
        "authorizerType": "REQUEST",
        "identitySource": ["$request.header.Authorization"],
        "authorizerUri": lambda_uri()
    });
    if protocol == "HTTP" {
        request["authorizerPayloadFormatVersion"] = json!("2.0");
    }
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateAuthorizer, got {status} {created}"
    );
    as_str(&created, "authorizerId")
}

/// Create one MOCK integration under `api_id` and answer its `IntegrationId`.
///
/// A route's `Target` has to address an integration its OWN api holds, so a test that
/// stores a target names an integration it created here rather than a literal id.
/// `MOCK` "lets API Gateway return a response without sending the request further to
/// the backend", so it needs no endpoint of its own.
async fn create_integration(emulator: &MultiServiceRouter, case: &str, api_id: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({ "integrationType": "MOCK" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateIntegration, got {status} {created}"
    );
    as_str(&created, "integrationId")
}

/// Read one route back and answer `(status, body)`.
async fn get_route(
    emulator: &MultiServiceRouter,
    api_id: &str,
    route_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/routes/{route_id}"),
        None,
    )
    .await
}

/// Patch one route and answer `(status, body)`.
async fn update_route(
    emulator: &MultiServiceRouter,
    api_id: &str,
    route_id: &str,
    patch: &Value,
) -> (u16, Value) {
    call(
        emulator,
        "PATCH",
        &format!("/v2/apis/{api_id}/routes/{route_id}"),
        Some(patch.clone()),
    )
    .await
}

/// The record `created` becomes once `patch` is applied: every member the patch names
/// takes the patch's value, and every other member keeps the stored one. That is the
/// patch rule itself, written once, so each row only has to supply the patch.
fn patched_record(created: &Value, patch: &Value) -> Value {
    let mut expected = created.clone();
    for (member, value) in patch.as_object().expect("the patch is a JSON object") {
        expected[member] = value.clone();
    }
    expected
}

/// The route every patch row starts from: a WebSocket route carrying every member the
/// service stores, so a row can replace any one of them.
///
/// Its key is `$connect`, because `RequestParameters` is accepted only on the route that
/// serves the connection request (`crate::state::route_request_parameters`) and this route
/// carries every member.
fn base_route(integration_id: &str, authorizer_id: &str) -> Value {
    json!({
        "routeKey": "$connect",
        "target": format!("integrations/{integration_id}"),
        "authorizationType": "CUSTOM",
        "authorizerId": authorizer_id,
        "apiKeyRequired": true,
        "authorizationScopes": ["scope/one", "scope/two"],
        "operationName": "sendMessage",
        "modelSelectionExpression": "$request.body.action",
        "routeResponseSelectionExpression": "$default",
        "requestModels": { "default": "Empty" },
        "requestParameters": { "route.request.header.x-trace": { "required": true } }
    })
}

/// One patch row: the request body sent to `UpdateRoute`.
struct PatchRow {
    case: &'static str,
    patch: Value,
}

fn patch_rows() -> Vec<PatchRow> {
    vec![
        PatchRow {
            case: "one string member",
            patch: json!({ "operationName": "renamedOperation" }),
        },
        PatchRow {
            case: "one boolean member, flipped",
            patch: json!({ "apiKeyRequired": false }),
        },
        PatchRow {
            case: "a shorter list replaces the stored list",
            patch: json!({ "authorizationScopes": ["scope/only"] }),
        },
        PatchRow {
            case: "an empty list clears the stored list",
            patch: json!({ "authorizationScopes": [] }),
        },
        PatchRow {
            case: "a map with different keys replaces the stored map",
            patch: json!({
                "requestParameters": {
                    "route.request.querystring.token": { "required": false },
                    "route.request.body.action": { "required": true }
                }
            }),
        },
        PatchRow {
            case: "an empty map clears the stored map",
            patch: json!({ "requestModels": {} }),
        },
        PatchRow {
            case: "the route key moves to a key no other route holds",
            // A route that moves OFF `$connect` gives up its request parameters in the same
            // patch: only that route may declare any, so the merged record could not carry
            // both.
            patch: json!({ "routeKey": "broadcast", "requestParameters": {} }),
        },
        PatchRow {
            case: "the route key is re-sent unchanged",
            patch: json!({ "routeKey": "$connect" }),
        },
        PatchRow {
            case: "the target",
            // The id is substituted below: a target has to name an integration the api
            // actually holds.
            patch: json!({ "target": "integrations/substituted" }),
        },
        PatchRow {
            case: "the authorization type drops to open access",
            patch: json!({ "authorizationType": "NONE" }),
        },
        PatchRow {
            case: "both selection expressions",
            patch: json!({
                "modelSelectionExpression": "$request.body.type",
                "routeResponseSelectionExpression": "$disconnect"
            }),
        },
        PatchRow {
            case: "several members of different types at once",
            patch: json!({
                "routeKey": "$disconnect",
                "apiKeyRequired": false,
                "operationName": "disconnect",
                "authorizationScopes": ["scope/three"],
                "requestModels": { "default": "Other", "extra": "Empty" },
                "requestParameters": {}
            }),
        },
        PatchRow {
            case: "a patch naming no member leaves the record as it is",
            patch: json!({}),
        },
    ]
}

#[tokio::test]
async fn update_route_replaces_the_members_it_names_and_keeps_the_rest() {
    let emulator = emulator();

    for (index, row) in patch_rows().into_iter().enumerate() {
        // One api per row, so a row's route key can move freely without colliding with
        // another row's route.
        let api_id = create_api(
            &emulator,
            row.case,
            &websocket_api(&format!("route-patch-{index}")),
        )
        .await;
        let authorizer_id = create_authorizer(&emulator, row.case, &api_id, "WEBSOCKET").await;
        // Two integrations: the one the route starts out targeting, and the one the
        // target-patch row moves it to.
        let integration_id = create_integration(&emulator, row.case, &api_id).await;
        let other_integration_id = create_integration(&emulator, row.case, &api_id).await;
        let route_id = create_route(
            &emulator,
            row.case,
            &api_id,
            &base_route(&integration_id, &authorizer_id),
        )
        .await;
        let (status, created) = get_route(&emulator, &api_id, &route_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetRoute on the fresh route, got {status} {created}",
            row.case
        );

        let mut patch = row.patch.clone();
        if patch.get("target").is_some() {
            patch["target"] = json!(format!("integrations/{other_integration_id}"));
        }
        let (status, updated) = update_route(&emulator, &api_id, &route_id, &patch).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from UpdateRoute, got {status} {updated}",
            row.case
        );
        let expected = patched_record(&created, &patch);
        assert_eq!(
            updated, expected,
            "case {:?}: UpdateRoute must report the patched record; expected {expected}, got {updated}",
            row.case
        );

        // The write is what a later caller sees, not just what this call answered.
        let (status, fetched) = get_route(&emulator, &api_id, &route_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetRoute after the patch, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            fetched, expected,
            "case {:?}: GetRoute must report what UpdateRoute stored; expected {expected}, got {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "routeId"),
            route_id,
            "case {:?}: the patch must not change the RouteId, got {fetched}",
            row.case
        );
    }
}

/// The three forms a request can take on `AuthorizerId`: a minted id, no member at all,
/// and an empty string (`Id` carries no length constraint, so the boundary validator
/// passes an empty one through to the handler).
enum AuthorizerIdForm {
    Minted,
    Absent,
    Empty,
}

impl AuthorizerIdForm {
    fn label(&self) -> &'static str {
        match self {
            Self::Minted => "naming the authorizer the api holds",
            Self::Absent => "naming no authorizer",
            Self::Empty => "with an empty AuthorizerId",
        }
    }

    /// Does the request leave the record associated with an authorizer?
    fn names_authorizer(&self) -> bool {
        matches!(self, Self::Minted)
    }

    fn value(&self, minted: &str) -> Option<String> {
        match self {
            Self::Minted => Some(minted.to_string()),
            Self::Absent => None,
            Self::Empty => Some(String::new()),
        }
    }
}

#[tokio::test]
async fn a_route_authorized_by_an_authorizer_is_refused_unless_it_names_one() {
    // The table is the MODEL's own catalog of `AuthorizationType` values
    // (`smithy_query(query="shape", shape="AuthorizationType")`:
    // `AWS_IAM | CUSTOM | JWT | NONE`), and the expectation is computed from the
    // service's list of the values that delegate to an `Authorizer` resource — so a
    // value added to either list is covered here without editing this test.
    use service_com_amazonaws_apigatewayv2_apigatewayv2::ops::types::AUTHORIZATION_TYPE_VALUES;
    use service_com_amazonaws_apigatewayv2_apigatewayv2::state::AUTHORIZER_BACKED_AUTHORIZATION_TYPES;

    let emulator = emulator();
    // An HTTP api: "For HTTP APIs, valid values are NONE …, JWT …, AWS_IAM … and
    // CUSTOM", so every value of the catalog is one this api's routes may carry.
    let api_id = create_api(&emulator, "authorization table", &http_api("route-auth-host")).await;
    let minted = create_authorizer(&emulator, "authorization table", &api_id, "HTTP").await;

    for authorization_type in AUTHORIZATION_TYPE_VALUES {
        for form in [
            AuthorizerIdForm::Minted,
            AuthorizerIdForm::Absent,
            AuthorizerIdForm::Empty,
        ] {
            let case = format!("{authorization_type} {}", form.label());
            let accepted = !AUTHORIZER_BACKED_AUTHORIZATION_TYPES.contains(authorization_type)
                || form.names_authorizer();

            let mut members = json!({ "authorizationType": authorization_type });
            if let Some(id) = form.value(&minted) {
                members["authorizerId"] = json!(id);
            }

            // (1) CreateRoute: the same requirement, on the create.
            let route_key = format!("GET /{authorization_type}-{}", form.label().len());
            let mut create = members.clone();
            create["routeKey"] = json!(route_key);
            let (status, created) = call(
                &emulator,
                "POST",
                &format!("/v2/apis/{api_id}/routes"),
                Some(create),
            )
            .await;
            if accepted {
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected 201 from CreateRoute, got {status} {created}"
                );
                assert_eq!(
                    as_str(&created, "authorizationType"),
                    *authorization_type,
                    "case {case:?}: CreateRoute must store the AuthorizationType, got {created}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400 from CreateRoute, got {status} {created}"
                );
                assert_eq!(
                    error_code(&created),
                    "BadRequestException",
                    "case {case:?}: expected BadRequestException from CreateRoute, got {created}"
                );
                // A refused create stores nothing: the key it asked for is on no route.
                let (_, routes) =
                    call(&emulator, "GET", &format!("/v2/apis/{api_id}/routes"), None).await;
                let keys: Vec<String> = routes["items"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|route| as_str(route, "routeKey"))
                    .collect();
                assert!(
                    !keys.contains(&route_key),
                    "case {case:?}: a refused CreateRoute must store no route, got {routes}"
                );
            }

            // (2) UpdateRoute: the same requirement, reached as a patch of an open
            // route that names no authorizer.
            let open_route = json!({ "routeKey": format!("PUT /{route_key}") });
            let route_id = create_route(&emulator, &case, &api_id, &open_route).await;
            let (_, before) = get_route(&emulator, &api_id, &route_id).await;
            let (status, updated) = update_route(&emulator, &api_id, &route_id, &members).await;
            if accepted {
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected 201 from UpdateRoute, got {status} {updated}"
                );
                assert_eq!(
                    as_str(&updated, "authorizationType"),
                    *authorization_type,
                    "case {case:?}: UpdateRoute must store the AuthorizationType, got {updated}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400 from UpdateRoute, got {status} {updated}"
                );
                assert_eq!(
                    error_code(&updated),
                    "BadRequestException",
                    "case {case:?}: expected BadRequestException from UpdateRoute, got {updated}"
                );
                let (_, after) = get_route(&emulator, &api_id, &route_id).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused UpdateRoute must leave the record as it was; expected {before}, got {after}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_route_that_already_names_an_authorizer_may_switch_between_authorizer_backed_types() {
    use service_com_amazonaws_apigatewayv2_apigatewayv2::state::AUTHORIZER_BACKED_AUTHORIZATION_TYPES;

    let emulator = emulator();
    let api_id = create_api(&emulator, "switch", &http_api("route-auth-switch")).await;
    let authorizer_id = create_authorizer(&emulator, "switch", &api_id, "HTTP").await;

    // The requirement is on the MERGED record, so a route already associated with an
    // authorizer may move to any authorizer-backed type without resending the id.
    for authorization_type in AUTHORIZER_BACKED_AUTHORIZATION_TYPES {
        let case = format!("switch to {authorization_type}");
        let route_id = create_route(
            &emulator,
            &case,
            &api_id,
            &json!({
                "routeKey": format!("GET /switch-{authorization_type}"),
                "authorizationType": "CUSTOM",
                "authorizerId": authorizer_id
            }),
        )
        .await;

        let (status, updated) = update_route(
            &emulator,
            &api_id,
            &route_id,
            &json!({ "authorizationType": authorization_type }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from UpdateRoute, got {status} {updated}"
        );
        assert_eq!(
            as_str(&updated, "authorizerId"),
            authorizer_id,
            "case {case:?}: the stored AuthorizerId must survive a patch that omits it, got {updated}"
        );
        assert_eq!(
            as_str(&updated, "authorizationType"),
            *authorization_type,
            "case {case:?}: wrong AuthorizationType, got {updated}"
        );
    }
}

#[tokio::test]
async fn an_update_cannot_move_a_route_onto_a_key_another_route_already_holds() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "conflict", &http_api("route-conflict-host")).await;

    // Three keys, so the conflict is asserted against a key that is neither the first
    // nor the route's own.
    let keys = ["GET /items", "POST /items", "DELETE /items/{id}"];
    let mut route_ids = Vec::new();
    for key in keys {
        route_ids.push(create_route(&emulator, key, &api_id, &json!({ "routeKey": key })).await);
    }

    for (index, route_id) in route_ids.iter().enumerate() {
        for (other, taken) in keys.iter().enumerate() {
            let case = format!("{} -> {taken}", keys[index]);
            let (status, body) =
                update_route(&emulator, &api_id, route_id, &json!({ "routeKey": taken })).await;
            if other == index {
                // Its own key is not a conflict with itself.
                assert_eq!(
                    status, 201,
                    "case {case:?}: re-sending a route's own key must be accepted, got {status} {body}"
                );
            } else {
                assert_eq!(
                    status, 409,
                    "case {case:?}: expected 409 for a key another route holds, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ConflictException",
                    "case {case:?}: expected ConflictException, got {body}"
                );
            }
        }
        // Whatever was refused, the route still carries its own key.
        let (_, fetched) = get_route(&emulator, &api_id, route_id).await;
        assert_eq!(
            as_str(&fetched, "routeKey"),
            keys[index],
            "case {:?}: a refused key change must leave the stored RouteKey, got {fetched}",
            keys[index]
        );
    }
}

/// The `RouteKey` of a route API Gateway manages is patched like any other route's, and
/// the route stays the service's: `UpdateApi$RouteKey` offers the very same change as a
/// member of the api ("this value replaces the route key of the quick create route"), so
/// the key is configuration rather than part of the managed route's identity, while
/// `ApiGatewayManaged` is not an input of `UpdateRoute` at all.
///
/// The rows vary the form of the key the managed route is moved onto: a method-and-path
/// key, a catch-all method key, a greedy path variable, and back onto the `$default`
/// catch-all it was minted with.
#[tokio::test]
async fn the_route_key_of_an_api_gateway_managed_route_is_patched_like_any_other() {
    let emulator = emulator();

    // Quick create mints the `$default` route and manages it.
    let api_id = create_api(
        &emulator,
        "quick create",
        &json!({
            "name": "route-managed-host",
            "protocolType": "HTTP",
            "target": "https://example.com/upstream"
        }),
    )
    .await;
    let (_, routes) = call(&emulator, "GET", &format!("/v2/apis/{api_id}/routes"), None).await;
    let managed = routes["items"]
        .as_array()
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        managed["apiGatewayManaged"].as_bool(),
        Some(true),
        "quick create must mint an API-Gateway-managed route, got {routes}"
    );
    let route_id = as_str(&managed, "routeId");
    let target = as_str(&managed, "target");

    for case in ["ANY /managed", "GET /items", "ANY /{proxy+}", "$default"] {
        let (status, updated) =
            update_route(&emulator, &api_id, &route_id, &json!({ "routeKey": case })).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 moving the managed route onto the key, got \
             {status} {updated}"
        );
        assert_eq!(
            as_str(&updated, "routeKey"),
            case,
            "case {case:?}: expected the patched RouteKey, got {updated}"
        );
        assert_eq!(
            updated.get("apiGatewayManaged"),
            Some(&json!(true)),
            "case {case:?}: a patched managed route is still the service's, got {updated}"
        );
        assert_eq!(
            as_str(&updated, "target"),
            target,
            "case {case:?}: a key patch leaves the managed route's Target as it was, got \
             {updated}"
        );

        // What the patch reported is what a later read reports, under the same id.
        let (status, fetched) = get_route(&emulator, &api_id, &route_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetRoute, got {status} {fetched}"
        );
        assert_eq!(
            fetched, updated,
            "case {case:?}: GetRoute must report the patched managed route, got {fetched}"
        );
    }

    // Every other member of a managed route is patchable too, and re-sending the key it
    // already carries changes nothing.
    let (status, updated) = update_route(
        &emulator,
        &api_id,
        &route_id,
        &json!({ "routeKey": "$default", "operationName": "catchAll" }),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 patching a managed route's other members, got {status} {updated}"
    );
    assert_eq!(
        as_str(&updated, "operationName"),
        "catchAll",
        "the patched OperationName must be stored, got {updated}"
    );
    assert_eq!(
        as_str(&updated, "routeKey"),
        "$default",
        "re-sending the stored key changes nothing, got {updated}"
    );
}

/// One WebSocket-only member, in the form a request spells it, with a value that
/// CONFIGURES it and a value that configures nothing.
///
/// The five members `CreateRoute`/`UpdateRoute` document as "Supported only for
/// WebSocket APIs" (`smithy_query(query="operation", operation="UpdateRoute")` plus
/// each member's own doc). `ApiKeyRequired` sent `false`, and an empty map, ask for
/// nothing: `false` is the value a route carries either way.
struct WebSocketOnlyMember {
    member: &'static str,
    configuring: Value,
    /// The same member set to the value it would carry anyway, or `None` where the
    /// member has no such value.
    inert: Option<Value>,
    /// The key the route on the SUPPORTING api is created with, where the member needs a
    /// particular one: `RequestParameters` is accepted only on `$connect`
    /// (`crate::state::route_request_parameters`). `None` takes the generic key below.
    route_key: Option<&'static str>,
}

fn websocket_only_members() -> Vec<WebSocketOnlyMember> {
    vec![
        WebSocketOnlyMember {
            member: "apiKeyRequired",
            configuring: json!(true),
            inert: Some(json!(false)),
            route_key: None,
        },
        WebSocketOnlyMember {
            member: "modelSelectionExpression",
            configuring: json!("$request.body.action"),
            inert: None,
            route_key: None,
        },
        WebSocketOnlyMember {
            member: "requestModels",
            configuring: json!({ "default": "Empty" }),
            inert: Some(json!({})),
            route_key: None,
        },
        // NOT in this table: `requestParameters`. The service accepts the member on a route
        // of an api of either protocol (`create_route`), so a patch that configures it on an
        // HTTP api's route is not refused; `route_request_parameter` owns what a route may
        // declare.
        WebSocketOnlyMember {
            member: "routeResponseSelectionExpression",
            configuring: json!("$default"),
            inert: None,
            route_key: None,
        },
    ]
}

#[tokio::test]
async fn a_patch_configuring_a_websocket_only_member_is_refused_on_an_http_api() {
    let emulator = emulator();
    let http_id = create_api(&emulator, "protocol gate", &http_api("route-patch-http")).await;
    let socket_id = create_api(
        &emulator,
        "protocol gate",
        &websocket_api("route-patch-socket"),
    )
    .await;

    for restricted in websocket_only_members() {
        let case = restricted.member;
        let http_route = create_route(
            &emulator,
            case,
            &http_id,
            &json!({ "routeKey": format!("GET /{case}") }),
        )
        .await;
        let socket_route = create_route(
            &emulator,
            case,
            &socket_id,
            &json!({
                "routeKey": restricted
                    .route_key
                    .map_or_else(|| format!("socket-{case}"), str::to_string)
            }),
        )
        .await;

        // On an api of the supporting protocol the same patch is accepted.
        let patch = json!({ case: restricted.configuring });
        let (status, updated) = update_route(&emulator, &socket_id, &socket_route, &patch).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a WebSocket api supports the member; expected 201, got {status} {updated}"
        );

        // On an HTTP api it is refused, and the route is left as it was.
        let (_, before) = get_route(&emulator, &http_id, &http_route).await;
        let (status, body) = update_route(&emulator, &http_id, &http_route, &patch).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 patching a WebSocket-only member on an HTTP api, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        let (_, after) = get_route(&emulator, &http_id, &http_route).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused patch must change nothing; expected {before}, got {after}"
        );

        // The value that configures nothing is accepted on either protocol.
        if let Some(inert) = restricted.inert {
            let (status, updated) =
                update_route(&emulator, &http_id, &http_route, &json!({ case: inert })).await;
            assert_eq!(
                status, 201,
                "case {case:?}: a member set to the value the route carries anyway asks for nothing; expected 201, got {status} {updated}"
            );
        }
    }
}

#[tokio::test]
async fn update_route_on_an_api_or_route_the_store_does_not_hold_is_not_found() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "lookup", &http_api("route-update-lookup")).await;
    let route_id = create_route(
        &emulator,
        "lookup",
        &api_id,
        &json!({ "routeKey": "GET /items" }),
    )
    .await;
    let other_api_id = create_api(&emulator, "lookup other", &http_api("route-update-other")).await;

    // Each row addresses a route the store does not hold, through a different level of
    // the path: an unknown api, an unknown route, and a route of ANOTHER api.
    let rows = [
        ("an unknown api", format!("/v2/apis/nosuchapi0/routes/{route_id}")),
        (
            "an unknown route",
            format!("/v2/apis/{api_id}/routes/nosuchrte0"),
        ),
        (
            "a route that belongs to another api",
            format!("/v2/apis/{other_api_id}/routes/{route_id}"),
        ),
    ];
    for (case, uri) in rows {
        let (status, body) = call(
            &emulator,
            "PATCH",
            &uri,
            Some(json!({ "operationName": "renamed" })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from UpdateRoute, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // The route the rows named is untouched.
    let (status, fetched) = get_route(&emulator, &api_id, &route_id).await;
    assert_eq!(
        status, 200,
        "expected 200 from GetRoute, got {status} {fetched}"
    );
    assert!(
        fetched.get("operationName").is_none(),
        "a refused UpdateRoute must store nothing, got {fetched}"
    );
}
