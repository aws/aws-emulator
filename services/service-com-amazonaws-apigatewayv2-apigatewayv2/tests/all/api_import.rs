// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `ImportApi`.
//!
//! The rules these cases assert:
//!
//! * **`ImportApi` persists the HTTP api its OpenAPI `Body` describes**: the name,
//!   description and version from the document's `info`, one route per operation the
//!   document's `paths` declare (keyed `{METHOD} {path}`, `ANY` for the
//!   `x-amazon-apigateway-any-method` extension) and one integration per
//!   `x-amazon-apigateway-integration` extension those operations carry, with the route
//!   wired to it through `integrations/{IntegrationId}`. Every member of the answer is
//!   read back through `GetApi` / `GetRoutes` / `GetIntegrations`, so nothing is echoed
//!   from the request without going through the store.
//! * **`Basepath` decides what the routes' paths are prefixed with**: `ignore` (the
//!   default) leaves the definition's base path out, `prepend` puts all of it in front,
//!   and `split` drops its first segment — a stage name — and prepends the rest.
//! * A definition this service cannot read at all is refused 400 and persists NOTHING,
//!   and `FailOnWarnings` turns a warning into that same refusal. A problem with ONE
//!   path's integration is a warning, not a refusal — see
//!   `api_import_integration_warnings`.
//!
//! The rows span what each rule generalises over: the document's version (OpenAPI 3 and
//! Swagger 2.0), how many paths and operations it declares, which optional members and
//! extensions it carries, the integration type and its spelling, and whether the
//! integration names a payload format version or leaves it to its default.
//! The recorded case that filed the issue — an import whose integration extension names
//! `payloadFormatVersion` — is ONE row.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, assert_minted_id, call, emulator, error_code};

/// The `RouteSelectionExpression` every HTTP api carries: "For HTTP APIs, the
/// routeSelectionExpression must be ${request.method} ${request.path}".
const HTTP_ROUTE_SELECTION_EXPRESSION: &str = "$request.method $request.path";

/// The `PayloadFormatVersion` an imported integration carries when its extension names
/// none: "For all other integrations, 1.0 is the only supported value"
/// (`CreateIntegration$PayloadFormatVersion`).
const DEFAULT_PAYLOAD_FORMAT_VERSION: &str = "1.0";

/// The `TimeoutInMillis` an HTTP api's integration carries when nothing names one ("30
/// seconds for HTTP APIs").
const DEFAULT_HTTP_TIMEOUT_MILLIS: i64 = 30_000;

/// Call `ImportApi`: `PUT /v2/apis` with the definition in the `Body` member and `query`
/// (`""`, or `"?basepath=split"`) carrying the operation's query members.
pub(crate) async fn import(
    router: &MultiServiceRouter,
    query: &str,
    definition: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/v2/apis{query}"),
        Some(json!({ "body": definition })),
    )
    .await
}

/// Every route of an api, as `RouteKey -> Target`, read back through `GetRoutes` — every
/// PAGE of it, so the answer is the api's whole route set and not whichever routes the
/// first page's `RouteId` order happens to put in it.
pub(crate) async fn stored_routes(
    router: &MultiServiceRouter,
    api_id: &str,
) -> BTreeMap<String, Option<String>> {
    let mut routes = BTreeMap::new();
    let mut next_token: Option<String> = None;
    loop {
        let uri = match next_token.as_deref() {
            Some(token) => format!("/v2/apis/{api_id}/routes?nextToken={token}"),
            None => format!("/v2/apis/{api_id}/routes"),
        };
        let (status, body) = call(router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "expected 200 from GetRoutes for api {api_id}, got {status} {body}"
        );
        routes.extend(
            body["items"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|route| {
                    (
                        as_str(route, "routeKey"),
                        route
                            .get("target")
                            .and_then(Value::as_str)
                            .map(ToString::to_string),
                    )
                }),
        );
        next_token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next_token.is_none() {
            return routes;
        }
    }
}

/// Every integration of an api, as `IntegrationId -> Integration`, read back through
/// `GetIntegrations`.
pub(crate) async fn stored_integrations(
    router: &MultiServiceRouter,
    api_id: &str,
) -> BTreeMap<String, Value> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/v2/apis/{api_id}/integrations"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from GetIntegrations for api {api_id}, got {status} {body}"
    );
    body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|integration| (as_str(integration, "integrationId"), integration.clone()))
        .collect()
}

/// How many apis the store holds, through `GetApis`.
async fn stored_api_count(router: &MultiServiceRouter) -> usize {
    let (status, body) = call(router, "GET", "/v2/apis", None).await;
    assert_eq!(status, 200, "expected 200 from GetApis, got {status} {body}");
    body["items"].as_array().map(Vec::len).unwrap_or_default()
}

/// What one imported route's integration must report.
struct ExpectedIntegration {
    integration_type: &'static str,
    uri: &'static str,
    payload_format_version: &'static str,
    timeout_in_millis: i64,
    integration_method: Option<&'static str>,
}

/// One row of the import table: a definition, and the api it must persist.
struct Row {
    case: &'static str,
    definition: String,
    expect_name: &'static str,
    expect_description: Option<&'static str>,
    expect_version: Option<&'static str>,
    /// Every route the import must persist: its key, and the integration it must be
    /// wired to (`None` for an operation that declared none).
    expect_routes: Vec<(&'static str, Option<ExpectedIntegration>)>,
    expect_cors: Option<Value>,
    /// Must the response report at least one warning?
    expect_warnings: bool,
}

/// The OpenAPI 3 definition the first row imports: two operations on one path, a
/// catch-all `ANY` operation on another, and three integrations differing in type,
/// payload format version and timeout.
fn pet_store_definition() -> String {
    json!({
        "openapi": "3.0.1",
        "info": {
            "title": "pet-store",
            "description": "the imported pet store",
            "version": "2026-01-01"
        },
        "paths": {
            "/pets": {
                "get": {
                    "x-amazon-apigateway-integration": {
                        "type": "http_proxy",
                        "httpMethod": "GET",
                        "uri": "https://petstore.example.com/pets",
                        "payloadFormatVersion": "1.0"
                    }
                },
                "post": {
                    "x-amazon-apigateway-integration": {
                        "type": "aws_proxy",
                        "httpMethod": "POST",
                        "uri": "arn:aws:apigateway:eu-west-1:lambda:path/2015-03-31/functions/arn:aws:lambda:eu-west-1:123456789012:function:pets/invocations",
                        "payloadFormatVersion": "2.0",
                        "timeoutInMillis": 12000
                    }
                }
            },
            "/pets/{petId}": {
                "x-amazon-apigateway-any-method": {
                    "x-amazon-apigateway-integration": {
                        "type": "http_proxy",
                        "uri": "https://petstore.example.com/pets/{petId}"
                    }
                }
            }
        }
    })
    .to_string()
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "openapi 3.0 JSON, three operations, payload format versions named and defaulted",
            definition: pet_store_definition(),
            expect_name: "pet-store",
            expect_description: Some("the imported pet store"),
            expect_version: Some("2026-01-01"),
            expect_routes: vec![
                (
                    "GET /pets",
                    Some(ExpectedIntegration {
                        integration_type: "HTTP_PROXY",
                        uri: "https://petstore.example.com/pets",
                        payload_format_version: "1.0",
                        timeout_in_millis: DEFAULT_HTTP_TIMEOUT_MILLIS,
                        integration_method: Some("GET"),
                    }),
                ),
                (
                    "POST /pets",
                    Some(ExpectedIntegration {
                        integration_type: "AWS_PROXY",
                        uri: "arn:aws:apigateway:eu-west-1:lambda:path/2015-03-31/functions/arn:aws:lambda:eu-west-1:123456789012:function:pets/invocations",
                        payload_format_version: "2.0",
                        timeout_in_millis: 12_000,
                        integration_method: Some("POST"),
                    }),
                ),
                (
                    "ANY /pets/{petId}",
                    Some(ExpectedIntegration {
                        integration_type: "HTTP_PROXY",
                        uri: "https://petstore.example.com/pets/{petId}",
                        payload_format_version: DEFAULT_PAYLOAD_FORMAT_VERSION,
                        timeout_in_millis: DEFAULT_HTTP_TIMEOUT_MILLIS,
                        integration_method: None,
                    }),
                ),
            ],
            expect_cors: None,
            expect_warnings: false,
        },
        Row {
            case: "one operation, a CORS extension, the integration type in lower case",
            definition: json!({
                "openapi": "3.0.1",
                "info": { "title": "cors-api", "version": "1.0" },
                "x-amazon-apigateway-cors": {
                    "allowOrigins": ["https://example.com"],
                    "allowMethods": ["GET", "POST"],
                    "allowHeaders": ["content-type"],
                    "maxAge": 600,
                    "allowCredentials": true
                },
                "paths": {
                    "/items": {
                        "get": {
                            "x-amazon-apigateway-integration": {
                                "type": "http_proxy",
                                "httpMethod": "GET",
                                "uri": "https://items.example.com/items",
                                "payloadFormatVersion": "1.0"
                            }
                        }
                    }
                }
            })
            .to_string(),
            expect_name: "cors-api",
            expect_description: None,
            expect_version: Some("1.0"),
            expect_routes: vec![(
                "GET /items",
                Some(ExpectedIntegration {
                    integration_type: "HTTP_PROXY",
                    uri: "https://items.example.com/items",
                    payload_format_version: "1.0",
                    timeout_in_millis: DEFAULT_HTTP_TIMEOUT_MILLIS,
                    integration_method: Some("GET"),
                }),
            )],
            expect_cors: Some(json!({
                "allowCredentials": true,
                "allowHeaders": ["content-type"],
                "allowMethods": ["GET", "POST"],
                "allowOrigins": ["https://example.com"],
                "maxAge": 600
            })),
            expect_warnings: false,
        },
        Row {
            case: "swagger 2.0, a base path the default Basepath ignores",
            definition: json!({
                "swagger": "2.0",
                "info": { "title": "swagger-api", "version": "0.9" },
                "basePath": "/dev",
                "paths": {
                    "/items": {
                        "get": {
                            "x-amazon-apigateway-integration": {
                                "type": "HTTP_PROXY",
                                "httpMethod": "ANY",
                                "uri": "https://items.example.com/items"
                            }
                        }
                    }
                }
            })
            .to_string(),
            expect_name: "swagger-api",
            expect_description: None,
            expect_version: Some("0.9"),
            expect_routes: vec![(
                "GET /items",
                Some(ExpectedIntegration {
                    integration_type: "HTTP_PROXY",
                    uri: "https://items.example.com/items",
                    payload_format_version: DEFAULT_PAYLOAD_FORMAT_VERSION,
                    timeout_in_millis: DEFAULT_HTTP_TIMEOUT_MILLIS,
                    integration_method: Some("ANY"),
                }),
            )],
            expect_cors: None,
            expect_warnings: false,
        },
        Row {
            case: "an operation with no integration extension is imported without a target",
            definition: json!({
                "openapi": "3.0.1",
                "info": { "title": "no-integration" },
                "paths": { "/items": { "get": { "summary": "list the items" } } }
            })
            .to_string(),
            expect_name: "no-integration",
            expect_description: None,
            expect_version: None,
            expect_routes: vec![("GET /items", None)],
            expect_cors: None,
            expect_warnings: true,
        },
        Row {
            case: "a path whose item declares no operations imports no route and warns",
            definition: json!({
                "openapi": "3.0.1",
                "info": { "title": "no-operations" },
                "paths": { "/pets": "see the documentation" }
            })
            .to_string(),
            expect_name: "no-operations",
            expect_description: None,
            expect_version: None,
            expect_routes: Vec::new(),
            expect_cors: None,
            expect_warnings: true,
        },
        Row {
            case: "a definition with no paths imports an api with no routes",
            definition: json!({
                "openapi": "3.1.0",
                "info": { "title": "empty-api", "version": "1" }
            })
            .to_string(),
            expect_name: "empty-api",
            expect_description: None,
            expect_version: Some("1"),
            expect_routes: Vec::new(),
            expect_cors: None,
            expect_warnings: false,
        },
    ]
}

#[tokio::test]
async fn import_api_persists_the_api_the_definition_describes() {
    let router = emulator();

    for row in rows() {
        let (status, imported) = import(&router, "", &row.definition).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from ImportApi, got {status} {imported}",
            row.case
        );

        let api_id = as_str(&imported, "apiId");
        assert_minted_id(row.case, "ApiId", &api_id);
        for (member, expected) in [
            ("name", Some(row.expect_name)),
            ("description", row.expect_description),
            ("version", row.expect_version),
        ] {
            assert_eq!(
                imported.get(member).and_then(Value::as_str),
                expected,
                "case {:?}: ImportApi must report {member} from the definition, got {imported}",
                row.case
            );
        }
        // An imported api is an HTTP api, resolved to the values that protocol
        // documents; `ImportApi` names none of them in its request.
        for (member, expected) in [
            ("protocolType", "HTTP"),
            ("routeSelectionExpression", HTTP_ROUTE_SELECTION_EXPRESSION),
            ("apiKeySelectionExpression", "$request.header.x-api-key"),
            ("ipAddressType", "ipv4"),
            (
                "apiEndpoint",
                &format!("https://{api_id}.execute-api.eu-west-1.amazonaws.com"),
            ),
        ] {
            assert_eq!(
                as_str(&imported, member),
                expected,
                "case {:?}: expected {member} {expected:?}, got {imported}",
                row.case
            );
        }
        assert_eq!(
            imported.get("corsConfiguration"),
            row.expect_cors.as_ref(),
            "case {:?}: CorsConfiguration must come from the definition's extension, got {imported}",
            row.case
        );
        assert_eq!(
            imported
                .get("warnings")
                .and_then(Value::as_array)
                .is_some_and(|warnings| !warnings.is_empty()),
            row.expect_warnings,
            "case {:?}: unexpected Warnings, got {imported}",
            row.case
        );

        // The STORED api, not the answer: every member above has to survive the write.
        let (status, fetched) = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetApi, got {status} {fetched}",
            row.case
        );
        for member in [
            "name",
            "description",
            "version",
            "protocolType",
            "routeSelectionExpression",
            "apiEndpoint",
            "corsConfiguration",
            "createdDate",
        ] {
            assert_eq!(
                fetched.get(member),
                imported.get(member),
                "case {:?}: GetApi must report the record ImportApi stored, member {member}: \
                 {fetched} against {imported}",
                row.case
            );
        }

        let routes = stored_routes(&router, &api_id).await;
        let integrations = stored_integrations(&router, &api_id).await;
        assert_eq!(
            routes.keys().cloned().collect::<BTreeSet<_>>(),
            row.expect_routes
                .iter()
                .map(|(key, _)| (*key).to_string())
                .collect::<BTreeSet<_>>(),
            "case {:?}: the imported routes must be the operations the definition declares, \
             got {routes:?}",
            row.case
        );
        assert_eq!(
            integrations.len(),
            row.expect_routes
                .iter()
                .filter(|(_, integration)| integration.is_some())
                .count(),
            "case {:?}: one integration per integration extension, got {integrations:?}",
            row.case
        );

        for (route_key, expected) in &row.expect_routes {
            let target = routes
                .get(*route_key)
                .cloned()
                .unwrap_or_default();
            let Some(expected) = expected else {
                assert!(
                    target.is_none(),
                    "case {:?}: route {route_key:?} declared no integration, so it carries no \
                     Target; got {target:?}",
                    row.case
                );
                continue;
            };
            let integration_id = target
                .as_deref()
                .and_then(|target| target.strip_prefix("integrations/"))
                .unwrap_or_default()
                .to_string();
            assert!(
                !integration_id.is_empty(),
                "case {:?}: route {route_key:?} must target integrations/{{IntegrationId}}, got \
                 {target:?}",
                row.case
            );
            let integration = integrations.get(&integration_id).unwrap_or_else(|| {
                panic!(
                    "case {:?}: route {route_key:?} targets {integration_id:?}, which the api \
                     does not hold; got {integrations:?}",
                    row.case
                )
            });
            assert_eq!(
                as_str(integration, "integrationType"),
                expected.integration_type,
                "case {:?}: route {route_key:?} integration type, got {integration}",
                row.case
            );
            assert_eq!(
                as_str(integration, "integrationUri"),
                expected.uri,
                "case {:?}: route {route_key:?} integration uri, got {integration}",
                row.case
            );
            assert_eq!(
                as_str(integration, "payloadFormatVersion"),
                expected.payload_format_version,
                "case {:?}: route {route_key:?} payload format version, got {integration}",
                row.case
            );
            assert_eq!(
                integration.get("timeoutInMillis").and_then(Value::as_i64),
                Some(expected.timeout_in_millis),
                "case {:?}: route {route_key:?} timeout, got {integration}",
                row.case
            );
            assert_eq!(
                integration
                    .get("integrationMethod")
                    .and_then(Value::as_str),
                expected.integration_method,
                "case {:?}: route {route_key:?} integration method, got {integration}",
                row.case
            );
        }
    }
}

/// The definition the base-path table imports: one path, under a server url whose own
/// path is the base path.
fn base_path_definition(server_url: &str) -> String {
    json!({
        "openapi": "3.0.1",
        "info": { "title": "base-path-api", "version": "1" },
        "servers": [{ "url": server_url }],
        "paths": {
            "/pets": {
                "get": {
                    "x-amazon-apigateway-integration": {
                        "type": "http_proxy",
                        "httpMethod": "GET",
                        "uri": "https://petstore.example.com/pets"
                    }
                }
            }
        }
    })
    .to_string()
}

#[tokio::test]
async fn basepath_decides_what_the_imported_paths_are_prefixed_with() {
    let router = emulator();

    // Each row varies the Basepath mode AND the base path the definition declares (two
    // segments, one segment, none at all, and a relative server url), because `split`
    // drops the first segment whatever the rest of it is.
    let cases: Vec<(&str, &str, String, &str)> = vec![
        (
            "no basepath, two-segment base path",
            "",
            base_path_definition("https://api.example.com/dev/v1"),
            "GET /pets",
        ),
        (
            "ignore, two-segment base path",
            "?basepath=ignore",
            base_path_definition("https://api.example.com/dev/v1"),
            "GET /pets",
        ),
        (
            "prepend, two-segment base path",
            "?basepath=prepend",
            base_path_definition("https://api.example.com/dev/v1"),
            "GET /dev/v1/pets",
        ),
        (
            "split, two-segment base path",
            "?basepath=split",
            base_path_definition("https://api.example.com/dev/v1"),
            "GET /v1/pets",
        ),
        (
            "prepend, one-segment base path",
            "?basepath=prepend",
            base_path_definition("https://api.example.com/dev"),
            "GET /dev/pets",
        ),
        (
            "split, one-segment base path leaves nothing to prepend",
            "?basepath=split",
            base_path_definition("https://api.example.com/dev"),
            "GET /pets",
        ),
        (
            "prepend, a server url with no path",
            "?basepath=prepend",
            base_path_definition("https://api.example.com"),
            "GET /pets",
        ),
        (
            "prepend, a relative server url",
            "?basepath=prepend",
            base_path_definition("/stage/v2"),
            "GET /stage/v2/pets",
        ),
        (
            "prepend, a swagger 2.0 basePath",
            "?basepath=prepend",
            json!({
                "swagger": "2.0",
                "info": { "title": "base-path-api", "version": "1" },
                "basePath": "/dev",
                "paths": {
                    "/pets": {
                        "get": {
                            "x-amazon-apigateway-integration": {
                                "type": "http_proxy",
                                "uri": "https://petstore.example.com/pets"
                            }
                        }
                    }
                }
            })
            .to_string(),
            "GET /dev/pets",
        ),
    ];

    for (case, query, definition, expected_route_key) in cases {
        let (status, imported) = import(&router, query, &definition).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ImportApi, got {status} {imported}"
        );
        let api_id = as_str(&imported, "apiId");
        let routes = stored_routes(&router, &api_id).await;
        assert_eq!(
            routes.keys().cloned().collect::<Vec<_>>(),
            vec![expected_route_key.to_string()],
            "case {case:?}: expected the route key {expected_route_key:?}, got {routes:?}"
        );
    }
}

#[tokio::test]
async fn a_definition_this_service_cannot_import_is_refused_and_persists_nothing() {
    let router = emulator();

    // Each row breaks a rule the import enforces, and names the member that breaks it.
    let cases: Vec<(&str, &str, String)> = vec![
        (
            "a body that is not a document",
            "",
            "not an api definition at all".to_string(),
        ),
        ("an empty body", "", String::new()),
        (
            "a document that is a list",
            "",
            json!([{ "openapi": "3.0.1" }]).to_string(),
        ),
        (
            "no openapi or swagger version",
            "",
            json!({ "info": { "title": "versionless" }, "paths": {} }).to_string(),
        ),
        (
            "no info.title to name the api",
            "",
            json!({ "openapi": "3.0.1", "paths": {} }).to_string(),
        ),
        (
            "an info.title past the modelled 128 characters",
            "",
            json!({ "openapi": "3.0.1", "info": { "title": "t".repeat(129) } }).to_string(),
        ),
        (
            "a path that does not start with '/'",
            "",
            json!({
                "openapi": "3.0.1",
                "info": { "title": "bad-path" },
                "paths": { "pets": { "get": {} } }
            })
            .to_string(),
        ),
        (
            "paths that is not an object",
            "",
            json!({ "openapi": "3.0.1", "info": { "title": "bad-paths" }, "paths": [] })
                .to_string(),
        ),
        // A problem with ONE path's integration is NOT a refusal — the api is created and
        // the problem is warned about; those rows live in
        // `api_import_integration_warnings`.
        (
            // And the CORS rule every writer of a configuration applies.
            "a CORS configuration allowing credentials and the wildcard origin",
            "",
            json!({
                "openapi": "3.0.1",
                "info": { "title": "cors-wildcard-credentials" },
                "x-amazon-apigateway-cors": { "allowOrigins": ["*"], "allowCredentials": true },
                "paths": {}
            })
            .to_string(),
        ),
        (
            // And the CORS value rules every writer of a configuration applies: a value
            // that is no method allows none.
            "a CORS configuration allowing a method that is not one",
            "",
            json!({
                "openapi": "3.0.1",
                "info": { "title": "cors-method" },
                "x-amazon-apigateway-cors": {
                    "allowOrigins": ["*"],
                    "allowMethods": ["BOGUS"]
                },
                "paths": {}
            })
            .to_string(),
        ),
        (
            "a Basepath outside the documented three",
            "?basepath=stage",
            json!({ "openapi": "3.0.1", "info": { "title": "bad-basepath" } }).to_string(),
        ),
        (
            "a Basepath in a case the documented values do not carry",
            "?basepath=Prepend",
            json!({ "openapi": "3.0.1", "info": { "title": "cased-basepath" } }).to_string(),
        ),
        (
            "a definition in the YAML spelling, which this emulator does not read",
            "",
            "openapi: 3.0.1\ninfo:\n  title: yaml-api\npaths: {}\n".to_string(),
        ),
    ];

    for (case, query, definition) in cases {
        let before = stored_api_count(&router).await;
        let (status, body) = import(&router, query, &definition).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from ImportApi, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            stored_api_count(&router).await,
            before,
            "case {case:?}: a refused import must persist nothing"
        );
    }
}

/// An integration timeout, as the extension writes it.
fn timeout_definition(timeout_in_millis: i64) -> String {
    json!({
        "openapi": "3.0.1",
        "info": { "title": "timeout-api" },
        "paths": {
            "/pets": {
                "get": {
                    "x-amazon-apigateway-integration": {
                        "type": "http_proxy",
                        "uri": "https://petstore.example.com/pets",
                        "timeoutInMillis": timeout_in_millis
                    }
                }
            }
        }
    })
    .to_string()
}

#[tokio::test]
async fn an_integration_timeout_is_held_to_the_modelled_range() {
    let router = emulator();

    // `TimeoutInMillis` targets `IntegerWithLengthBetween50And30000` (range 50..30000),
    // so each published bound is accepted and one past it is refused.
    for timeout in [50_i64, 29_000, 30_000] {
        let (status, imported) = import(&router, "", &timeout_definition(timeout)).await;
        assert_eq!(
            status, 200,
            "case timeoutInMillis={timeout}: inside the modelled range, got {status} {imported}"
        );
        let api_id = as_str(&imported, "apiId");
        let integrations = stored_integrations(&router, &api_id).await;
        let reported: Vec<Option<i64>> = integrations
            .values()
            .map(|integration| integration.get("timeoutInMillis").and_then(Value::as_i64))
            .collect();
        assert_eq!(
            reported,
            vec![Some(timeout)],
            "case timeoutInMillis={timeout}: the stored integration must report the timeout \
             the extension named, got {integrations:?}"
        );
    }
    // Outside that range the integration is not created: the problem belongs to one path,
    // so the api is imported, the route is kept without a target and the response warns.
    for timeout in [49_i64, 30_001, 0] {
        let (status, imported) = import(&router, "", &timeout_definition(timeout)).await;
        assert_eq!(
            status, 200,
            "case timeoutInMillis={timeout}: outside the modelled range, got {status} {imported}"
        );
        let warnings = imported["warnings"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            warnings.len(),
            1,
            "case timeoutInMillis={timeout}: expected one warning for the integration the \
             import could not create, got {imported}"
        );
        let api_id = as_str(&imported, "apiId");
        assert_eq!(
            stored_integrations(&router, &api_id).await.len(),
            0,
            "case timeoutInMillis={timeout}: the integration must not be created"
        );
        assert_eq!(
            stored_routes(&router, &api_id).await,
            BTreeMap::from([("GET /pets".to_string(), None)]),
            "case timeoutInMillis={timeout}: the route must be imported without a target"
        );
    }
}

/// The most characters the base path an import prefixes onto every route's path may run
/// to: the reader's own bound, so the test tracks it rather than restating a number.
const MAX_BASE_PATH_CHARS: usize =
    service_com_amazonaws_apigatewayv2_apigatewayv2::ops::import_api::MAX_BASE_PATH_CHARS;

/// The most routes one api may hold, the quota the read stops at.
const MAX_ROUTES_PER_API: usize =
    service_com_amazonaws_apigatewayv2_apigatewayv2::state::MAX_ROUTES_PER_API;

/// A definition declaring one base path of `base_path_segments` segments and `paths`
/// minimal `GET` operations — the shape whose READ would amplify the body if the base
/// path the prefix is built from were unbounded, since the prefix is repeated into every
/// route's path.
fn prefixed_definition(base_path_chars: usize, paths: usize) -> String {
    let base_path = format!("/{}", "s".repeat(base_path_chars.saturating_sub(1)));
    let mut declared = serde_json::Map::new();
    for index in 0..paths {
        declared.insert(format!("/p{index}"), json!({ "get": {} }));
    }
    json!({
        "openapi": "3.0.1",
        "info": { "title": "prefixed-api" },
        "servers": [{ "url": base_path }],
        "paths": Value::Object(declared)
    })
    .to_string()
}

#[tokio::test]
async fn the_read_is_bounded_by_the_base_path_length_and_the_route_quota() {
    let router = emulator();

    // The base path at its bound is read; one character past it is refused, because that
    // prefix is what every route's path carries.
    for (case, chars, expected) in [
        ("at the bound", MAX_BASE_PATH_CHARS, 200),
        ("one past the bound", MAX_BASE_PATH_CHARS + 1, 400),
    ] {
        let before = stored_api_count(&router).await;
        let (status, body) = import(
            &router,
            "?basepath=prepend",
            &prefixed_definition(chars, 1),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: a base path of {chars} characters, got {status} {body}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body}"
            );
            assert_eq!(
                stored_api_count(&router).await,
                before,
                "case {case:?}: a refused import must persist nothing"
            );
        }
    }

    // A definition declaring more operations than one api may hold routes is refused for
    // the quota it exceeds, and persists nothing — the read stops at the quota rather
    // than collecting every path the body declares.
    let before = stored_api_count(&router).await;
    let (status, body) = import(
        &router,
        "",
        &prefixed_definition(1, MAX_ROUTES_PER_API + 1),
    )
    .await;
    assert_eq!(
        status, 429,
        "expected 429 past the route quota of {MAX_ROUTES_PER_API}, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "expected TooManyRequestsException, got {body}"
    );
    assert_eq!(
        as_str(&body, "limitType"),
        "Route",
        "expected the refusal to name the limit it exceeded, got {body}"
    );
    assert_eq!(
        stored_api_count(&router).await,
        before,
        "a refused import must persist nothing"
    );

    // A definition declaring exactly the quota is imported, so the stop is at the bound
    // and not before it.
    let (status, imported) = import(&router, "", &prefixed_definition(1, MAX_ROUTES_PER_API)).await;
    assert_eq!(
        status, 200,
        "expected 200 at the route quota of {MAX_ROUTES_PER_API}, got {status} {imported}"
    );
    let api_id = as_str(&imported, "apiId");
    let routes = stored_routes(&router, &api_id).await;
    assert!(
        routes.contains_key("GET /p0"),
        "expected the api to hold the routes the definition declared, got {routes:?}"
    );
}

/// A definition that warns twice: an operation under a method no route key may carry, and
/// an operation that declares no integration.
fn warning_definition() -> String {
    json!({
        "openapi": "3.0.1",
        "info": { "title": "warned-api" },
        "components": { "schemas": {} },
        "security": [],
        "paths": {
            "/pets": {
                "trace": { "summary": "trace the pets" },
                "get": { "summary": "list the pets" }
            }
        }
    })
    .to_string()
}

#[tokio::test]
async fn fail_on_warnings_rolls_the_import_back() {
    let router = emulator();

    // Without `FailOnWarnings` the api is created and the warnings are reported...
    let (status, imported) = import(&router, "", &warning_definition()).await;
    assert_eq!(
        status, 200,
        "expected 200 from ImportApi without FailOnWarnings, got {status} {imported}"
    );
    let warnings = imported["warnings"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        warnings.len(),
        2,
        "expected one warning for the unsupported method and one for the missing \
         integration, got {imported}"
    );
    // ...and the properties the import ignored are reported too.
    let import_info: BTreeSet<String> = imported["importInfo"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry.as_str().map(ToString::to_string))
        .collect();
    assert_eq!(
        import_info.len(),
        2,
        "expected ImportInfo to name the two root properties the import ignores \
         (components, security), got {imported}"
    );
    assert!(
        import_info.iter().any(|entry| entry.contains("components"))
            && import_info.iter().any(|entry| entry.contains("security")),
        "expected ImportInfo to name components and security, got {import_info:?}"
    );
    let api_id = as_str(&imported, "apiId");
    let routes = stored_routes(&router, &api_id).await;
    assert_eq!(
        routes.keys().cloned().collect::<Vec<_>>(),
        vec!["GET /pets".to_string()],
        "the unsupported method imports nothing and the supported one imports a route, \
         got {routes:?}"
    );

    // ...and with it, the same definition is refused and nothing is written.
    let before = stored_api_count(&router).await;
    let (status, body) = import(&router, "?failOnWarnings=true", &warning_definition()).await;
    assert_eq!(
        status, 400,
        "expected 400 from ImportApi with FailOnWarnings, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "BadRequestException",
        "expected BadRequestException, got {body}"
    );
    assert_eq!(
        stored_api_count(&router).await,
        before,
        "FailOnWarnings rolls the import back, so no api is created"
    );

    // A definition that warns about nothing is not affected by FailOnWarnings.
    let (status, imported) = import(
        &router,
        "?failOnWarnings=true",
        &json!({
            "openapi": "3.0.1",
            "info": { "title": "clean-api" },
            "paths": {
                "/pets": {
                    "get": {
                        "x-amazon-apigateway-integration": {
                            "type": "http_proxy",
                            "uri": "https://petstore.example.com/pets"
                        }
                    }
                }
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        status, 200,
        "a definition with no warnings imports under FailOnWarnings, got {status} {imported}"
    );
    assert!(
        imported.get("warnings").is_none() || imported["warnings"].is_null(),
        "an import that warned about nothing reports no Warnings, got {imported}"
    );
    assert!(
        imported.get("importInfo").is_none() || imported["importInfo"].is_null(),
        "an import that ignored nothing reports no ImportInfo, got {imported}"
    );
}

#[tokio::test]
async fn two_imports_of_one_definition_are_two_apis() {
    let router = emulator();

    // Importing the same definition twice is two APIs, each with its own ids: an import
    // creates a resource rather than updating one (that is `ReimportApi`).
    let (_, first) = import(&router, "", &pet_store_definition()).await;
    let (_, second) = import(&router, "", &pet_store_definition()).await;
    let (first_id, second_id) = (as_str(&first, "apiId"), as_str(&second, "apiId"));
    assert_ne!(
        first_id, second_id,
        "each import mints its own ApiId, got {first_id} twice"
    );

    let first_routes = stored_routes(&router, &first_id).await;
    let second_routes = stored_routes(&router, &second_id).await;
    assert_eq!(
        first_routes.keys().collect::<Vec<_>>(),
        second_routes.keys().collect::<Vec<_>>(),
        "the same definition imports the same route keys under either api"
    );

    // An `IntegrationId` is unique within its api, so each import resolves its own
    // targets inside its own api — the same id names a DIFFERENT record under the other
    // one, and every target has to resolve under the api that holds the route.
    for (api_id, routes) in [(&first_id, &first_routes), (&second_id, &second_routes)] {
        let integrations = stored_integrations(&router, api_id).await;
        for (route_key, target) in routes {
            let integration_id = target
                .as_deref()
                .and_then(|target| target.strip_prefix("integrations/"))
                .unwrap_or_default();
            assert!(
                integrations.contains_key(integration_id),
                "api {api_id}: route {route_key:?} targets {target:?}, which that api does not \
                 hold; got {integrations:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_imported_service_action_integration_needs_no_uri() {
    let router = emulator();

    // The same rule `CreateIntegration` applies: an integration carrying an
    // `IntegrationSubtype` names the AWS service action it invokes there, so it needs no
    // `uri`. One row per subtype, so the rule is asserted over the class and not over
    // one service.
    for subtype in [
        "SQS-SendMessage",
        "EventBridge-PutEvents",
        "StepFunctions-StartExecution",
    ] {
        let definition = json!({
            "openapi": "3.0.1",
            "info": { "title": format!("subtype-{subtype}") },
            "paths": {
                "/send": {
                    "post": {
                        "x-amazon-apigateway-integration": {
                            "type": "aws_proxy",
                            "integrationSubtype": subtype,
                            "payloadFormatVersion": "1.0",
                            "credentials": "arn:aws:iam::000000000000:role/apigw",
                            "requestParameters": { "MessageBody": "$request.body.message" }
                        }
                    }
                }
            }
        })
        .to_string();

        let (status, imported) = import(&router, "", &definition).await;
        assert_eq!(
            status, 200,
            "case {subtype:?}: expected 200 from ImportApi, got {status} {imported}"
        );
        let api_id = as_str(&imported, "apiId");
        let integrations = stored_integrations(&router, &api_id).await;
        let stored = integrations
            .values()
            .next()
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            as_str(&stored, "integrationSubtype"),
            subtype,
            "case {subtype:?}: the imported integration must carry the subtype, got {stored}"
        );
        assert!(
            stored.get("integrationUri").is_none(),
            "case {subtype:?}: an integration created from a subtype names no URI, got {stored}"
        );
    }
}
