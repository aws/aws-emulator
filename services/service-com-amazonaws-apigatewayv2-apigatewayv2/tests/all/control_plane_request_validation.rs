// SPDX-License-Identifier: CC0-1.0

//! What the ApiGatewayV2 control plane ACCEPTS and what it REFUSES, for the members whose
//! value rules a request is judged by before anything is stored.
//!
//! Every rule here is one the recorded service applies, each over a class of values rather
//! than over the one value a recording happened to carry, so each table's rows differ along
//! what its rule generalises over:
//!
//! * **a CORS configuration's `AllowOrigins` entry is an ORIGIN** — the wildcard, or the
//!   `scheme://host[:port]` serialization RFC 6454 §6.1 gives one — and a value of any other
//!   form is refused `400 BadRequestException` with `Invalid format for origin <origin>`;
//! * **its `AllowMethods` entry is an HTTP method the gateway serves**, or the wildcard, and
//!   any other value is refused with `Method <method> is not supported`;
//! * **its `MaxAge` is bounded by NEITHER end of the range its shape declares**: whatever
//!   integer it carries is stored and reported back;
//! * **a route's `RequestParameters` are accepted on an api of EITHER protocol** and read
//!   back through `GetRoute`, with the entries addressable through
//!   `DeleteRouteRequestParameter`;
//! * **a JWT authorizer's `Issuer` names an identity provider whose OpenID discovery
//!   document can be fetched**: a configuration carrying none, one carrying no issuer, one
//!   whose issuer is no https URL, and one whose issuer this emulator SERVES something other
//!   than that document for are each refused, while a provider outside this emulator is
//!   accepted as given;
//! * **`GetModelTemplate` generates a template only for a schema declaring a COMPOSITE
//!   type**, and names the data type it will not generate one for;
//! * **a portal's `IncludedPortalProductArns` are well-formed ARNs of portal products the
//!   account HOLDS**, refused `Invalid ARN. ARN is not well formed <arn>` and
//!   `Product not found in account <account> with id <id>` respectively;
//! * **a portal product's sharing policy is a resource-based IAM policy document**, refused
//!   `The specified policy was invalid` otherwise.
//!
//! The other two rules of the same class are pinned by the suites that already own their
//! operations: `product_rest_endpoint_page_generation` for a page whose REST endpoint the
//! account does not have, and `vpc_link_subnet_resolution` for a VPC link naming a subnet
//! Amazon EC2 holds none of.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The error this API refuses a request it cannot accept with ("The request is not valid,
/// for example, the input is incomplete or incorrect").
const BAD_REQUEST: &str = "BadRequestException";

/// Create one HTTP api carrying `cors`, and answer `(status, body)`.
async fn create_api_with_cors(
    emulator: &MultiServiceRouter,
    name: &str,
    cors: Value,
) -> (u16, Value) {
    call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": name,
            "protocolType": "HTTP",
            "corsConfiguration": cors
        })),
    )
    .await
}

/// Create one plain HTTP api and answer its `ApiId`.
async fn http_api(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateApi for {name:?}, got {status} {created}"
    );
    as_str(&created, "apiId")
}

#[tokio::test]
async fn an_allowed_origin_is_the_wildcard_or_an_origin_serialization() {
    let emulator = emulator();

    // (case, the origin, accepted?) — the rows span the wildcard, every part of an origin
    // serialization (scheme, host, port), a host carrying the wildcard character, an IPv6
    // literal, and the ways a value stops being an origin: no scheme, no host, a path, a
    // query, a fragment, a non-numeric port, inner whitespace. The recorded create that
    // filed the issue carried `not-a-url`, which is ONE row.
    let rows: &[(&str, &str, bool)] = &[
        ("the wildcard", "*", true),
        ("an https origin", "https://example.com", true),
        ("an http origin", "http://example.com", true),
        ("an origin with a port", "http://localhost:3000", true),
        ("a host carrying the wildcard character", "https://*.example.com", true),
        ("an IPv6 literal", "http://[::1]:8080", true),
        ("a value that is not a URL at all", "not-a-url", false),
        ("a bare host", "example.com", false),
        ("a scheme with no host", "https://", false),
        (
            // The URL parse normalises an empty path to the root, so the two spellings of
            // one origin are the same value.
            "an origin written with a trailing path separator",
            "https://example.com/",
            true,
        ),
        ("an origin carrying a path", "https://example.com/callback", false),
        ("an origin carrying a query", "https://example.com?v=1", false),
        ("an origin carrying a fragment", "https://example.com#top", false),
        ("a port that is not a number", "https://example.com:port", false),
        ("whitespace inside the host", "https://exa mple.com", false),
        ("the empty value", "", false),
    ];

    for (index, (case, origin, accepted)) in rows.iter().enumerate() {
        let (status, body) = create_api_with_cors(
            &emulator,
            &format!("origin-{index}"),
            json!({ "allowOrigins": [origin] }),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201 from CreateApi, got {status} {body}"
            );
            assert_eq!(
                body.get("corsConfiguration")
                    .and_then(|cors| cors.get("allowOrigins")),
                Some(&json!([origin])),
                "case {case:?}: expected the api to report the origin it was given, got {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateApi, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            BAD_REQUEST,
            "case {case:?}: expected {BAD_REQUEST}, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            format!("Invalid format for origin {origin}"),
            "case {case:?}: expected the malformed-origin sentence naming the value the \
             request sent, got {body}"
        );
    }

    // The FIRST malformed entry of a list is the one the refusal names, whatever sits
    // before it.
    let (status, body) = create_api_with_cors(
        &emulator,
        "origin-list",
        json!({ "allowOrigins": ["https://ok.example.com", "nope", "also-nope"] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a list carrying a malformed origin must be refused, got {status} {body}"
    );
    assert_eq!(
        as_str(&body, "message"),
        "Invalid format for origin nope",
        "expected the first malformed entry to be named, got {body}"
    );
}

#[tokio::test]
async fn an_allowed_method_is_the_wildcard_or_a_method_the_gateway_serves() {
    let emulator = emulator();

    // Every method an api of this control plane serves a route for, the wildcard, and the
    // ways a value stops being one: another verb, the wrong case, the catch-all a route key
    // names (which is no HTTP method), a list written as one value, and the empty value.
    // The published catalog is the one the service's own OpenAPI import reads an operation
    // under (`crate::ops::import_api`'s `OPERATION_KEYS`).
    let rows: &[(&str, bool)] = &[
        ("GET", true),
        ("PUT", true),
        ("POST", true),
        ("DELETE", true),
        ("OPTIONS", true),
        ("HEAD", true),
        ("PATCH", true),
        ("*", true),
        ("BOGUS", false),
        ("get", false),
        ("ANY", false),
        ("TRACE", false),
        ("GET,POST", false),
        ("", false),
    ];

    for (index, (method, accepted)) in rows.iter().enumerate() {
        let (status, body) = create_api_with_cors(
            &emulator,
            &format!("method-{index}"),
            json!({ "allowOrigins": ["https://example.com"], "allowMethods": [method] }),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 201,
                "case {method:?}: expected 201 from CreateApi, got {status} {body}"
            );
            assert_eq!(
                body.get("corsConfiguration")
                    .and_then(|cors| cors.get("allowMethods")),
                Some(&json!([method])),
                "case {method:?}: expected the api to report the method it was given, got \
                 {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {method:?}: expected 400 from CreateApi, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            BAD_REQUEST,
            "case {method:?}: expected {BAD_REQUEST}, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            format!("Method {method} is not supported"),
            "case {method:?}: expected the unsupported-method sentence naming the value the \
             request sent, got {body}"
        );
    }
}

#[tokio::test]
async fn a_cors_max_age_is_stored_and_reported_whatever_integer_it_carries() {
    let emulator = emulator();

    // The shape `Cors$MaxAge` targets declares `range -1..86400`, and the service enforces
    // neither end: the rows are each published bound, one past each of them, the member's
    // documented "no caching" value, an ordinary value, and the widest integer the member's
    // type carries.
    let rows: &[(&str, i64)] = &[
        ("one below the declared minimum", -2),
        ("the declared minimum", -1),
        ("no caching at all", 0),
        ("an ordinary value", 600),
        ("the declared maximum", 86_400),
        ("one past the declared maximum", 86_401),
        ("far past the declared maximum", 604_800),
        ("the widest value the member's type carries", i64::from(i32::MAX)),
    ];

    for (index, (case, max_age)) in rows.iter().enumerate() {
        let (status, created) = create_api_with_cors(
            &emulator,
            &format!("max-age-{index}"),
            json!({ "allowOrigins": ["https://example.com"], "maxAge": max_age }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateApi, got {status} {created}"
        );
        assert_eq!(
            created
                .get("corsConfiguration")
                .and_then(|cors| cors.get("maxAge")),
            Some(&json!(max_age)),
            "case {case:?}: expected the create to echo the maxAge it was given, got {created}"
        );

        // And the STORED record reports it: the value went into the api, not just into the
        // answer.
        let api_id = as_str(&created, "apiId");
        let (status, fetched) = call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetApi, got {status} {fetched}"
        );
        assert_eq!(
            fetched
                .get("corsConfiguration")
                .and_then(|cors| cors.get("maxAge")),
            Some(&json!(max_age)),
            "case {case:?}: expected the stored api to report that maxAge, got {fetched}"
        );
    }
}

#[tokio::test]
async fn a_route_of_either_protocol_carries_the_request_parameters_it_declares() {
    let emulator = emulator();

    // (case, the api's protocol type, the route key, the parameters the route declares).
    // The rows span the two protocols, each parameter LOCATION a mapping expression names,
    // several entries at once, `required` both ways, and the one route key a WEBSOCKET api
    // accepts parameters on. The recorded create that filed the issue is the HTTP row
    // declaring one query-string parameter as required.
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        (
            "an HTTP route declaring one required query-string parameter",
            "HTTP",
            "GET /pets/{petId}",
            json!({ "route.request.querystring.petId": { "required": true } }),
        ),
        (
            "an HTTP route declaring a header parameter that is not required",
            "HTTP",
            "POST /pets",
            json!({ "route.request.header.x-trace": { "required": false } }),
        ),
        (
            "an HTTP route declaring one parameter per location",
            "HTTP",
            "PUT /pets/{petId}",
            json!({
                "route.request.path.petId": { "required": true },
                "route.request.querystring.dryRun": { "required": false },
                "route.request.header.x-trace": { "required": true }
            }),
        ),
        (
            "an HTTP route declaring a parameter with no constraint of its own",
            "HTTP",
            "DELETE /pets/{petId}",
            json!({ "route.request.header.x-trace": {} }),
        ),
        (
            "the connection request of a WEBSOCKET api",
            "WEBSOCKET",
            "$connect",
            json!({ "route.request.querystring.token": { "required": true } }),
        ),
    ];

    for (index, (case, protocol_type, route_key, parameters)) in rows.into_iter().enumerate() {
        let mut api = json!({ "name": format!("params-{index}"), "protocolType": protocol_type });
        if protocol_type == "WEBSOCKET" {
            api["routeSelectionExpression"] = json!("$request.body.action");
        }
        let (status, created) = call(&emulator, "POST", "/v2/apis", Some(api)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateApi, got {status} {created}"
        );
        let api_id = as_str(&created, "apiId");

        let (status, route) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({ "routeKey": route_key, "requestParameters": parameters })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateRoute, got {status} {route}"
        );
        assert_eq!(
            route.get("requestParameters"),
            Some(&parameters),
            "case {case:?}: expected the create to echo the parameters it was given, got \
             {route}"
        );
        let route_id = as_str(&route, "routeId");

        // The round trip: the STORED route reports them.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/routes/{route_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetRoute, got {status} {fetched}"
        );
        assert_eq!(
            fetched.get("requestParameters"),
            Some(&parameters),
            "case {case:?}: expected the stored route to report them, got {fetched}"
        );

        // And each entry is addressable as the sub-resource it is: deleting one leaves the
        // rest of the map exactly as it was.
        let keys: Vec<String> = parameters
            .as_object()
            .expect("every row declares a map")
            .keys()
            .cloned()
            .collect();
        let removed = keys.first().expect("every row declares a parameter");
        let (status, deleted) = call(
            &emulator,
            "DELETE",
            &format!("/v2/apis/{api_id}/routes/{route_id}/requestparameters/{removed}"),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204 from DeleteRouteRequestParameter, got {status} \
             {deleted}"
        );
        let (status, after) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/routes/{route_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetRoute, got {status} {after}"
        );
        let mut remaining = parameters.clone();
        remaining
            .as_object_mut()
            .expect("every row declares a map")
            .remove(removed);
        assert_eq!(
            after.get("requestParameters"),
            Some(&remaining),
            "case {case:?}: expected the deleted entry gone and the rest kept, got {after}"
        );
    }
}

/// One `CreateAuthorizer` of a JWT authorizer carrying `jwt_configuration`.
async fn create_jwt_authorizer(
    emulator: &MultiServiceRouter,
    api_id: &str,
    name: &str,
    jwt_configuration: Option<Value>,
) -> (u16, Value) {
    let mut request = json!({
        "name": name,
        "authorizerType": "JWT",
        "identitySource": ["$request.header.Authorization"]
    });
    if let Some(configuration) = jwt_configuration {
        request["jwtConfiguration"] = configuration;
    }
    call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(request),
    )
    .await
}

#[tokio::test]
async fn a_jwt_authorizer_names_an_issuer_whose_discovery_document_can_be_fetched() {
    let emulator = emulator();
    let api_id = http_api(&emulator, "jwt-issuers").await;

    // The api's OWN endpoint: a host this emulator serves, which serves an api's routes and
    // no identity provider's metadata — so the document the create fetches there is not one,
    // and the issuer resolves to nothing. This is the recorded create that filed the issue.
    let (status, hosting) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "jwt-issuer-host", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateApi for the issuer host, got {status} {hosting}"
    );
    let served_endpoint = as_str(&hosting, "apiEndpoint");

    // The sentences the service states, each with the issuer taken from the request.
    let discovery_path = "/.well-known/openid-configuration";
    let invalid_issuer = |issuer: &str| {
        format!(
            "Caught exception when connecting to {issuer}{discovery_path} for issuer \
             {issuer}. Please try again later. Error: Invalid issuer: {issuer}. Issuer must \
             have a valid discovery endpoint ended with '{discovery_path}'"
        )
    };

    // (case, the `JwtConfiguration` the request carries, the expected refusal — `None` for
    // a configuration the service accepts).
    let rows: Vec<(&str, Option<Value>, Option<String>)> = vec![
        (
            "no JwtConfiguration at all",
            None,
            Some("JwtConfiguration must not be null for JWT Authorizer".to_string()),
        ),
        (
            "a JwtConfiguration naming no issuer",
            Some(json!({ "audience": ["an-audience"] })),
            Some("Invalid issuer: Issuer can not be empty for JWT Authorizer.".to_string()),
        ),
        (
            "an issuer written empty",
            Some(json!({ "issuer": "" })),
            Some("Invalid issuer: Issuer can not be empty for JWT Authorizer.".to_string()),
        ),
        (
            "an issuer that is not a URL",
            Some(json!({ "issuer": "not-an-issuer" })),
            Some(invalid_issuer("not-an-issuer")),
        ),
        (
            "an issuer served over http rather than https",
            Some(json!({ "issuer": "http://issuer.example.test" })),
            Some(invalid_issuer("http://issuer.example.test")),
        ),
        (
            "an https issuer with no host",
            Some(json!({ "issuer": "https://" })),
            Some(invalid_issuer("https://")),
        ),
        (
            "an issuer carrying a query component",
            Some(json!({ "issuer": "https://issuer.example.test?v=1" })),
            Some(invalid_issuer("https://issuer.example.test?v=1")),
        ),
        (
            "an issuer this emulator serves something other than a discovery document for",
            Some(json!({ "issuer": served_endpoint.clone() })),
            Some(invalid_issuer(&served_endpoint)),
        ),
        (
            // A provider OUTSIDE this emulator: nothing here serves that host, this
            // emulator makes no outbound connection to it, and so there is no evidence
            // about the document it publishes.
            "an issuer naming a provider outside this emulator",
            Some(json!({ "issuer": "https://issuer.example.test" })),
            None,
        ),
        (
            "the same provider, with the path and trailing slash an issuer may carry",
            Some(json!({ "issuer": "https://issuer.example.test/oauth2/" })),
            None,
        ),
    ];

    for (index, (case, configuration, refusal)) in rows.into_iter().enumerate() {
        let (status, body) = create_jwt_authorizer(
            &emulator,
            &api_id,
            &format!("jwt-{index}"),
            configuration,
        )
        .await;
        match refusal {
            Some(message) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400 from CreateAuthorizer, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    BAD_REQUEST,
                    "case {case:?}: expected {BAD_REQUEST}, got {body}"
                );
                assert_eq!(
                    as_str(&body, "message"),
                    message,
                    "case {case:?}: expected the service's own sentence, got {body}"
                );
            }
            None => {
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected 201 from CreateAuthorizer, got {status} {body}"
                );
                // The round trip: the stored authorizer reports the issuer it was given.
                let authorizer_id = as_str(&body, "authorizerId");
                let (status, fetched) = call(
                    &emulator,
                    "GET",
                    &format!("/v2/apis/{api_id}/authorizers/{authorizer_id}"),
                    None,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from GetAuthorizer, got {status} {fetched}"
                );
                assert_eq!(
                    fetched
                        .get("jwtConfiguration")
                        .and_then(|configuration| configuration.get("issuer")),
                    body.get("jwtConfiguration")
                        .and_then(|configuration| configuration.get("issuer")),
                    "case {case:?}: expected the stored authorizer to report that issuer, \
                     got {fetched}"
                );
            }
        }
    }
}

/// An issuer that names an identity provider this emulator HOSTS is resolved against that
/// provider: the Amazon Cognito user pool whose issuer `JWTConfiguration$Issuer` documents
/// publishes its OpenID discovery document, so an authorizer naming a pool the account holds
/// is created, and one naming a pool it does not hold is refused.
///
/// This is the other side of the fetch the table above drives: there every served host
/// answered something that is not a discovery document, here one answers the document
/// itself.
#[tokio::test]
async fn an_issuer_naming_a_user_pool_this_emulator_hosts_resolves_against_that_pool() {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut emulator = MultiServiceRouter::new();
    emulator.set_request_authentication(false);
    emulator.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    emulator.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(
            engine,
        ),
    );

    // One user pool, created through the service that OWNS pools, so the issuer below names
    // a provider the account really holds.
    let cognito_host = "cognito-idp.eu-west-1.amazonaws.com";
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{cognito_host}/"))
        .header("host", cognito_host)
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            "AWSCognitoIdentityProviderService.CreateUserPool",
        )
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/eu-west-1/cognito-idp/\
             aws4_request, SignedHeaders=host, Signature=0000",
        )
        .body(bytes::Bytes::from(
            json!({ "PoolName": "jwt-issuer-pool" }).to_string(),
        ))
        .expect("the user pools request builds");
    let response = emulator.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "setup: CreateUserPool must answer 200"
    );
    let created: Value =
        serde_json::from_slice(&response.into_body()).expect("CreateUserPool answers JSON");
    let pool_id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an Id, got {created}"))
        .to_string();

    let api_id = http_api(&emulator, "jwt-pool-issuers").await;

    // (case, the issuer, accepted?) — the documented Cognito form over a pool the account
    // holds, the same form over a pool nothing created, and the pool's issuer written with
    // the trailing separator a client may send.
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "the documented issuer of a pool the account holds",
            format!("https://{cognito_host}/{pool_id}"),
            true,
        ),
        (
            "that same issuer with a trailing separator",
            format!("https://{cognito_host}/{pool_id}/"),
            true,
        ),
        (
            "the documented form over a pool id nothing minted",
            format!("https://{cognito_host}/eu-west-1_nosuchpool"),
            false,
        ),
    ];

    for (index, (case, issuer, accepted)) in rows.into_iter().enumerate() {
        let (status, body) = create_jwt_authorizer(
            &emulator,
            &api_id,
            &format!("pool-{index}"),
            Some(json!({ "issuer": issuer })),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201 from CreateAuthorizer, got {status} {body}"
            );
            assert_eq!(
                body.get("jwtConfiguration")
                    .and_then(|configuration| configuration.get("issuer")),
                Some(&json!(issuer)),
                "case {case:?}: expected the authorizer to report that issuer, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 from CreateAuthorizer, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                BAD_REQUEST,
                "case {case:?}: expected {BAD_REQUEST}, got {body}"
            );
            assert!(
                as_str(&body, "message").contains(&issuer),
                "case {case:?}: expected the refusal to name the issuer, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_mapping_template_is_generated_only_for_a_composite_schema_type() {
    let emulator = emulator();
    // A model is a WEBSOCKET api's resource ("Currently, Models are not permitted for APIs
    // with a protocol type of HTTP"), so the models below live under an api of that
    // protocol.
    let (status, created) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": "template-types",
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateApi, got {status} {created}"
    );
    let api_id = as_str(&created, "apiId");

    // (case, the model's schema, the data type no template is generated for). Every JSON
    // Schema type draft-04 §5.5.2 declares is a row, plus a union of types and a schema
    // declaring none: a template MAPS the members of a structure, so the two composite
    // types are generated for and each scalar is named as one that is not.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "an object",
            json!({ "type": "object", "properties": { "id": { "type": "string" } } }),
            None,
        ),
        (
            "an array",
            json!({ "type": "array", "items": { "type": "string" } }),
            None,
        ),
        (
            "a schema declaring no type at all",
            json!({ "properties": { "id": { "type": "string" } } }),
            None,
        ),
        (
            "a union naming the object type",
            json!({ "type": ["object", "null"] }),
            None,
        ),
        ("a string", json!({ "type": "string" }), Some("string")),
        ("an integer", json!({ "type": "integer" }), Some("integer")),
        ("a number", json!({ "type": "number" }), Some("number")),
        ("a boolean", json!({ "type": "boolean" }), Some("boolean")),
        ("null", json!({ "type": "null" }), Some("null")),
        (
            "a union naming a scalar type",
            json!({ "type": ["string", "null"] }),
            Some("string"),
        ),
        (
            // The first name that is not `null` is the type, wherever it sits.
            "a union whose first name is null and whose second is a scalar",
            json!({ "type": ["null", "integer"] }),
            Some("integer"),
        ),
    ];

    for (index, (case, schema, unsupported)) in rows.into_iter().enumerate() {
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/models"),
            Some(json!({
                "name": format!("Template{index}"),
                "contentType": "application/json",
                "schema": schema.to_string()
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateModel, got {status} {created}"
        );
        let model_id = as_str(&created, "modelId");

        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/models/{model_id}/template"),
            None,
        )
        .await;
        match unsupported {
            Some(data_type) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400 from GetModelTemplate, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    BAD_REQUEST,
                    "case {case:?}: expected {BAD_REQUEST}, got {body}"
                );
                assert_eq!(
                    as_str(&body, "message"),
                    format!("Unsupported data type for mapping template: {data_type}"),
                    "case {case:?}: expected the sentence naming the model's own data type, \
                     got {body}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from GetModelTemplate, got {status} {body}"
                );
                assert!(
                    as_str(&body, "value").contains("$inputRoot"),
                    "case {case:?}: expected a generated template, got {body}"
                );
            }
        }
    }
}

/// Create one portal product and answer its ARN.
async fn portal_product(emulator: &MultiServiceRouter, display_name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v2/portalproducts",
        Some(json!({ "displayName": display_name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreatePortalProduct for {display_name:?}, got {status} {created}"
    );
    as_str(&created, "portalProductArn")
}

/// One `CreatePortal` including `arns`.
async fn create_portal(
    emulator: &MultiServiceRouter,
    display_name: &str,
    arns: Vec<Value>,
) -> (u16, Value) {
    call(
        emulator,
        "POST",
        "/v2/portals",
        Some(json!({
            "authorization": { "none": {} },
            "endpointConfiguration": { "none": {} },
            "portalContent": {
                "displayName": display_name,
                "theme": {
                    "customColors": {
                        "accentColor": "#ff9900",
                        "backgroundColor": "#ffffff",
                        "errorValidationColor": "#d13212",
                        "headerColor": "#232f3e",
                        "navigationColor": "#16191f",
                        "textColor": "#111111"
                    }
                }
            },
            "includedPortalProductArns": arns
        })),
    )
    .await
}

#[tokio::test]
async fn an_included_portal_product_is_a_well_formed_arn_of_a_product_the_account_holds() {
    let emulator = emulator();
    let account = engine::emulator_account_id();
    let held = portal_product(&emulator, "Held product").await;
    let also_held = portal_product(&emulator, "Another held product").await;

    // A portal including products the account HOLDS is created, and reports them back —
    // beside an ARN of ANOTHER account's product, which this store holds no record of and
    // does not resolve: a portal product is shareable, so such an inclusion is legitimate.
    let shared = format!(
        "arn:aws:apigateway:us-west-2:210987654321:/portalproducts/{}",
        "sharedprod"
    );
    let (status, created) = create_portal(
        &emulator,
        "Included",
        vec![
            json!(held.clone()),
            json!(also_held.clone()),
            json!(shared.clone()),
        ],
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreatePortal including products the account holds, got {status} \
         {created}"
    );
    assert_eq!(
        created.get("includedPortalProductArns"),
        Some(&json!([held.clone(), also_held, shared])),
        "expected the portal to report the products it includes, got {created}"
    );

    // (case, the ARN, the sentence it is refused with). The rows span each field an ARN of
    // an account-owned resource carries — the literal `arn`, the partition, the vendor, the
    // Region, the account, the resource path and its id — plus a value that is no ARN at
    // all and one naming another of this service's resource types. Every row is at least
    // the 20 characters `PortalProductArn` declares, so what refuses it is this rule rather
    // than the member's modelled length. The recorded create that filed the issue carried
    // the row whose account field is empty.
    let not_well_formed = |arn: &str| format!("Invalid ARN. ARN is not well formed {arn}");
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a value that is no ARN at all",
            "this-value-is-no-arn-at-all".to_string(),
            not_well_formed("this-value-is-no-arn-at-all"),
        ),
        (
            "an ARN missing its trailing fields",
            "arn:aws:apigateway:us-west-2".to_string(),
            not_well_formed("arn:aws:apigateway:us-west-2"),
        ),
        (
            "an ARN with no account field",
            "arn:aws:apigateway:us-west-2::/portalproducts/doesnotexist".to_string(),
            not_well_formed("arn:aws:apigateway:us-west-2::/portalproducts/doesnotexist"),
        ),
        (
            "an ARN with no Region field",
            format!("arn:aws:apigateway::{account}:/portalproducts/doesnotexist"),
            not_well_formed(&format!(
                "arn:aws:apigateway::{account}:/portalproducts/doesnotexist"
            )),
        ),
        (
            "an ARN with an empty vendor field",
            format!("arn:aws::us-west-2:{account}:/portalproducts/doesnotexist"),
            not_well_formed(&format!(
                "arn:aws::us-west-2:{account}:/portalproducts/doesnotexist"
            )),
        ),
        (
            "an ARN of another vendor",
            format!("arn:aws:s3:us-west-2:{account}:/portalproducts/doesnotexist"),
            not_well_formed(&format!(
                "arn:aws:s3:us-west-2:{account}:/portalproducts/doesnotexist"
            )),
        ),
        (
            "an ARN naming another resource type of this service",
            format!("arn:aws:apigateway:us-west-2:{account}:/apis/abcdefghij"),
            not_well_formed(&format!(
                "arn:aws:apigateway:us-west-2:{account}:/apis/abcdefghij"
            )),
        ),
        (
            "an ARN carrying the resource type but no product id",
            format!("arn:aws:apigateway:us-west-2:{account}:/portalproducts/"),
            not_well_formed(&format!(
                "arn:aws:apigateway:us-west-2:{account}:/portalproducts/"
            )),
        ),
        (
            // Well formed, and the account holds no product under it.
            "a well-formed ARN of a product nothing minted",
            format!("arn:aws:apigateway:us-west-2:{account}:/portalproducts/doesnotexist0"),
            format!("Product not found in account {account} with id doesnotexist0"),
        ),
    ];

    for (index, (case, arn, message)) in rows.into_iter().enumerate() {
        for (where_in_the_list, arns) in [
            ("alone", vec![json!(arn.clone())]),
            (
                "after a product the account holds",
                vec![json!(held.clone()), json!(arn.clone())],
            ),
        ] {
            let (status, body) =
                create_portal(&emulator, &format!("refused-{index}"), arns).await;
            assert_eq!(
                status, 400,
                "case {case:?} ({where_in_the_list}): expected 400 from CreatePortal, got \
                 {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                BAD_REQUEST,
                "case {case:?} ({where_in_the_list}): expected {BAD_REQUEST}, got {body}"
            );
            assert_eq!(
                as_str(&body, "message"),
                message,
                "case {case:?} ({where_in_the_list}): expected the service's own sentence, \
                 got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_product_sharing_policy_is_a_resource_based_policy_document() {
    let emulator = emulator();
    let (status, created) = call(
        &emulator,
        "POST",
        "/v2/portalproducts",
        Some(json!({ "displayName": "Shared product" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreatePortalProduct, got {status} {created}"
    );
    let product_id = as_str(&created, "portalProductId");
    let uri = format!("/v2/portalproducts/{product_id}/sharingpolicy");

    // A document the service stores: a resource-based policy naming an effect, an action
    // and the principal the permission is granted to.
    let stored = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()) },
            "Action": "apigateway:GET"
        }]
    })
    .to_string();
    let (status, written) = call(
        &emulator,
        "PUT",
        &uri,
        Some(json!({ "policyDocument": stored })),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from PutPortalProductSharingPolicy, got {status} {written}"
    );

    // Each clause of the policy grammar a document can break, and the ONE sentence the
    // service refuses every one of them with — followed by the stored policy, which a
    // refused put may not have moved.
    let invalid = "The specified policy was invalid";
    let rows: Vec<(&str, String)> = vec![
        ("a document that is not JSON", "not a policy".to_string()),
        ("a JSON array rather than an object", "[]".to_string()),
        ("an object declaring no statement", "{}".to_string()),
        (
            "a statement list that holds no statement",
            json!({ "Version": "2012-10-17", "Statement": [] }).to_string(),
        ),
        (
            "a policy version that does not exist",
            json!({
                "Version": "2024-01-01",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "apigateway:GET"
                }]
            })
            .to_string(),
        ),
        (
            "a statement declaring no Effect",
            json!({ "Statement": [{ "Principal": "*", "Action": "apigateway:GET" }] })
                .to_string(),
        ),
        (
            "a statement naming no action",
            json!({ "Statement": [{ "Effect": "Allow", "Principal": "*" }] }).to_string(),
        ),
        (
            "a statement naming no principal",
            json!({ "Statement": [{ "Effect": "Allow", "Action": "apigateway:GET" }] })
                .to_string(),
        ),
    ];

    for (case, document) in rows {
        let (status, refused) = call(
            &emulator,
            "PUT",
            &uri,
            Some(json!({ "policyDocument": document })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from PutPortalProductSharingPolicy, got {status} \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            BAD_REQUEST,
            "case {case:?}: expected {BAD_REQUEST}, got {refused}"
        );
        assert_eq!(
            as_str(&refused, "message"),
            invalid,
            "case {case:?}: expected the one invalid-policy sentence, got {refused}"
        );

        let (status, read_back) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortalProductSharingPolicy, got {status} \
             {read_back}"
        );
        assert_eq!(
            read_back.get("policyDocument"),
            Some(&json!(stored.clone())),
            "case {case:?}: expected the refused put to have changed nothing, got {read_back}"
        );
    }
}
