// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetExport`: the OpenAPI definition a deployed stage is exported
//! as, read back through the wire path a client uses.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource` helpers are this crate's existing harness, reused
//! from `rest_api_lifecycle`.
//!
//! The tests assert the RULE over sets of inputs rather than one recorded call: an export
//! renders the DEPLOYMENT the named stage serves as an OpenAPI document in the dialect
//! `exportType` names, serialized in the media type `accepts` asks for, with every value
//! taken from the stored records. The rows differ along what that rule generalises over —
//! both `exportType` values, both `accepts` values (and the forms an `Accept` header is
//! written in), the number of paths and their nesting, the HTTP verbs on a path
//! (including the `ANY` catch-all), path and query and header request parameters, the
//! method responses and their headers, every `authorizationType` the method
//! documentation lists, and every `parameters` combination the `GetExport` documentation
//! publishes.

use std::collections::BTreeMap;

use bytes::Bytes;
use serde_json::{json, Map, Value};

use crate::execute_api_data_plane::{deploy, put_method};
use crate::gateway_response::put_gateway_response;
use crate::rest_api_lifecycle::{create_api, create_resource, string_member, Gateway};

/// The two dialects `exportType` accepts, from `GetExportRequest$exportType`: "'oas30'
/// for OpenAPI 3.0.x and 'swagger' for Swagger/OpenAPI 2.0".
const DIALECTS: &[&str] = &["swagger", "oas30"];

/// One named list of a case's table: a resource path and the HTTP verbs put on it, or a
/// status code and the headers its method response declares.
type NamedList<'a> = (&'a str, &'a [&'a str]);

/// One row of the path/operation table: the case, the API's paths, and the paths and
/// Path Item keys the export must carry.
type PathCase<'a> = (&'a str, &'a [NamedList<'a>], Vec<(&'a str, Vec<&'a str>)>);

/// One row of the method-response table: the case, the method's verb, and its responses
/// with the headers each declares.
type ResponseCase<'a> = (&'a str, &'a str, &'a [NamedList<'a>]);

/// The query string `parameters` of an export request.
type Parameters<'a> = &'a [(&'a str, &'a str)];

/// One row of the extensions table: the case, the request's `parameters`, and whether the
/// integration and the authorizer extension must be carried.
type ExtensionCase<'a> = (&'a str, Parameters<'a>, bool, bool);

/// One row of the gateway-response table: the case, the `PutGatewayResponse` requests the
/// API is customized with (by response type), and the gateway-response extension the
/// export must carry — `Value::Null` for a row that must carry none.
type GatewayResponseCase<'a> = (&'a str, Vec<(&'a str, Value)>, Value);

/// One row of the refusal table: the case, the api id, the stage, the export type, the
/// `Accept` value, the `parameters`, and the status and error code the call is refused
/// with.
type RefusalCase<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    Option<&'a str>,
    Parameters<'a>,
    u16,
    &'a str,
);

/// One answered export: the status, the error code of a refusal, the two response headers
/// the operation binds and the body bytes.
///
/// Shared with the `stage_export_documentation` suite, which exports the documentation set
/// a stage serves through this same wire path.
pub(crate) struct Export {
    pub(crate) status: u16,
    pub(crate) error: Option<String>,
    content_type: Option<String>,
    /// `pub(crate)` for the sibling `stage_export_version_and_containers` suite, which
    /// asserts the attachment is named for the version the document states.
    pub(crate) content_disposition: Option<String>,
    pub(crate) body: Vec<u8>,
}

impl Export {
    /// The export body as the JSON document it is, with the case named on failure.
    pub(crate) fn json(&self, case: &str) -> Value {
        assert_eq!(
            self.status, 200,
            "case {case}: expected 200 from GetExport, got {} with {:?}",
            self.status,
            String::from_utf8_lossy(&self.body)
        );
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "case {case}: expected a JSON export, got {error} for {:?}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }
}

/// `GetExport` for `stage` of `api`, through the raw wire path so the response HEADERS
/// (`Content-Type`, `Content-Disposition`) are visible beside the payload.
///
/// `accepts` is sent as the `Accept` header the member is bound to; `parameters` are the
/// query string parameters the `parameters` map is bound to, sent with no value at all
/// when the value is empty.
pub(crate) async fn export(
    gateway: &Gateway,
    api: &str,
    stage: &str,
    export_type: &str,
    accepts: Option<&str>,
    parameters: Parameters<'_>,
) -> Export {
    let query: Vec<String> = parameters
        .iter()
        .map(|(key, value)| {
            if value.is_empty() {
                (*key).to_string()
            } else {
                format!("{key}={value}")
            }
        })
        .collect();
    let uri = if query.is_empty() {
        format!("/restapis/{api}/stages/{stage}/exports/{export_type}")
    } else {
        format!(
            "/restapis/{api}/stages/{stage}/exports/{export_type}?{}",
            query.join("&")
        )
    };
    let mut builder = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "apigateway.us-east-1.amazonaws.com")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/apigateway/aws4_request, \
             SignedHeaders=host, Signature=0",
        );
    if let Some(accepts) = accepts {
        builder = builder.header("accept", accepts);
    }
    let request = builder
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| -> Option<String> {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let error = header("x-amzn-errortype");
    let content_type = header("content-type");
    let content_disposition = header("content-disposition");
    Export {
        status,
        error,
        content_type,
        content_disposition,
        body: response.into_body().to_vec(),
    }
}

/// `PutMethodResponse` for `status` on the `verb` method of `resource`, asserting the
/// modelled 201. The request itself goes through the sibling `method_response` suite's
/// helper, so this suite does not build that URI a second time.
async fn create_method_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    request: Value,
) {
    let answer =
        crate::method_response::put_method_response(gateway, api, resource, verb, status, request)
            .await;
    assert_eq!(
        answer.status, 201,
        "case PutMethodResponse({verb} {status}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `PutIntegration` for `verb` on `resource`, asserting the modelled 201, through the
/// sibling `method_integration` suite's helper for the same reason.
async fn create_integration(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    request: Value,
) {
    let answer =
        crate::method_integration::put_integration(gateway, api, resource, verb, request).await;
    assert_eq!(
        answer.status, 201,
        "case PutIntegration({verb}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// An API carrying the resources `paths` names — creating the ancestors a nested path
/// implies — with the verbs of each path put on it as `NONE` methods.
///
/// Answers the API id and the resource id of every path it created, so a case can put a
/// response or an integration on one of them.
pub(crate) async fn api_with_paths(
    gateway: &Gateway,
    name: &str,
    paths: &[NamedList<'_>],
) -> (String, BTreeMap<String, String>) {
    let (api, root) = create_api(gateway, name).await;
    let mut by_path: BTreeMap<String, String> = BTreeMap::new();
    by_path.insert("/".to_string(), root.clone());
    for (path, verbs) in paths {
        let mut parent = root.clone();
        let mut walked = String::new();
        for segment in path.split('/').filter(|segment| !segment.is_empty()) {
            walked = format!("{walked}/{segment}");
            if let Some(existing) = by_path.get(&walked) {
                parent = existing.clone();
                continue;
            }
            let created = create_resource(gateway, &api, &parent, segment).await;
            let id = string_member(&created, "id", path);
            by_path.insert(walked.clone(), id.clone());
            parent = id;
        }
        for verb in *verbs {
            put_method(
                gateway,
                &api,
                &parent,
                verb,
                json!({ "authorizationType": "NONE" }),
            )
            .await;
            // Every method gets the integration a deployment requires of it — `MOCK`, the
            // type that needs no backend — so the API this helper builds is deployable. A
            // case that wants another integration puts its own over this one.
            create_integration(gateway, &api, &parent, verb, json!({ "type": "MOCK" })).await;
        }
    }
    (api, by_path)
}

/// The `paths` object of an exported document, with the case named on failure.
fn document_paths(document: &Value, case: &str) -> Map<String, Value> {
    document["paths"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a paths object, got {document}"))
        .clone()
}

/// The security scheme map of an exported document — `securityDefinitions` in a 2.0
/// document, `components.securitySchemes` in a 3.0 one — or an empty map when the
/// document declares none.
fn document_schemes(document: &Value, dialect: &str) -> Map<String, Value> {
    let schemes = match dialect {
        "swagger" => &document["securityDefinitions"],
        _ => &document["components"]["securitySchemes"],
    };
    schemes.as_object().cloned().unwrap_or_default()
}

/// The keys of a document object, SORTED.
///
/// A JSON object is unordered, and this workspace's `serde_json` keeps a document's keys
/// in insertion order in some builds and in sorted order in others, so a key SET is what
/// a case may assert about one. Every expected list below is written sorted.
fn sorted_keys(object: &Map<String, Value>) -> Vec<&str> {
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys
}

#[tokio::test]
async fn every_deployed_resource_and_method_becomes_a_path_and_an_operation() {
    // Rows differ along what the rule generalises over: how many paths the API has, how
    // deeply they nest, whether they share an ancestor, whether a path part is a
    // parameter or the greedy catch-all, how many verbs one path carries, and whether a
    // verb is the `ANY` catch-all (whose Path Item key is API Gateway's own extension).
    // A resource with no method is not a path of the definition, which the rows with
    // intermediate resources assert.
    let cases: Vec<PathCase<'_>> = vec![
        (
            "one path, one verb",
            &[("/pets", &["GET"])],
            vec![("/pets", vec!["get"])],
        ),
        (
            "one path, every verb a method may carry",
            &[(
                "/things",
                &["GET", "PUT", "POST", "DELETE", "HEAD", "OPTIONS", "PATCH"],
            )],
            vec![(
                "/things",
                vec!["delete", "get", "head", "options", "patch", "post", "put"],
            )],
        ),
        (
            "the ANY catch-all",
            &[("/any", &["ANY"])],
            vec![("/any", vec!["x-amazon-apigateway-any-method"])],
        ),
        (
            "a nested path whose ancestor carries no method",
            &[("/pets/{petId}/photos", &["GET", "POST"])],
            vec![("/pets/{petId}/photos", vec!["get", "post"])],
        ),
        (
            "two paths sharing an ancestor that itself carries a method",
            &[("/pets", &["GET"]), ("/pets/{petId}", &["GET", "DELETE"])],
            vec![
                ("/pets", vec!["get"]),
                ("/pets/{petId}", vec!["delete", "get"]),
            ],
        ),
        (
            "the greedy proxy path part",
            &[("/{proxy+}", &["ANY"])],
            vec![("/{proxy+}", vec!["x-amazon-apigateway-any-method"])],
        ),
        (
            "a method on the API's own root resource",
            &[("/", &["GET"])],
            vec![("/", vec!["get"])],
        ),
    ];

    for (case, paths, expected) in cases {
        let gateway = Gateway::new();
        let (api, _) = api_with_paths(&gateway, case, paths).await;
        deploy(&gateway, &api, "dev").await;
        for dialect in DIALECTS {
            let case = format!("{case} as {dialect}");
            let document = export(&gateway, &api, "dev", dialect, None, &[])
                .await
                .json(&case);

            // The dialect's own identity and the endpoint it names the stage with.
            match *dialect {
                "swagger" => {
                    assert_eq!(
                        document["swagger"].as_str(),
                        Some("2.0"),
                        "case {case}: a swagger export must declare swagger 2.0, got {document}"
                    );
                    assert_eq!(
                        document["host"].as_str(),
                        Some(format!("{api}.execute-api.us-east-1.amazonaws.com").as_str()),
                        "case {case}: expected the api's invoke host, got {document}"
                    );
                    assert_eq!(
                        document["basePath"].as_str(),
                        Some("/dev"),
                        "case {case}: expected the stage as the basePath, got {document}"
                    );
                    assert_eq!(
                        document["schemes"],
                        json!(["https"]),
                        "case {case}: expected the https invoke scheme, got {document}"
                    );
                }
                _ => {
                    assert_eq!(
                        document["openapi"].as_str(),
                        Some("3.0.1"),
                        "case {case}: an oas30 export must declare openapi 3.0.x, got {document}"
                    );
                    assert_eq!(
                        document["servers"],
                        json!([{
                            "url": format!("https://{api}.execute-api.us-east-1.amazonaws.com/{{basePath}}"),
                            "variables": { "basePath": { "default": "dev" } },
                        }]),
                        "case {case}: expected the stage's invoke server, got {document}"
                    );
                    assert!(
                        document.get("host").is_none() && document.get("basePath").is_none(),
                        "case {case}: an oas30 export carries no 2.0 host/basePath, got {document}"
                    );
                }
            }
            assert_eq!(
                document["info"]["title"].as_str(),
                Some(case.split(" as ").next().unwrap_or_default()),
                "case {case}: expected the API's name as info.title, got {document}"
            );

            // Every path of the deployment, and only those, with every method on it.
            let exported = document_paths(&document, &case);
            let paths_found = sorted_keys(&exported);
            let paths_expected: Vec<&str> = expected.iter().map(|(path, _)| *path).collect();
            assert_eq!(
                paths_found, paths_expected,
                "case {case}: expected the deployed paths {paths_expected:?}, got {paths_found:?}"
            );
            for (path, verbs) in &expected {
                let item = exported[*path]
                    .as_object()
                    .unwrap_or_else(|| panic!("case {case}: expected a path item for {path}"))
                    .clone();
                let keys = sorted_keys(&item);
                assert_eq!(
                    &keys, verbs,
                    "case {case}: expected the operations {verbs:?} on {path}, got {keys:?}"
                );
                for (key, operation) in &item {
                    assert!(
                        operation.get("responses").is_some(),
                        "case {case}: operation {key} on {path} must carry a responses object, \
                         got {operation}"
                    );
                    // A path template expression must have a matching required path
                    // parameter for the document to be a valid definition.
                    for name in path
                        .split('/')
                        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
                        .map(|name| name.trim_end_matches('+'))
                    {
                        let declared = operation["parameters"]
                            .as_array()
                            .map(|parameters| {
                                parameters.iter().any(|parameter| {
                                    parameter["name"].as_str() == Some(name)
                                        && parameter["in"].as_str() == Some("path")
                                        && parameter["required"].as_bool() == Some(true)
                                })
                            })
                            .unwrap_or(false);
                        assert!(
                            declared,
                            "case {case}: operation {key} on {path} must declare the required \
                             path parameter {name}, got {operation}"
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn an_export_renders_the_deployment_the_stage_serves_and_follows_a_redeploy() {
    let gateway = Gateway::new();
    let (api, by_path) = api_with_paths(&gateway, "snapshot", &[("/pets", &["GET"])]).await;
    deploy(&gateway, &api, "dev").await;

    let deployed = export(&gateway, &api, "dev", "oas30", None, &[])
        .await
        .json("the first deployment");
    assert_eq!(
        sorted_keys(&document_paths(&deployed, "the first deployment")),
        vec!["/pets"],
        "case the first deployment: expected only the deployed path, got {deployed}"
    );

    // A resource added AFTER the deployment is not what the stage serves, so the export
    // of that stage does not change until the API is redeployed.
    let root = by_path["/"].clone();
    let created = create_resource(&gateway, &api, &root, "orders").await;
    let orders = string_member(&created, "id", "orders");
    put_method(
        &gateway,
        &api,
        &orders,
        "POST",
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    create_integration(&gateway, &api, &orders, "POST", json!({ "type": "MOCK" })).await;
    let unchanged = export(&gateway, &api, "dev", "oas30", None, &[])
        .await
        .json("before the redeploy");
    assert_eq!(
        unchanged, deployed,
        "case before the redeploy: an export renders the deployment the stage serves, got \
         {unchanged}"
    );

    // Redeploying the stage is what makes the new resource part of what it serves.
    deploy(&gateway, &api, "dev").await;
    let redeployed = export(&gateway, &api, "dev", "oas30", None, &[])
        .await
        .json("after the redeploy");
    assert_eq!(
        sorted_keys(&document_paths(&redeployed, "after the redeploy")),
        vec!["/orders", "/pets"],
        "case after the redeploy: expected both deployed paths, got {redeployed}"
    );

    // A second stage of the same API serves its own deployment, and names itself in the
    // endpoint the document carries.
    deploy(&gateway, &api, "prod").await;
    let prod = export(&gateway, &api, "prod", "swagger", None, &[])
        .await
        .json("the second stage");
    assert_eq!(
        prod["basePath"].as_str(),
        Some("/prod"),
        "case the second stage: expected the stage's own basePath, got {prod}"
    );
}

#[tokio::test]
async fn method_request_parameters_become_the_operations_parameters() {
    // Rows differ along the parameter's location (every `{location}` the mapping
    // expression documents), whether it is required, and whether it is a path parameter
    // the resource's own template already declares.
    let cases: Vec<(&str, Value, Vec<Value>)> = vec![
        (
            "a required query parameter",
            json!({ "method.request.querystring.type": true }),
            vec![
                json!({ "name": "petId", "in": "path", "required": true }),
                json!({ "name": "type", "in": "query", "required": true }),
            ],
        ),
        (
            "an optional query parameter",
            json!({ "method.request.querystring.page": false }),
            vec![
                json!({ "name": "petId", "in": "path", "required": true }),
                json!({ "name": "page", "in": "query", "required": false }),
            ],
        ),
        (
            "a header parameter",
            json!({ "method.request.header.X-Trace": false }),
            vec![
                json!({ "name": "petId", "in": "path", "required": true }),
                json!({ "name": "X-Trace", "in": "header", "required": false }),
            ],
        ),
        (
            "the path parameter the resource template declares",
            json!({ "method.request.path.petId": true }),
            vec![json!({ "name": "petId", "in": "path", "required": true })],
        ),
        (
            "several parameters at once",
            json!({
                "method.request.querystring.type": true,
                "method.request.header.X-Trace": false,
                "method.request.path.petId": true,
            }),
            vec![
                json!({ "name": "petId", "in": "path", "required": true }),
                json!({ "name": "X-Trace", "in": "header", "required": false }),
                json!({ "name": "type", "in": "query", "required": true }),
            ],
        ),
        (
            "no request parameters at all",
            json!({}),
            vec![json!({ "name": "petId", "in": "path", "required": true })],
        ),
    ];

    for (case, request_parameters, expected) in cases {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, case).await;
        let created = create_resource(&gateway, &api, &root, "pets").await;
        let pets = string_member(&created, "id", case);
        let created = create_resource(&gateway, &api, &pets, "{petId}").await;
        let pet = string_member(&created, "id", case);
        let mut request = json!({ "authorizationType": "NONE" });
        if let (Some(target), Some(parameters)) = (
            request.as_object_mut(),
            request_parameters.as_object().filter(|map| !map.is_empty()),
        ) {
            target.insert(
                "requestParameters".to_string(),
                Value::Object(parameters.clone()),
            );
        }
        put_method(&gateway, &api, &pet, "GET", request).await;
        create_integration(&gateway, &api, &pet, "GET", json!({ "type": "MOCK" })).await;
        deploy(&gateway, &api, "dev").await;

        for dialect in DIALECTS {
            let case = format!("{case} as {dialect}");
            let document = export(&gateway, &api, "dev", dialect, None, &[])
                .await
                .json(&case);
            let parameters = document["paths"]["/pets/{petId}"]["get"]["parameters"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            // The parameter's type is written on the parameter in 2.0 and in its own
            // schema in 3.0, which is the one difference between the dialects here.
            let expected: Vec<Value> = expected
                .iter()
                .map(|parameter| {
                    let mut parameter = parameter.as_object().cloned().unwrap_or_default();
                    match *dialect {
                        "swagger" => {
                            parameter.insert("type".to_string(), json!("string"));
                        }
                        _ => {
                            parameter.insert("schema".to_string(), json!({ "type": "string" }));
                        }
                    }
                    Value::Object(parameter)
                })
                .collect();
            assert_eq!(
                parameters, expected,
                "case {case}: expected the parameters {expected:?}, got {parameters:?}"
            );
        }
    }
}

#[tokio::test]
async fn method_responses_become_responses_carrying_their_declared_headers() {
    // Rows differ along the status code, how many responses one method has and how many
    // headers each declares — including the CORS headers an OPTIONS method of a
    // cross-origin API declares, which is what makes the exported definition carry that
    // API's CORS configuration.
    let cases: Vec<ResponseCase<'_>> = vec![
        ("one response, no headers", "GET", &[("200", &[])]),
        (
            "one response with one header",
            "GET",
            &[("200", &["X-Count"])],
        ),
        (
            "several responses",
            "POST",
            &[("201", &["Location"]), ("400", &[]), ("500", &[])],
        ),
        (
            "the CORS preflight response of an OPTIONS method",
            "OPTIONS",
            &[(
                "200",
                &[
                    "Access-Control-Allow-Headers",
                    "Access-Control-Allow-Methods",
                    "Access-Control-Allow-Origin",
                ],
            )],
        ),
    ];

    for (case, verb, responses) in cases {
        let gateway = Gateway::new();
        let (api, by_path) = api_with_paths(&gateway, case, &[("/pets", &[verb])]).await;
        let pets = by_path["/pets"].clone();
        for (status, headers) in responses {
            let parameters: Map<String, Value> = headers
                .iter()
                .map(|header| (format!("method.response.header.{header}"), json!(true)))
                .collect();
            let mut request = json!({});
            if let (Some(target), false) = (request.as_object_mut(), parameters.is_empty()) {
                target.insert(
                    "responseParameters".to_string(),
                    Value::Object(parameters.clone()),
                );
            }
            create_method_response(&gateway, &api, &pets, verb, status, request).await;
        }
        deploy(&gateway, &api, "dev").await;

        for dialect in DIALECTS {
            let case = format!("{case} as {dialect}");
            let document = export(&gateway, &api, "dev", dialect, None, &[])
                .await
                .json(&case);
            let operation = &document["paths"]["/pets"][verb.to_ascii_lowercase()];
            let exported = operation["responses"]
                .as_object()
                .unwrap_or_else(|| panic!("case {case}: expected a responses object, got {operation}"))
                .clone();
            let codes = sorted_keys(&exported);
            let expected_codes: Vec<&str> = responses.iter().map(|(status, _)| *status).collect();
            assert_eq!(
                codes, expected_codes,
                "case {case}: expected the responses {expected_codes:?}, got {codes:?}"
            );
            for (status, headers) in responses {
                let response = &exported[*status];
                assert_eq!(
                    response["description"].as_str(),
                    Some(format!("{status} response").as_str()),
                    "case {case}: expected the {status} response's description, got {response}"
                );
                let exported_headers = response["headers"].as_object().cloned().unwrap_or_default();
                let names = sorted_keys(&exported_headers);
                assert_eq!(
                    names, *headers,
                    "case {case}: expected the {status} headers {headers:?}, got {names:?}"
                );
                for name in *headers {
                    let expected = match *dialect {
                        "swagger" => json!({ "type": "string" }),
                        _ => json!({ "schema": { "type": "string" } }),
                    };
                    assert_eq!(
                        exported_headers[*name], expected,
                        "case {case}: expected the header {name} as {expected}, got {:?}",
                        exported_headers[*name]
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_methods_authorization_becomes_the_operations_security_and_the_documents_schemes() {
    // Rows differ along every `authorizationType` the method documentation lists, with
    // and without `apiKeyRequired`: the requirement names every scheme the method needs,
    // because the entries of one requirement object all have to be satisfied.
    let cases: Vec<(&str, &str, bool, Value, Vec<&str>)> = vec![
        ("open access", "NONE", false, Value::Null, vec![]),
        (
            "an API key",
            "NONE",
            true,
            json!({ "api_key": [] }),
            vec!["api_key"],
        ),
        (
            "IAM permissions",
            "AWS_IAM",
            false,
            json!({ "sigv4": [] }),
            vec!["sigv4"],
        ),
        (
            "IAM permissions and an API key",
            "AWS_IAM",
            true,
            json!({ "sigv4": [], "api_key": [] }),
            vec!["api_key", "sigv4"],
        ),
        (
            "a custom authorizer",
            "CUSTOM",
            false,
            json!({ "tokens": [] }),
            vec!["tokens"],
        ),
        (
            "a Cognito user pool with scopes",
            "COGNITO_USER_POOLS",
            false,
            json!({ "pools": ["pets/read"] }),
            vec!["pools"],
        ),
    ];

    for (case, authorization_type, api_key_required, requirement, scheme_names) in cases {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, case).await;
        let created = create_resource(&gateway, &api, &root, "pets").await;
        let pets = string_member(&created, "id", case);
        let mut request = json!({
            "authorizationType": authorization_type,
            "apiKeyRequired": api_key_required,
        });
        if authorization_type == "CUSTOM" {
            let answer = gateway
                .call(
                    "POST",
                    &format!("/restapis/{api}/authorizers"),
                    Some(json!({
                        "name": "tokens",
                        "type": "TOKEN",
                        "identitySource": "method.request.header.X-Auth",
                        "authorizerUri": "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/arn:aws:lambda:us-east-1:123456789012:function:auth/invocations",
                    })),
                )
                .await;
            let id = string_member(&answer.body, "id", case);
            request["authorizerId"] = json!(id);
        }
        if authorization_type == "COGNITO_USER_POOLS" {
            let answer = gateway
                .call(
                    "POST",
                    &format!("/restapis/{api}/authorizers"),
                    Some(json!({
                        "name": "pools",
                        "type": "COGNITO_USER_POOLS",
                        "identitySource": "method.request.header.Authorization",
                        "providerARNs": ["arn:aws:cognito-idp:us-east-1:123456789012:userpool/us-east-1_pets"],
                    })),
                )
                .await;
            let id = string_member(&answer.body, "id", case);
            request["authorizerId"] = json!(id);
            request["authorizationScopes"] = json!(["pets/read"]);
        }
        put_method(&gateway, &api, &pets, "GET", request).await;
        create_integration(&gateway, &api, &pets, "GET", json!({ "type": "MOCK" })).await;
        deploy(&gateway, &api, "dev").await;

        for dialect in DIALECTS {
            let case = format!("{case} as {dialect}");
            let document = export(&gateway, &api, "dev", dialect, None, &[])
                .await
                .json(&case);
            let security = &document["paths"]["/pets"]["get"]["security"];
            if requirement.is_null() {
                assert!(
                    security.is_null(),
                    "case {case}: a method needing no authorization carries no security, got \
                     {security}"
                );
            } else {
                assert_eq!(
                    security,
                    &json!([requirement]),
                    "case {case}: expected the security requirement {requirement}, got {security}"
                );
            }
            let schemes = document_schemes(&document, dialect);
            let names = sorted_keys(&schemes);
            assert_eq!(
                names, scheme_names,
                "case {case}: expected the schemes {scheme_names:?}, got {names:?}"
            );
            // Each scheme says where its value is carried and which kind of
            // authorization it stands for.
            for name in &scheme_names {
                let scheme = &schemes[*name];
                assert_eq!(
                    scheme["type"].as_str(),
                    Some("apiKey"),
                    "case {case}: expected an apiKey scheme for {name}, got {scheme}"
                );
                let expected_header = match *name {
                    "api_key" => "x-api-key",
                    "tokens" => "X-Auth",
                    _ => "Authorization",
                };
                assert_eq!(
                    scheme["name"].as_str(),
                    Some(expected_header),
                    "case {case}: expected {name} in the {expected_header} header, got {scheme}"
                );
                let expected_authtype = match *name {
                    "api_key" => None,
                    "sigv4" => Some("awsSigv4"),
                    "tokens" => Some("custom"),
                    _ => Some("cognito_user_pools"),
                };
                assert_eq!(
                    scheme["x-amazon-apigateway-authtype"].as_str(),
                    expected_authtype,
                    "case {case}: expected the authtype {expected_authtype:?} for {name}, got \
                     {scheme}"
                );
                // The authorizer extension is carried only when the request asks for it.
                assert!(
                    scheme.get("x-amazon-apigateway-authorizer").is_none(),
                    "case {case}: an export that did not ask for the authorizer extension must \
                     not carry it, got {scheme}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_vendor_extensions_are_carried_only_when_the_parameters_ask_for_them() {
    // Rows differ along every `extensions` value the member documents and the
    // combinations of them, including the comma-separated spelling of a combination.
    let cases: Vec<ExtensionCase<'_>> = vec![
        ("no parameters at all", &[], false, false),
        (
            "extensions=integrations",
            &[("extensions", "integrations")],
            true,
            false,
        ),
        (
            "extensions=apigateway",
            &[("extensions", "apigateway")],
            true,
            false,
        ),
        (
            "extensions=authorizers",
            &[("extensions", "authorizers")],
            false,
            true,
        ),
        (
            "both extension sets at once",
            &[("extensions", "apigateway,authorizers")],
            true,
            true,
        ),
        (
            "extensions=documentation, which asks for neither of these two",
            &[("extensions", "documentation")],
            false,
            false,
        ),
        (
            "a parameter the member documents no meaning for",
            &[("ignore", "documentation")],
            false,
            false,
        ),
        (
            "a postman flag turned off, which asks for no Postman extensions",
            &[("postman", "false")],
            false,
            false,
        ),
    ];

    for (case, parameters, integration_expected, authorizer_expected) in cases {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, case).await;
        let created = create_resource(&gateway, &api, &root, "pets").await;
        let pets = string_member(&created, "id", case);
        let answer = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/authorizers"),
                Some(json!({
                    "name": "tokens",
                    "type": "TOKEN",
                    "identitySource": "method.request.header.X-Auth",
                    "authorizerUri": "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/arn:aws:lambda:us-east-1:123456789012:function:auth/invocations",
                    "authorizerResultTtlInSeconds": 120,
                })),
            )
            .await;
        let authorizer = string_member(&answer.body, "id", case);
        put_method(
            &gateway,
            &api,
            &pets,
            "GET",
            json!({ "authorizationType": "CUSTOM", "authorizerId": authorizer }),
        )
        .await;
        create_integration(
            &gateway,
            &api,
            &pets,
            "GET",
            json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
            }),
        )
        .await;
        deploy(&gateway, &api, "dev").await;

        for dialect in DIALECTS {
            let case = format!("{case} as {dialect}");
            let document = export(&gateway, &api, "dev", dialect, None, parameters)
                .await
                .json(&case);
            let integration =
                &document["paths"]["/pets"]["get"]["x-amazon-apigateway-integration"];
            assert_eq!(
                !integration.is_null(),
                integration_expected,
                "case {case}: expected the integration extension present={integration_expected}, \
                 got {integration}"
            );
            if integration_expected {
                // The extension carries the stored integration, with the type written the
                // way the extension writes it.
                assert_eq!(
                    integration["type"].as_str(),
                    Some("mock"),
                    "case {case}: expected the stored integration type, got {integration}"
                );
                assert_eq!(
                    integration["requestTemplates"]["application/json"].as_str(),
                    Some("{\"statusCode\": 200}"),
                    "case {case}: expected the stored request template, got {integration}"
                );
            }
            let scheme = &document_schemes(&document, dialect)["tokens"];
            let extension = &scheme["x-amazon-apigateway-authorizer"];
            assert_eq!(
                !extension.is_null(),
                authorizer_expected,
                "case {case}: expected the authorizer extension present={authorizer_expected}, \
                 got {scheme}"
            );
            if authorizer_expected {
                assert_eq!(
                    extension["type"].as_str(),
                    Some("token"),
                    "case {case}: expected the stored authorizer type, got {extension}"
                );
                assert_eq!(
                    extension["authorizerResultTtlInSeconds"].as_i64(),
                    Some(120),
                    "case {case}: expected the stored authorizer TTL, got {extension}"
                );
                assert_eq!(
                    extension["identitySource"].as_str(),
                    Some("method.request.header.X-Auth"),
                    "case {case}: expected the stored identity source, got {extension}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_method_whose_authorizer_record_is_missing_still_exports_its_authorization() {
    // A method's `authorizerId` names an `Authorizer` the api holds — `PutMethod` resolves
    // it — so the one state in which a method's authorizer record is MISSING is a DEPLOYED
    // SNAPSHOT: the stage serves the tree as it was deployed, and the authorizer that tree
    // named has since been deleted along with the method that named it. An export of that
    // stage must not describe such a method as an open one: it names the authorization the
    // METHOD carries. Rows differ along the authorization type.
    let cases: Vec<(&str, &str, &str)> = vec![
        (
            "a custom authorization whose authorizer record is gone",
            "CUSTOM",
            "custom",
        ),
        (
            "a Cognito authorization whose authorizer record is gone",
            "COGNITO_USER_POOLS",
            "cognito_user_pools",
        ),
    ];

    for (case, authorization_type, authtype) in cases {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, case).await;
        let created = create_resource(&gateway, &api, &root, "pets").await;
        let pets = string_member(&created, "id", case);
        let answer = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/authorizers"),
                Some(json!({
                    "name": "to-be-deleted",
                    "type": "TOKEN",
                    "identitySource": "method.request.header.X-Auth",
                    "authorizerUri": "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/arn:aws:lambda:us-east-1:123456789012:function:auth/invocations",
                })),
            )
            .await;
        let authorizer_id = string_member(&answer.body, "id", case);
        put_method(
            &gateway,
            &api,
            &pets,
            "GET",
            json!({
                "authorizationType": authorization_type,
                "authorizerId": authorizer_id.clone(),
            }),
        )
        .await;
        create_integration(&gateway, &api, &pets, "GET", json!({ "type": "MOCK" })).await;
        deploy(&gateway, &api, "dev").await;

        // The live tree gives the reference up; the deployed snapshot keeps it.
        for (verb, uri) in [
            (
                "DELETE",
                format!("/restapis/{api}/resources/{pets}/methods/GET"),
            ),
            (
                "DELETE",
                format!("/restapis/{api}/authorizers/{authorizer_id}"),
            ),
        ] {
            let removed = gateway.call(verb, &uri, None).await;
            assert!(
                (200..300).contains(&removed.status),
                "case {case}: expected {uri} to be deleted, got {} with {:?}",
                removed.status,
                removed.body
            );
        }

        for dialect in DIALECTS {
            let case = format!("{case} as {dialect}");
            let document = export(&gateway, &api, "dev", dialect, None, &[])
                .await
                .json(&case);
            let security = &document["paths"]["/pets"]["get"]["security"];
            assert_eq!(
                security,
                &json!([{ authorizer_id.clone(): [] }]),
                "case {case}: expected the method's authorization to be named, got {security}"
            );
            let schemes = document_schemes(&document, dialect);
            assert_eq!(
                sorted_keys(&schemes),
                vec![authorizer_id.as_str()],
                "case {case}: expected the scheme {authorizer_id}, got {schemes:?}"
            );
            assert_eq!(
                schemes[&authorizer_id]["x-amazon-apigateway-authtype"].as_str(),
                Some(authtype),
                "case {case}: expected the authtype {authtype}, got {:?}",
                schemes[&authorizer_id]
            );
        }
    }
}

#[tokio::test]
async fn an_export_is_serialized_in_the_media_type_accepts_asks_for() {
    // Rows differ along the `Accept` value: absent, each documented media type, the
    // spelling with a parameter, the case-insensitive spelling, and the wildcard a client
    // sends when it has no preference. The third column is the serialization the row asks
    // for, which shows in the BODY and in the attachment's filename extension — never in
    // the response's own content type, which is the attachment's byte stream for every row.
    let cases: Vec<(&str, Option<&str>, &str)> = vec![
        ("no Accept header", None, "application/json"),
        ("Accept: application/json", Some("application/json"), "application/json"),
        (
            "Accept with a parameter",
            Some("application/json; charset=utf-8"),
            "application/json",
        ),
        ("Accept: */*", Some("*/*"), "application/json"),
        ("Accept: application/yaml", Some("application/yaml"), "application/yaml"),
        (
            "Accept in upper case",
            Some("APPLICATION/YAML"),
            "application/yaml",
        ),
        (
            "Accept with a second range",
            Some("application/yaml, application/json"),
            "application/yaml",
        ),
        (
            "Accept whose first range has the lower quality",
            Some("application/json;q=0.1, application/yaml"),
            "application/yaml",
        ),
        (
            "Accept whose first range is a media type this service does not serialize",
            Some("text/csv, application/yaml"),
            "application/yaml",
        ),
        (
            "Accept that rules a range out with q=0",
            Some("application/yaml;q=0, application/json"),
            "application/json",
        ),
        (
            "Accept with a wildcard subtype",
            Some("application/*"),
            "application/json",
        ),
    ];

    let gateway = Gateway::new();
    let (api, _) = api_with_paths(&gateway, "media types", &[("/pets", &["GET"])]).await;
    deploy(&gateway, &api, "dev").await;

    for (case, accepts, expected) in cases {
        for dialect in DIALECTS {
            let case = format!("{case} as {dialect}");
            let answer = export(&gateway, &api, "dev", dialect, accepts, &[]).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.content_type.as_deref(),
                Some("application/octet-stream"),
                "case {case}: expected the export to be served as an attachment's byte \
                 stream, got {:?}",
                answer.content_type
            );
            let extension = if expected == "application/yaml" {
                "yaml"
            } else {
                "json"
            };
            let disposition = answer.content_disposition.clone().unwrap_or_default();
            assert!(
                disposition.starts_with(&format!("attachment; filename=\"{dialect}_"))
                    && disposition.ends_with(&format!(".{extension}\"")),
                "case {case}: expected an attachment named for the export, got {disposition:?}"
            );
            let text = String::from_utf8(answer.body.clone()).unwrap_or_else(|error| {
                panic!("case {case}: expected a UTF-8 export, got {error}")
            });
            // Whichever serialization was asked for, the document is the same one.
            let version_key = if *dialect == "swagger" {
                "swagger"
            } else {
                "openapi"
            };
            if expected == "application/yaml" {
                let documents = ::yaml_rust2::yaml::YamlLoader::load_from_str(&text)
                    .unwrap_or_else(|error| {
                        panic!("case {case}: expected a YAML export, got {error} for {text}")
                    });
                let document = documents
                    .first()
                    .unwrap_or_else(|| panic!("case {case}: expected one YAML document"));
                assert!(
                    document[version_key].as_str().is_some(),
                    "case {case}: expected a {version_key} version in the YAML export, got {text}"
                );
                assert!(
                    document["paths"]["/pets"]["get"].as_hash().is_some(),
                    "case {case}: expected the deployed operation in the YAML export, got {text}"
                );
            } else {
                let document: Value = serde_json::from_str(&text).unwrap_or_else(|error| {
                    panic!("case {case}: expected a JSON export, got {error} for {text}")
                });
                assert!(
                    document[version_key].as_str().is_some(),
                    "case {case}: expected a {version_key} version in the JSON export, got {text}"
                );
                assert!(
                    document["paths"]["/pets"]["get"].is_object(),
                    "case {case}: expected the deployed operation in the JSON export, got {text}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_export_this_service_cannot_produce_is_refused_rather_than_approximated() {
    let gateway = Gateway::new();
    let (api, _) = api_with_paths(&gateway, "refusals", &[("/pets", &["GET"])]).await;
    deploy(&gateway, &api, "dev").await;

    // Rows differ along WHICH part of the request names something that does not exist or
    // is not produced: the api, the stage, the export type, the media type, the extension
    // set, and the Postman extensions this emulator does not generate.
    let cases: Vec<RefusalCase<'_>> = vec![
        (
            "an api that does not exist",
            "deadbeef00",
            "dev",
            "oas30",
            None,
            &[],
            404,
            "NotFoundException",
        ),
        (
            "a stage that does not exist",
            api.as_str(),
            "staging",
            "oas30",
            None,
            &[],
            404,
            "NotFoundException",
        ),
        (
            "an export type outside the documented values",
            api.as_str(),
            "dev",
            "raml",
            None,
            &[],
            400,
            "BadRequestException",
        ),
        (
            "the export type in the wrong case",
            api.as_str(),
            "dev",
            "OAS30",
            None,
            &[],
            400,
            "BadRequestException",
        ),
        (
            "a media type outside the documented values",
            api.as_str(),
            "dev",
            "oas30",
            Some("text/csv"),
            &[],
            400,
            "BadRequestException",
        ),
        (
            "every media range outside the documented values",
            api.as_str(),
            "dev",
            "oas30",
            Some("text/csv;q=0.9, application/xml"),
            &[],
            400,
            "BadRequestException",
        ),
        (
            "an Accept that excludes every range it names",
            api.as_str(),
            "dev",
            "oas30",
            Some("application/json;q=0, application/yaml;q=0"),
            &[],
            400,
            "BadRequestException",
        ),
        (
            "an extensions value outside the documented values",
            api.as_str(),
            "dev",
            "oas30",
            None,
            &[("extensions", "intergrations")],
            400,
            "BadRequestException",
        ),
        (
            "the Postman extensions asked for as the extensions value the member documents",
            api.as_str(),
            "dev",
            "oas30",
            None,
            &[("extensions", "postman")],
            501,
            "NotImplemented",
        ),
        (
            "the Postman extensions this emulator does not generate",
            api.as_str(),
            "dev",
            "oas30",
            None,
            &[("postman", "true")],
            501,
            "NotImplemented",
        ),
        (
            "the Postman extensions asked for by the valueless spelling of the flag",
            api.as_str(),
            "dev",
            "oas30",
            None,
            &[("postman", "")],
            501,
            "NotImplemented",
        ),
    ];

    for (case, api_id, stage, export_type, accepts, parameters, status, code) in cases {
        let answer = export(&gateway, api_id, stage, export_type, accepts, parameters).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected {status}, got {} with {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error.as_deref(),
            Some(code),
            "case {case}: expected {code}, got {:?}",
            answer.error
        );
    }
}

#[tokio::test]
async fn exporting_a_stage_twice_answers_the_same_document() {
    let gateway = Gateway::new();
    let (api, by_path) = api_with_paths(&gateway, "reporting", &[("/pets", &["GET"])]).await;
    let pets = by_path["/pets"].clone();
    create_method_response(
        &gateway,
        &api,
        &pets,
        "GET",
        "200",
        json!({ "responseParameters": { "method.response.header.X-Count": true } }),
    )
    .await;
    create_integration(&gateway, &api, &pets, "GET", json!({ "type": "MOCK" })).await;
    deploy(&gateway, &api, "dev").await;

    for dialect in DIALECTS {
        let case = format!("two exports as {dialect}");
        let first = export(
            &gateway,
            &api,
            "dev",
            dialect,
            None,
            &[("extensions", "integrations")],
        )
        .await
        .json(&case);
        let second = export(
            &gateway,
            &api,
            "dev",
            dialect,
            None,
            &[("extensions", "integrations")],
        )
        .await
        .json(&case);
        assert_eq!(
            first, second,
            "case {case}: GetExport reports and writes nothing, so two exports of one stage \
             must be the same document, got {first} then {second}"
        );
    }
}

#[tokio::test]
async fn the_gateway_responses_configured_on_the_api_are_part_of_the_exported_definition() {
    // Rows differ along what the rule generalises over: which response types are
    // customized and how many, whether the customization sets a status code or keeps the
    // one its type generates (`DEFAULT_4XX` and `DEFAULT_5XX` generate none at all),
    // whether it carries response parameters, response templates, both or neither, and
    // how many entries each of those maps has. The CORS case — the headers a
    // cross-origin API answers an error with — is one row of that table, not its subject.
    let cases: Vec<GatewayResponseCase<'_>> = vec![
        ("no customization at all", vec![], Value::Null),
        (
            "one type with a cross-origin header",
            vec![(
                "DEFAULT_4XX",
                json!({
                    "responseParameters": {
                        "gatewayresponse.header.Access-Control-Allow-Origin": "'*'"
                    }
                }),
            )],
            json!({
                "DEFAULT_4XX": {
                    "responseParameters": {
                        "gatewayresponse.header.Access-Control-Allow-Origin": "'*'"
                    }
                }
            }),
        ),
        (
            "both default classes, with several headers each",
            vec![
                (
                    "DEFAULT_4XX",
                    json!({
                        "responseParameters": {
                            "gatewayresponse.header.Access-Control-Allow-Origin": "'https://example.com'",
                            "gatewayresponse.header.Access-Control-Allow-Headers": "'Content-Type'"
                        }
                    }),
                ),
                (
                    "DEFAULT_5XX",
                    json!({
                        "responseParameters": {
                            "gatewayresponse.header.Access-Control-Allow-Origin": "'https://example.com'"
                        },
                        "responseTemplates": {
                            "application/json": "{\"message\":$context.error.messageString}"
                        }
                    }),
                ),
            ],
            json!({
                "DEFAULT_4XX": {
                    "responseParameters": {
                        "gatewayresponse.header.Access-Control-Allow-Headers": "'Content-Type'",
                        "gatewayresponse.header.Access-Control-Allow-Origin": "'https://example.com'"
                    }
                },
                "DEFAULT_5XX": {
                    "responseParameters": {
                        "gatewayresponse.header.Access-Control-Allow-Origin": "'https://example.com'"
                    },
                    "responseTemplates": {
                        "application/json": "{\"message\":$context.error.messageString}"
                    }
                }
            }),
        ),
        (
            "a type whose generated status code the put keeps",
            vec![("UNAUTHORIZED", json!({}))],
            // `UNAUTHORIZED` generates a 401, which a put that sets no status code keeps.
            json!({ "UNAUTHORIZED": { "statusCode": "401" } }),
        ),
        (
            "a customization that overrides the status code and the body",
            vec![(
                "MISSING_AUTHENTICATION_TOKEN",
                json!({
                    "statusCode": "404",
                    "responseTemplates": { "application/json": "{\"message\":\"not here\"}" }
                }),
            )],
            json!({
                "MISSING_AUTHENTICATION_TOKEN": {
                    "statusCode": "404",
                    "responseTemplates": { "application/json": "{\"message\":\"not here\"}" }
                }
            }),
        ),
    ];

    // The extension belongs to API Gateway's own extension set, so every row is asserted
    // with the value that asks for that set AND with each other `parameters` spelling the
    // member documents, where the definition carries no such extension. The third column
    // of a row is what the FIRST of these sets must carry.
    let parameter_sets: Vec<(Parameters<'_>, bool)> = vec![
        (&[("extensions", "apigateway")], true),
        (&[("extensions", "apigateway,authorizers")], true),
        (&[], false),
        (&[("extensions", "integrations")], false),
        (&[("extensions", "authorizers")], false),
    ];

    for (case, customizations, expected) in cases {
        let gateway = Gateway::new();
        let (api, _) = api_with_paths(&gateway, "gateway responses", &[("/pets", &["GET"])]).await;
        for (response_type, request) in &customizations {
            let created =
                put_gateway_response(&gateway, &api, response_type, request.clone()).await;
            assert_eq!(
                created.status, 201,
                "case {case}/{response_type}: expected 201 from PutGatewayResponse, got {} \
                 with {:?}",
                created.status, created.body
            );
        }
        deploy(&gateway, &api, "dev").await;

        for dialect in DIALECTS {
            for (parameters, asked_for) in &parameter_sets {
                let case = format!("{case} as {dialect} with {parameters:?}");
                let document = export(&gateway, &api, "dev", dialect, None, parameters)
                    .await
                    .json(&case);
                let carried = &document["x-amazon-apigateway-gateway-responses"];
                if expected.is_null() || !asked_for {
                    assert!(
                        carried.is_null(),
                        "case {case}: the gateway-response extension is carried only by an \
                         export that asked for API Gateway's extensions of an API that \
                         configured a gateway response, got {carried}"
                    );
                    continue;
                }
                assert_eq!(
                    carried, &expected,
                    "case {case}: expected the configured gateway responses {expected}, got \
                     {carried}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_export_type_no_exporter_renders_is_refused_naming_the_type_asked_for() {
    // An export type names the EXPORTER that renders the definition, so a value naming
    // none is refused with that value in the message. Rows differ along the shape of the
    // value: a name no format has, a documented type in the wrong case (the two values the
    // member publishes are lower case), another name for one of the dialects, a version
    // number, and the empty-looking value a caller sends by mistake.
    let cases = ["not-a-real-type", "OAS30", "openapi", "3.0.1", "json"];

    let gateway = Gateway::new();
    let (api, _) = api_with_paths(&gateway, "export types", &[("/pets", &["GET"])]).await;
    deploy(&gateway, &api, "dev").await;

    for export_type in cases {
        let answer = export(&gateway, &api, "dev", export_type, None, &[]).await;
        assert_eq!(
            answer.status, 400,
            "case {export_type}: expected 400, got {} with {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("BadRequestException"),
            "case {export_type}: expected a BadRequestException, got {:?}",
            answer.error
        );
        let body = String::from_utf8_lossy(&answer.body).to_string();
        assert!(
            body.contains(&format!("No API exporter for type '{export_type}'")),
            "case {export_type}: expected the refusal to name the export type asked for, \
             got {body}"
        );
    }

    // The two the member publishes are rendered, which is what makes the rows above a
    // refusal of what has no exporter rather than of everything.
    for export_type in DIALECTS {
        let answer = export(&gateway, &api, "dev", export_type, None, &[]).await;
        assert_eq!(
            answer.status, 200,
            "case {export_type}: expected 200, got {} with {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
}
