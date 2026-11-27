// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `execute-api` DATA plane: a request addressed to a deployed
//! REST API stage's invoke URL.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service, on the api's own
//! `{rest-api-id}.execute-api.{region}.amazonaws.com` host — so what is asserted is what a
//! client of the published invoke URL sees, not what a handler returns. The control plane
//! is driven through the same emulator ([`Gateway`], this crate's one wire-test harness) to
//! build the records the data plane then resolves, so nothing here reaches into state.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * every request to a deployed stage is ANSWERED, and which status it gets is decided by
//!   the api's own records, over a table of paths (root, literal, path parameter, greedy
//!   parameter, unmatched), verbs (the method's own, the `ANY` catch-all, one not
//!   deployed), and stages (deployed, unknown) — plus the deployment-snapshot rule in both
//!   directions, by redeploying: both for the methods the deployment holds and for the
//!   CONFIGURATION of a method it holds, which an undeployed edit does not change;
//! * a `MOCK` integration's response is built from the selected integration response, over
//!   a table of status codes, response templates, negotiated content types and header
//!   mappings;
//! * the method's deployed `authorizationType` and `apiKeyRequired` decide the refusal,
//!   over a table spanning each documented authorization type and each way an API key can
//!   fail to admit a call;
//! * an integration that names a backend this emulator cannot call is answered as an
//!   integration that could not be performed, over a table of the integration types and
//!   configurations that cannot be served;
//! * a gateway-response customization of the refused type reaches the caller;
//! * `PutIntegrationResponse` round-trips through `GetIntegration`, and its refusals are
//!   the modelled ones.

use bytes::Bytes;
use serde_json::{json, Value};

use crate::rest_api_lifecycle::{create_api, create_resource, string_member, Gateway};

/// The TRANSFORMATIONS a STANDARD (non-proxy) `HTTP` integration of this plane performs on
/// the request it makes and the response it answers — `contentHandling`, the override
/// directives a mapping template states and the `415` of `passthroughBehavior` — in their own
/// file beside this one: the other half of what a deployed method's integration does with a
/// call, and this suite's sibling concern. `tests/all.rs` declares one module per suite FILE,
/// and this is that file's module; the `#[path]` points it into this suite's own directory, as
/// that file's own declaration points into `tests/all/`.
#[path = "execute_api_data_plane/nonproxy_integration_transformations.rs"]
mod nonproxy_integration_transformations;

/// WHICH REQUEST a deployed stage says it answered — the request id a client may state for
/// itself and the extended id the endpoint always mints — in its own file beside this one,
/// declared as the sibling above is.
#[path = "execute_api_data_plane/stated_request_identity.rs"]
mod stated_request_identity;

/// What a response template may override about the STATUS the method response is sent with,
/// in its own file beside this one, declared as the siblings above are.
#[path = "execute_api_data_plane/response_override_status_type.rs"]
mod response_override_status_type;

/// That each MEMBER one response template's override object states is applied on its own type
/// — a discarded status leaving the headers stated beside it untouched — in its own file beside
/// this one, declared as the siblings above are.
#[path = "execute_api_data_plane/override_member_independence.rs"]
mod override_member_independence;

/// WHICH INVOCATION of its Lambda function a method's integration makes — the `InvocationType`
/// its `requestParameters` map onto the integration request — and what the caller is served
/// for one made for no answer, in its own file beside this one, declared as the siblings above
/// are.
#[path = "execute_api_data_plane/lambda_event_invocation_type.rs"]
mod lambda_event_invocation_type;

/// WHAT THE CALLER IS SERVED for an invocation Lambda answers before the function has produced
/// anything — the method response the integration's own responses map that payload-less answer
/// onto — in its own file beside this one, declared as the siblings above are.
#[path = "execute_api_data_plane/accepted_invocation_method_response.rs"]
mod accepted_invocation_method_response;

/// What a `TOKEN` authorizer's own `identityValidationExpression` decides about the identity a
/// request presents, BEFORE its function is invoked, in its own file beside this one, declared
/// as the siblings above are.
#[path = "execute_api_data_plane/token_authorizer_identity_validation.rs"]
mod token_authorizer_identity_validation;

/// WHICH RELEASE of an api one request to a stage carrying a CANARY is served from, and what a
/// non-proxy `HTTP` integration's request and response templates make of a real body, in their
/// own file beside this one, declared as the siblings above are.
#[path = "execute_api_data_plane/canary_release_and_vtl_transformation.rs"]
mod canary_release_and_vtl_transformation;

/// The region the invoke host is addressed in. Any region is served — the host's region
/// label is not matched against a list — so one is enough to drive the data plane, and
/// `src/service/invoke.rs`'s own unit test varies the label.
const REGION: &str = "us-east-1";

/// The body a refusal the gateway sets NO error message for is answered with: the response
/// type's own default template rendered with the message unset, so the `message` member is the
/// JSON literal `null`. The whole class is pinned by
/// [`crate::stage_authorizer_failure_body`]; here it is the answer one row owes.
const NO_AUTHORIZER_DECISION: &str = r#"{"message":null}"#;

/// One answered invoke: the status line, the headers asserted on, and the body verbatim.
///
/// `pub(crate)` with [`invoke`], so the sibling `test_invoke_method_data_plane_parity`
/// suite compares a SIMULATED invocation against what this endpoint serves for the same
/// records through this one reader rather than a second one of its own.
pub(crate) struct Invoked {
    pub(crate) status: u16,
    headers: std::collections::HashMap<String, String>,
    pub(crate) body: String,
}

impl Invoked {
    /// One response header, lowercased as HTTP names compare.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// The `message` member of a JSON body, which is what every API Gateway gateway
    /// response carries; `None` for a body that is not such an object.
    pub(crate) fn message(&self) -> Option<String> {
        serde_json::from_str::<Value>(&self.body)
            .ok()?
            .get("message")?
            .as_str()
            .map(str::to_owned)
    }
}

/// The invoke host of `api`: the endpoint `RestApi$disableExecuteApiEndpoint` documents,
/// `https://{api_id}.execute-api.{region}.amazonaws.com`.
fn invoke_host(api: &str) -> String {
    format!("{api}.execute-api.{REGION}.amazonaws.com")
}

/// Issue one request to `api`'s invoke endpoint at `path`, carrying `headers` and no body.
///
/// `host` is the api's own endpoint rather than the control plane's, which is the whole
/// point: the data plane is reached by HOST.
///
/// `pub(crate)` for the sibling `test_invoke_method_data_plane_parity` suite; see
/// [`Invoked`].
pub(crate) async fn invoke(
    gateway: &Gateway,
    api: &str,
    verb: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Invoked {
    invoke_on_host(gateway, &invoke_host(api), verb, path, headers).await
}

/// [`invoke`] against an arbitrary host, for the cases that assert which hosts this
/// service does NOT claim.
///
/// `pub(crate)` for the sibling `private_api_resource_policy` suite, which addresses a
/// PRIVATE api on its VPC endpoint's own DNS name rather than on the api's — a host this
/// helper's caller chooses, with the api id in a header.
pub(crate) async fn invoke_on_host(
    gateway: &Gateway,
    host: &str,
    verb: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Invoked {
    let mut builder = http::Request::builder()
        .method(verb)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::new())
        .expect("the invoke request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_owned()))
        })
        .collect();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    Invoked {
        status,
        headers,
        body,
    }
}

/// A SigV4 credential scope naming this service, as a signed SDK request carries it. The
/// signature itself is the emulator's business and is not verified in these tests (the
/// harness runs the router with request authentication off), so only its PRESENCE is what
/// a case varies.
const SIGNED: &str =
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/apigateway/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// The POLICY DOCUMENT an api's reported `policy` member carries: the member is "a
/// stringified JSON policy document", reported as the document's text ENCODED as a JSON
/// string, so a reader decodes that string once to get the document back.
///
/// `pub(crate)` because every suite that sets a resource policy and reads it back decodes
/// it the same way (`private_api_resource_policy`, `private_proxy_backend_answer`,
/// `stage_resource_policy_source_ip`).
pub(crate) fn policy_document_of(reported: &str, case: &str) -> String {
    ::serde_json::from_str::<String>(&format!("\"{reported}\"")).unwrap_or_else(|error| {
        panic!(
            "case {case}: the reported policy {reported:?} must be a JSON-encoded string, got \
             {error}"
        )
    })
}

/// The document the api STORES for `document`: API Gateway NORMALIZES a resource policy
/// before storing it, expanding every `Resource`/`NotResource` written in the
/// `execute-api:/{stage}/{verb}/{path}` shorthand to the ARN of the method of this api it
/// names — `arn_prefix` is `arn:aws:execute-api:{region}:{account}:{api-id}`, which the
/// shorthand's own leading `/` follows. Every other value is stored as it was written.
///
/// `pub(crate)` for the same suites: a document each writes in the shorthand is read back in
/// this form.
pub(crate) fn stored_policy_document(document: &Value, arn_prefix: &str) -> String {
    let mut document = document.clone();
    if let Some(statements) = document.get_mut("Statement").and_then(Value::as_array_mut) {
        for statement in statements {
            for element in ["Resource", "NotResource"] {
                match statement.get_mut(element) {
                    Some(Value::String(resource)) => {
                        *resource = expanded_resource(resource, arn_prefix);
                    }
                    Some(Value::Array(resources)) => {
                        for resource in resources.iter_mut() {
                            if let Value::String(resource) = resource {
                                *resource = expanded_resource(resource, arn_prefix);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    document.to_string()
}

/// One resource value as the api stores it; see [`stored_policy_document`].
fn expanded_resource(resource: &str, arn_prefix: &str) -> String {
    match resource.strip_prefix("execute-api:") {
        Some(path) if path.starts_with('/') => format!("{arn_prefix}{path}"),
        _ => resource.to_string(),
    }
}

/// `PutMethod` for `verb` on `resource`, with `request` as its body.
///
/// `pub(crate)` because the sibling `stage_export` suite stands a method up the same way
/// before deploying it, so both suites put a method through one helper.
pub(crate) async fn put_method(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    request: Value,
) {
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
            Some(request),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutMethod({resource}/{verb}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `PutIntegration` for `verb` on `resource`, with `request` as its body.
///
/// `pub(crate)` for the sibling suites that stand a `MOCK` method up to invoke it
/// (`stage_request_validation`).
pub(crate) async fn put_integration(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    request: Value,
) {
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
            Some(request),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutIntegration({resource}/{verb}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `PutIntegrationResponse` for `status` on the integration of `verb`.
async fn put_integration_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    request: Value,
) -> crate::rest_api_lifecycle::Answer {
    gateway
        .call(
            "PUT",
            &format!(
                "/restapis/{api}/resources/{resource}/methods/{verb}/integration/responses/{status}"
            ),
            Some(request),
        )
        .await
}

/// `PutIntegrationResponse`, asserting the modelled 201.
///
/// `pub(crate)` for the sibling suites that stand a `MOCK` method up to invoke it.
pub(crate) async fn integration_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    request: Value,
) {
    let answer = put_integration_response(gateway, api, resource, verb, status, request).await;
    assert_eq!(
        answer.status, 201,
        "case PutIntegrationResponse({resource}/{verb}/{status}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `PutMethodResponse` for `status` on `verb`, asserting the modelled 201.
///
/// `pub(crate)` for the sibling suites that stand a `MOCK` method up to invoke it.
pub(crate) async fn method_response(
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
        "case PutMethodResponse({resource}/{verb}/{status}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `CreateDeployment` of `api` to `stage`, asserting the modelled 201 and answering the
/// deployment id.
///
/// `pub(crate)` for the sibling `stage_export` suite: an export is of what a stage SERVES,
/// so every case there deploys through this same helper.
pub(crate) async fn deploy(gateway: &Gateway, api: &str, stage: &str) -> String {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": stage })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateDeployment({api} -> {stage}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", "CreateDeployment")
}

/// A resource under `parent` carrying one `MOCK` method that answers `status` with
/// `templates` as its response templates, answering the resource id.
///
/// This is the one integration type that needs no backend — "for testing without actually
/// invoking the backend" (`IntegrationType`) — so it is what a data-plane case that asserts
/// a SERVED response is built from.
///
/// `pub(crate)` for the sibling `private_api_resource_policy` suite, which stands one up to
/// assert that a request its api's resource policy refuses never reaches it.
pub(crate) async fn mock_resource(
    gateway: &Gateway,
    api: &str,
    parent: &str,
    path_part: &str,
    verb: &str,
    status: &str,
    templates: Value,
) -> String {
    let resource = create_resource(gateway, api, parent, path_part).await;
    let resource_id = string_member(&resource, "id", path_part);
    mock_method(gateway, api, &resource_id, verb, status, templates).await;
    resource_id
}

/// One `MOCK` method on an existing resource: the method, its integration, the integration
/// response that maps the call and the method response the status is declared by.
///
/// `pub(crate)` for the sibling suites that stand one up on a resource they did not create
/// — the api's ROOT resource, which every api is created with (`deployment_mints_the_missing_stage`).
pub(crate) async fn mock_method(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    templates: Value,
) {
    put_method(gateway, api, resource, verb, json!({ "authorizationType": "NONE" })).await;
    put_integration(gateway, api, resource, verb, json!({ "type": "MOCK" })).await;
    method_response(gateway, api, resource, verb, status, json!({})).await;
    integration_response(
        gateway,
        api,
        resource,
        verb,
        status,
        json!({ "responseTemplates": templates }),
    )
    .await;
}

/// A JSON response-template map answering `body` for `application/json` callers.
///
/// `pub(crate)` for the sibling `private_api_resource_policy` suite; see [`mock_resource`].
pub(crate) fn json_template(body: &str) -> Value {
    json!({ "application/json": body })
}

/// The response header the two branches below map their own value for: declared by each
/// method response (a header is declarable only there) and given its value by the integration
/// response that was selected, so an answer says WHICH branch it was mapped by.
pub(crate) const ROUTE_HEADER: &str = "X-Sim-Route";

/// The branch a result no `selectionPattern` covers maps to, keyed by `200`.
pub(crate) const CATCH_ALL_ROUTE: &str = "internal";

/// The branch covering the 5xx class, keyed by `500`.
pub(crate) const PATTERNED_ROUTE: &str = "external";

/// The pattern that branch declares: the whole 5xx class, so it covers every result of that
/// class and not only the status it is keyed by.
pub(crate) const SERVER_CLASS_PATTERN: &str = r"5\d{2}";

/// The body the branch for `route` renders — the value that tells which mapping was applied.
///
/// `pub(crate)` with [`branch_records`]: one fixture for every suite that asserts WHICH
/// branch a `MOCK`'s result was mapped by.
pub(crate) fn branch_body(route: &str) -> String {
    format!("{{\"route\":\"{route}\"}}")
}

/// The TWO BRANCHES a `MOCK`'s result is selected between, as
/// `(statusCode, PutMethodResponse request, PutIntegrationResponse request)` triples: the
/// catch-all keyed `200`, which is what a result no pattern covers maps to, and the one keyed
/// `500` whose [`SERVER_CLASS_PATTERN`] covers the 5xx class. Each renders [`branch_body`] of
/// its own name and maps its own value for the one header both method responses declare.
///
/// `empty_default_pattern` spells the catch-all's own `selectionPattern` as the EMPTY STRING
/// instead of leaving it out: `selectionPattern` is "the regular expression (regex) pattern
/// used to choose an integration response based on the response from the back end", so a
/// response declaring neither is chosen by no result in particular and both spellings are the
/// catch-all.
///
/// `pub(crate)` for every suite that needs that pair — the deployed stage's wire
/// (`mock_rendered_status_selection`), a `TestInvokeMethod` simulation of the same records
/// (`test_invoke_method_data_plane_parity`) and a template branching on the request's verb
/// (`stage_verb_precedence`) — so one fixture is configured once.
pub(crate) fn branch_records(empty_default_pattern: bool) -> Vec<(&'static str, Value, Value)> {
    let parameter = format!("method.response.header.{ROUTE_HEADER}");
    let mut declared = ::serde_json::Map::new();
    declared.insert(parameter.clone(), json!(true));
    let declared = json!({ "responseParameters": Value::Object(declared) });

    let mut branches = Vec::with_capacity(2);
    for (status, route, pattern) in [
        ("200", CATCH_ALL_ROUTE, empty_default_pattern.then_some("")),
        ("500", PATTERNED_ROUTE, Some(SERVER_CLASS_PATTERN)),
    ] {
        let mut mapped = ::serde_json::Map::new();
        mapped.insert(parameter.clone(), json!(format!("'{route}'")));
        let mut response = json!({
            "responseTemplates": json_template(&branch_body(route)),
            "responseParameters": Value::Object(mapped),
        });
        if let Some(pattern) = pattern {
            response["selectionPattern"] = json!(pattern);
        }
        branches.push((status, declared.clone(), response));
    }
    branches
}

#[tokio::test]
async fn an_integration_responses_selection_pattern_is_evaluated_server_side() {
    // `IntegrationResponse$selectionPattern` is "the regular expression (regex) pattern used
    // to choose an integration response based on the response from the back end". A `MOCK`
    // has no back end: its request template IS what the integration answered, and the
    // `statusCode` that rendering names is the result the patterns are matched against — so
    // a `5\d{2}` branch surfaces as a real 500 on the wire, with its own body and its own
    // mapped header, and the branch with NO pattern is the one a result no pattern covers
    // maps to.
    struct Case {
        name: &'static str,
        /// The `requestTemplates` entry of the `MOCK` integration, which for a `MOCK` is the
        /// integration's own answer.
        request_template: &'static str,
        expected_status: u16,
        expected_body: &'static str,
        /// The mapped header the selected branch declares, by lowercased name.
        expected_header: Option<(&'static str, &'static str)>,
    }

    // Rows differ along what the rule generalises over: which result the integration
    // produced — one the unpatterned branch answers, one each patterned branch covers, one a
    // patterned branch covers and maps ONTO ANOTHER status, a result no pattern covers at
    // all, and a rendering that names no result.
    let cases = [
        Case {
            name: "a result the unpatterned branch answers",
            request_template: r#"{"statusCode": 200}"#,
            expected_status: 200,
            expected_body: r#"{"route":"internal"}"#,
            expected_header: None,
        },
        Case {
            name: "a result the 5xx branch covers",
            request_template: r#"{"statusCode": 500}"#,
            expected_status: 500,
            expected_body: r#"{"branch":"server"}"#,
            expected_header: Some(("x-branch", "server")),
        },
        Case {
            name: "a result the 4xx branch covers",
            request_template: r#"{"statusCode": 404}"#,
            expected_status: 404,
            expected_body: r#"{"branch":"client"}"#,
            expected_header: None,
        },
        Case {
            // The branch is selected by its PATTERN and not by the status it is keyed by:
            // `IntegrationResponse$statusCode` is "the status code that is used to map the
            // integration response to an existing MethodResponse", so a `5\d{2}` branch
            // keyed `500` covers a `503` result and maps it onto its own `500`.
            name: "a result a patterned branch covers maps onto that branch's own status",
            request_template: r#"{"statusCode": 503}"#,
            expected_status: 500,
            expected_body: r#"{"branch":"server"}"#,
            expected_header: Some(("x-branch", "server")),
        },
        Case {
            name: "a result no pattern covers maps to the unpatterned one",
            request_template: r#"{"statusCode": 302}"#,
            expected_status: 200,
            expected_body: r#"{"route":"internal"}"#,
            expected_header: None,
        },
        Case {
            name: "a rendering that names no result maps to the unpatterned one",
            request_template: r#"{"ok":true}"#,
            expected_status: 200,
            expected_body: r#"{"route":"internal"}"#,
            expected_header: None,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, "selection-pattern").await;
        let resource = create_resource(&gateway, &api, &root, &format!("branch{index}")).await;
        let resource = string_member(&resource, "id", case.name);
        put_method(
            &gateway,
            &api,
            &resource,
            "GET",
            json!({ "authorizationType": "NONE" }),
        )
        .await;
        put_integration(
            &gateway,
            &api,
            &resource,
            "GET",
            json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": case.request_template },
            }),
        )
        .await;
        // The three branches: the unpatterned one, and one per status class, each with the
        // method response that declares what it returns.
        for (status, pattern, body, declared) in [
            ("200", None, r#"{"route":"internal"}"#, json!({})),
            (
                "404",
                Some(r"4\d{2}"),
                r#"{"branch":"client"}"#,
                json!({}),
            ),
            (
                "500",
                Some(r"5\d{2}"),
                r#"{"branch":"server"}"#,
                json!({ "method.response.header.X-Branch": true }),
            ),
        ] {
            method_response(
                &gateway,
                &api,
                &resource,
                "GET",
                status,
                json!({ "responseParameters": declared }),
            )
            .await;
            let mut request = json!({ "responseTemplates": json_template(body) });
            if let Some(pattern) = pattern {
                request["selectionPattern"] = json!(pattern);
            }
            if status == "500" {
                request["responseParameters"] =
                    json!({ "method.response.header.X-Branch": "'server'" });
            }
            integration_response(&gateway, &api, &resource, "GET", status, request).await;
        }
        deploy(&gateway, &api, "prod").await;

        let answered = invoke(
            &gateway,
            &api,
            "GET",
            &format!("/prod/branch{index}"),
            &[],
        )
        .await;
        assert_eq!(
            answered.status, case.expected_status,
            "case {:?}: expected {} on the wire, got {} with {:?}",
            case.name, case.expected_status, answered.status, answered.body
        );
        assert_eq!(
            answered.body, case.expected_body,
            "case {:?}: expected the selected branch's body {:?}, got {:?}",
            case.name, case.expected_body, answered.body
        );
        if let Some((name, value)) = case.expected_header {
            assert_eq!(
                answered.header(name),
                Some(value),
                "case {:?}: expected the selected branch's header {name}: {value}, got {:?}",
                case.name,
                answered.header(name)
            );
        }
    }
}

#[tokio::test]
async fn every_request_to_a_deployed_stage_is_answered_from_the_apis_own_records() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "invoke-routing").await;

    // The api's resource tree spans what the routing rule generalises over: the root
    // resource, a literal child, a path parameter, a greedy parameter, and a method put on
    // the api AFTER the deployment is created.
    mock_method(&gateway, &api, &root, "GET", "200", json_template(r#"{"at":"root"}"#)).await;
    let pets = mock_resource(
        &gateway,
        &api,
        &root,
        "pets",
        "GET",
        "200",
        json_template(r#"{"at":"pets"}"#),
    )
    .await;
    mock_resource(
        &gateway,
        &api,
        &pets,
        "{petId}",
        "GET",
        "200",
        json_template(r#"{"at":"one-pet"}"#),
    )
    .await;
    let files = create_resource(&gateway, &api, &root, "files").await;
    let files = string_member(&files, "id", "files");
    mock_resource(
        &gateway,
        &api,
        &files,
        "{proxy+}",
        "ANY",
        "200",
        json_template(r#"{"at":"proxy"}"#),
    )
    .await;
    let late = create_resource(&gateway, &api, &root, "late").await;
    let late = string_member(&late, "id", "late");

    deploy(&gateway, &api, "prod").await;
    // Put AFTER the deployment, so it is not in the stage's snapshot.
    mock_method(&gateway, &api, &late, "GET", "200", json_template(r#"{"at":"late"}"#)).await;

    // Rows differ along the verb, the path shape, the stage and the segment count.
    let cases: &[(&str, &str, &str, u16, &str)] = &[
        (
            "the root resource of the stage",
            "GET",
            "/prod/",
            200,
            r#"{"at":"root"}"#,
        ),
        (
            "the stage with no trailing slash is still the root resource",
            "GET",
            "/prod",
            200,
            r#"{"at":"root"}"#,
        ),
        ("a literal path", "GET", "/prod/pets", 200, r#"{"at":"pets"}"#),
        (
            "a path parameter takes one segment",
            "GET",
            "/prod/pets/42",
            200,
            r#"{"at":"one-pet"}"#,
        ),
        (
            "a greedy parameter takes the rest of the path",
            "GET",
            "/prod/files/a/b/c.txt",
            200,
            r#"{"at":"proxy"}"#,
        ),
        (
            "the ANY catch-all serves a verb of its own",
            "DELETE",
            "/prod/files/a",
            200,
            r#"{"at":"proxy"}"#,
        ),
        (
            "a path the deployment holds no resource for",
            "GET",
            "/prod/nope",
            403,
            "Missing Authentication Token",
        ),
        (
            "one segment past a path parameter",
            "GET",
            "/prod/pets/42/toys",
            403,
            "Missing Authentication Token",
        ),
        (
            "a verb the resource has no method for",
            "POST",
            "/prod/pets",
            403,
            "Missing Authentication Token",
        ),
        (
            "a method put on the api after this stage was deployed",
            "GET",
            "/prod/late",
            403,
            "Missing Authentication Token",
        ),
        (
            "a stage the api does not have",
            "GET",
            "/beta/pets",
            403,
            "Forbidden",
        ),
        ("no stage at all", "GET", "/", 403, "Forbidden"),
    ];

    for (case, verb, path, status, expected) in cases {
        let answered = invoke(&gateway, &api, verb, path, &[]).await;
        assert_eq!(
            answered.status, *status,
            "case {case:?} ({verb} {path}): expected {status}, got {} with {:?}",
            answered.status, answered.body
        );
        if *status == 200 {
            assert_eq!(
                answered.body, *expected,
                "case {case:?} ({verb} {path}): expected the body {expected:?}, got {:?}",
                answered.body
            );
        } else {
            assert_eq!(
                answered.message().as_deref(),
                Some(*expected),
                "case {case:?} ({verb} {path}): expected the message {expected:?}, got {:?}",
                answered.body
            );
        }
    }

    // The snapshot rule in the other direction: redeploying the api takes the methods as
    // they stand now, so the late method is served and nothing else changed.
    deploy(&gateway, &api, "prod").await;
    let answered = invoke(&gateway, &api, "GET", "/prod/late", &[]).await;
    assert_eq!(
        answered.status, 200,
        "case redeployed late method: expected 200, got {} with {:?}",
        answered.status, answered.body
    );
    assert_eq!(
        answered.body, r#"{"at":"late"}"#,
        "case redeployed late method: expected the late body, got {:?}",
        answered.body
    );
}

#[tokio::test]
async fn a_mock_integrations_response_is_built_from_the_selected_integration_response() {
    // Rows differ along what the rule generalises over: the integration response's status
    // code (a 2xx, a 4xx and a 5xx), the response templates it carries (one, several,
    // none), the content type the caller accepts, and whether a static header mapping is
    // declared by the method response.
    struct Case {
        name: &'static str,
        /// The `statusCode` of the integration response and of the method response.
        status: &'static str,
        templates: Value,
        /// The `responseParameters` of the integration response.
        mapped: Value,
        /// The `responseParameters` of the method response: only a header DECLARED here is
        /// returned, which is the rule the `responseParameters` members document.
        declared: Value,
        accept: Option<&'static str>,
        expected_status: u16,
        expected_body: &'static str,
        expected_content_type: &'static str,
        expected_headers: &'static [(&'static str, &'static str)],
        absent_headers: &'static [&'static str],
    }

    let cases = vec![
        Case {
            name: "a literal JSON template on a 200",
            status: "200",
            templates: json_template(r#"{"ok":true}"#),
            mapped: json!({}),
            declared: json!({}),
            accept: None,
            expected_status: 200,
            expected_body: r#"{"ok":true}"#,
            expected_content_type: "application/json",
            expected_headers: &[],
            absent_headers: &[],
        },
        Case {
            name: "a 404 the integration response maps to",
            status: "404",
            templates: json_template(r#"{"missing":true}"#),
            mapped: json!({}),
            declared: json!({}),
            accept: None,
            expected_status: 404,
            expected_body: r#"{"missing":true}"#,
            expected_content_type: "application/json",
            expected_headers: &[],
            absent_headers: &[],
        },
        Case {
            name: "a 503 the integration response maps to",
            status: "503",
            templates: json_template(r#"{"down":true}"#),
            mapped: json!({}),
            declared: json!({}),
            accept: None,
            expected_status: 503,
            expected_body: r#"{"down":true}"#,
            expected_content_type: "application/json",
            expected_headers: &[],
            absent_headers: &[],
        },
        Case {
            // The template the caller's `Accept` chooses decides the BODY and nothing
            // else: with no `Content-Type` mapping declared, the media type the response is
            // served with stays `application/json`.
            name: "the template of the accepted content type is chosen",
            status: "200",
            templates: json!({
                "application/json": r#"{"form":"json"}"#,
                "text/plain": "form=plain",
            }),
            mapped: json!({}),
            declared: json!({}),
            accept: Some("text/plain"),
            expected_status: 200,
            expected_body: "form=plain",
            expected_content_type: "application/json",
            expected_headers: &[],
            absent_headers: &[],
        },
        Case {
            name: "an Accept with parameters still selects its media type",
            status: "200",
            templates: json!({
                "application/json": r#"{"form":"json"}"#,
                "text/plain": "form=plain",
            }),
            accept: Some("text/plain; charset=utf-8"),
            mapped: json!({}),
            declared: json!({}),
            expected_status: 200,
            expected_body: "form=plain",
            expected_content_type: "application/json",
            expected_headers: &[],
            absent_headers: &[],
        },
        Case {
            name: "an Accept no template carries falls back to the JSON template",
            status: "200",
            templates: json!({
                "application/json": r#"{"form":"json"}"#,
                "text/plain": "form=plain",
            }),
            accept: Some("application/xml"),
            mapped: json!({}),
            declared: json!({}),
            expected_status: 200,
            expected_body: r#"{"form":"json"}"#,
            expected_content_type: "application/json",
            expected_headers: &[],
            absent_headers: &[],
        },
        Case {
            name: "no response template at all answers an empty body",
            status: "201",
            templates: json!({}),
            mapped: json!({}),
            declared: json!({}),
            accept: None,
            expected_status: 201,
            expected_body: "",
            expected_content_type: "application/json",
            expected_headers: &[],
            absent_headers: &[],
        },
        Case {
            name: "a literal body carrying characters a template language uses",
            status: "200",
            templates: json_template(r##"{"color":"#fff","price":"$5"}"##),
            mapped: json!({}),
            declared: json!({}),
            accept: None,
            expected_status: 200,
            expected_body: r##"{"color":"#fff","price":"$5"}"##,
            expected_content_type: "application/json",
            expected_headers: &[],
            absent_headers: &[],
        },
        Case {
            // Nothing restricts a stored template to ASCII, and serving one must not fail.
            name: "a literal body carrying non-ASCII text",
            status: "200",
            templates: json_template(r#"{"msg":"café ☃ 🙂"}"#),
            mapped: json!({}),
            declared: json!({}),
            accept: None,
            expected_status: 200,
            expected_body: r#"{"msg":"café ☃ 🙂"}"#,
            expected_content_type: "application/json",
            expected_headers: &[],
            absent_headers: &[],
        },
        Case {
            // Two templates and nothing to choose between them: the call is served with no
            // body rather than with a template the caller did not ask for.
            name: "an Accept no template carries, with no JSON template to fall back to",
            status: "200",
            templates: json!({
                "text/plain": "form=plain",
                "text/csv": "form,csv",
            }),
            accept: Some("application/xml"),
            mapped: json!({}),
            declared: json!({}),
            expected_status: 200,
            expected_body: "",
            expected_content_type: "application/json",
            expected_headers: &[],
            absent_headers: &[],
        },
        Case {
            // A stored mapping the wire cannot carry must not fail the request: the header
            // is skipped and the rest of the response is still served.
            name: "a declared static header whose value cannot be sent is skipped",
            status: "200",
            templates: json_template(r#"{"served":true}"#),
            mapped: json!({ "method.response.header.X-Bad": "'one\ntwo'" }),
            declared: json!({ "method.response.header.X-Bad": true }),
            accept: None,
            expected_status: 200,
            expected_body: r#"{"served":true}"#,
            expected_content_type: "application/json",
            expected_headers: &[],
            absent_headers: &["x-bad"],
        },
        Case {
            // The one mapping that DOES change the media type: a `Content-Type` the method
            // response declares and the integration response maps.
            name: "a declared Content-Type mapping is the media type the call is served with",
            status: "200",
            templates: json!({
                "application/json": r#"{"form":"json"}"#,
                "application/xml": "<form>xml</form>",
            }),
            mapped: json!({ "method.response.header.Content-Type": "'application/xml'" }),
            declared: json!({ "method.response.header.Content-Type": true }),
            accept: Some("application/xml"),
            expected_status: 200,
            expected_body: "<form>xml</form>",
            expected_content_type: "application/xml",
            expected_headers: &[],
            absent_headers: &[],
        },
        Case {
            name: "a declared static header mapping is returned, an undeclared one is not",
            status: "200",
            templates: json_template(r#"{"cors":true}"#),
            mapped: json!({
                "method.response.header.Access-Control-Allow-Origin": "'*'",
                "method.response.header.X-Undeclared": "'dropped'",
            }),
            declared: json!({ "method.response.header.Access-Control-Allow-Origin": true }),
            accept: None,
            expected_status: 200,
            expected_body: r#"{"cors":true}"#,
            expected_content_type: "application/json",
            expected_headers: &[("access-control-allow-origin", "*")],
            absent_headers: &["x-undeclared"],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, "invoke-mock").await;
        let resource = create_resource(&gateway, &api, &root, &format!("case{index}")).await;
        let resource = string_member(&resource, "id", case.name);
        put_method(
            &gateway,
            &api,
            &resource,
            "GET",
            json!({ "authorizationType": "NONE" }),
        )
        .await;
        put_integration(&gateway, &api, &resource, "GET", json!({ "type": "MOCK" })).await;
        method_response(
            &gateway,
            &api,
            &resource,
            "GET",
            case.status,
            json!({ "responseParameters": case.declared }),
        )
        .await;
        integration_response(
            &gateway,
            &api,
            &resource,
            "GET",
            case.status,
            json!({
                "responseTemplates": case.templates,
                "responseParameters": case.mapped,
            }),
        )
        .await;
        deploy(&gateway, &api, "prod").await;

        let headers: Vec<(&str, &str)> = case
            .accept
            .map(|accept| vec![("accept", accept)])
            .unwrap_or_default();
        let answered = invoke(
            &gateway,
            &api,
            "GET",
            &format!("/prod/case{index}"),
            &headers,
        )
        .await;
        assert_eq!(
            answered.status, case.expected_status,
            "case {:?}: expected {}, got {} with {:?}",
            case.name, case.expected_status, answered.status, answered.body
        );
        assert_eq!(
            answered.body, case.expected_body,
            "case {:?}: expected the body {:?}, got {:?}",
            case.name, case.expected_body, answered.body
        );
        assert_eq!(
            answered.header("content-type"),
            Some(case.expected_content_type),
            "case {:?}: expected content-type {:?}, got {:?}",
            case.name,
            case.expected_content_type,
            answered.header("content-type")
        );
        for (name, value) in case.expected_headers {
            assert_eq!(
                answered.header(name),
                Some(*value),
                "case {:?}: expected the header {name}: {value}, got {:?}",
                case.name,
                answered.header(name)
            );
        }
        for name in case.absent_headers {
            assert_eq!(
                answered.header(name),
                None,
                "case {:?}: expected no {name} header, got {:?}",
                case.name,
                answered.header(name)
            );
        }
    }
}

#[tokio::test]
async fn the_deployed_methods_authorization_decides_which_requests_are_served() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "invoke-authorization").await;

    // One resource per authorization type the `Method$authorizationType` documentation
    // lists, each with the same MOCK integration behind it, so only the authorization
    // varies between the rows.
    let open = mock_resource(
        &gateway,
        &api,
        &root,
        "open",
        "GET",
        "200",
        json_template(r#"{"open":true}"#),
    )
    .await;
    // Named for the row that invokes `/prod/open`; the id itself is not needed again.
    drop(open);

    let iam = create_resource(&gateway, &api, &root, "iam").await;
    let iam = string_member(&iam, "id", "iam");
    put_method(
        &gateway,
        &api,
        &iam,
        "GET",
        json!({ "authorizationType": "AWS_IAM" }),
    )
    .await;
    put_integration(&gateway, &api, &iam, "GET", json!({ "type": "MOCK" })).await;
    method_response(&gateway, &api, &iam, "GET", "200", json!({})).await;
    integration_response(
        &gateway,
        &api,
        &iam,
        "GET",
        "200",
        json!({ "responseTemplates": json_template(r#"{"iam":true}"#) }),
    )
    .await;

    let authorizer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/authorizers"),
            Some(json!({
                "name": "token-authorizer",
                "type": "TOKEN",
                "identitySource": "method.request.header.Auth",
                "authorizerUri": format!(
                    "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                     arn:aws:lambda:{REGION}:123456789012:function:authorize/invocations"
                ),
            })),
        )
        .await;
    assert_eq!(
        authorizer.status, 201,
        "case CreateAuthorizer: expected 201, got {} with {:?}",
        authorizer.status, authorizer.body
    );
    let authorizer = string_member(&authorizer.body, "id", "CreateAuthorizer");
    let custom = create_resource(&gateway, &api, &root, "custom").await;
    let custom = string_member(&custom, "id", "custom");
    put_method(
        &gateway,
        &api,
        &custom,
        "GET",
        json!({ "authorizationType": "CUSTOM", "authorizerId": authorizer }),
    )
    .await;
    put_integration(&gateway, &api, &custom, "GET", json!({ "type": "MOCK" })).await;

    // A method whose authorization type no authorizer of this api can serve: the stage
    // cannot authorize the call, which is the configuration-error class. The type is one
    // no `Method$authorizationType` value names — the way such a method is now reachable
    // at all, since a `CUSTOM` method naming no authorizer is refused when it is put
    // (`PutMethod` requires an authorizer the api holds).
    let unauthorized = create_resource(&gateway, &api, &root, "no-authorizer").await;
    let unauthorized = string_member(&unauthorized, "id", "no-authorizer");
    put_method(
        &gateway,
        &api,
        &unauthorized,
        "GET",
        json!({ "authorizationType": "SOMETHING_ELSE" }),
    )
    .await;
    put_integration(&gateway, &api, &unauthorized, "GET", json!({ "type": "MOCK" })).await;

    let keyed = create_resource(&gateway, &api, &root, "keyed").await;
    let keyed = string_member(&keyed, "id", "keyed");
    put_method(
        &gateway,
        &api,
        &keyed,
        "GET",
        json!({ "authorizationType": "NONE", "apiKeyRequired": true }),
    )
    .await;
    put_integration(&gateway, &api, &keyed, "GET", json!({ "type": "MOCK" })).await;
    method_response(&gateway, &api, &keyed, "GET", "200", json!({})).await;
    integration_response(
        &gateway,
        &api,
        &keyed,
        "GET",
        "200",
        json!({ "responseTemplates": json_template(r#"{"keyed":true}"#) }),
    )
    .await;

    deploy(&gateway, &api, "prod").await;

    // The key is created after the stage exists, because a `stageKeys` entry must name a
    // stored stage. One key is associated with this stage and enabled; the others differ in
    // exactly one of those two properties, which is what the rule generalises over.
    let associated = "0123456789abcdef0123456789abcdef01234567";
    let disabled = "fedcba9876543210fedcba9876543210fedcba98";
    let unassociated = "aaaabbbbccccddddeeeeffff00001111222233334";
    for (value, enabled, stages) in [
        (associated, true, json!([{ "restApiId": api, "stageName": "prod" }])),
        (disabled, false, json!([{ "restApiId": api, "stageName": "prod" }])),
        (unassociated, true, json!([])),
    ] {
        let created = gateway
            .call(
                "POST",
                "/apikeys",
                Some(json!({ "value": value, "enabled": enabled, "stageKeys": stages })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case CreateApiKey({value}): expected 201, got {} with {:?}",
            created.status, created.body
        );
    }

    // Rows differ along the authorization type, the credential or key the request carries,
    // and the identity source the authorizer declares.
    struct AuthorizationCase<'c> {
        name: &'c str,
        path: &'c str,
        headers: &'c [(&'c str, &'c str)],
        expected_status: u16,
        /// The body for a served call, the gateway response's `message` for a refusal that
        /// carries one, and [`NO_AUTHORIZER_DECISION`] for the refusal class that carries
        /// none.
        expected: &'c str,
    }
    let cases: &[AuthorizationCase] = &[
        AuthorizationCase {
            name: "an open method is served to an anonymous caller",
            path: "/prod/open",
            headers: &[],
            expected_status: 200,
            expected: r#"{"open":true}"#,
        },
        AuthorizationCase {
            name: "an AWS_IAM method reached with no credential",
            path: "/prod/iam",
            headers: &[],
            expected_status: 403,
            expected: "Missing Authentication Token",
        },
        AuthorizationCase {
            name: "an AWS_IAM method reached with a SigV4 credential",
            path: "/prod/iam",
            headers: &[("authorization", SIGNED)],
            expected_status: 200,
            expected: r#"{"iam":true}"#,
        },
        AuthorizationCase {
            name: "an AWS_IAM method reached with a presigned credential",
            path: "/prod/iam",
            headers: &[],
            expected_status: 200,
            expected: r#"{"iam":true}"#,
        },
        AuthorizationCase {
            name: "a CUSTOM method whose identity source is absent is refused before the \
                   authorizer runs",
            path: "/prod/custom",
            headers: &[],
            expected_status: 401,
            expected: "Unauthorized",
        },
        AuthorizationCase {
            name: "a CUSTOM method whose identity source is present needs the authorizer's \
                   decision",
            path: "/prod/custom",
            headers: &[("auth", "a-token")],
            expected_status: 500,
            expected: NO_AUTHORIZER_DECISION,
        },
        AuthorizationCase {
            name: "a method whose authorization type no authorizer can serve",
            path: "/prod/no-authorizer",
            headers: &[("auth", "a-token")],
            expected_status: 500,
            expected: "Internal server error",
        },
        AuthorizationCase {
            name: "a method requiring an API key reached with none",
            path: "/prod/keyed",
            headers: &[],
            expected_status: 403,
            expected: "Forbidden",
        },
        AuthorizationCase {
            name: "a method requiring an API key reached with an unknown value",
            path: "/prod/keyed",
            headers: &[("x-api-key", "not-a-key")],
            expected_status: 403,
            expected: "Forbidden",
        },
        AuthorizationCase {
            name: "a key associated with the stage but disabled",
            path: "/prod/keyed",
            headers: &[("x-api-key", disabled)],
            expected_status: 403,
            expected: "Forbidden",
        },
        AuthorizationCase {
            name: "an enabled key associated with no stage",
            path: "/prod/keyed",
            headers: &[("x-api-key", unassociated)],
            expected_status: 403,
            expected: "Forbidden",
        },
        AuthorizationCase {
            name: "an enabled key associated with this stage",
            path: "/prod/keyed",
            headers: &[("x-api-key", associated)],
            expected_status: 200,
            expected: r#"{"keyed":true}"#,
        },
    ];

    for case in cases {
        // A presigned invoke carries its credential scope in the query string instead of a
        // header, and must be admitted by the same rule a header-signed one is.
        let path = if case.name.contains("presigned") {
            format!(
                "{}?X-Amz-Credential=AKIDEXAMPLE%2F20260101%2Fus-east-1%2Fapigateway%2F\
                 aws4_request",
                case.path
            )
        } else {
            case.path.to_string()
        };
        let answered = invoke(&gateway, &api, "GET", &path, case.headers).await;
        assert_eq!(
            answered.status, case.expected_status,
            "case {:?} (GET {path}): expected {}, got {} with {:?}",
            case.name, case.expected_status, answered.status, answered.body
        );
        if case.expected_status == 200 {
            assert_eq!(
                answered.body, case.expected,
                "case {:?} (GET {path}): expected the body {:?}, got {:?}",
                case.name, case.expected, answered.body
            );
        } else if case.expected == NO_AUTHORIZER_DECISION {
            // A refusal the gateway sets no error message for carries the JSON literal `null`
            // under the member, so there is no sentence to read off it: the whole body is
            // what the rule owes ([`crate::stage_authorizer_failure_body`]).
            assert_eq!(
                answered.body, NO_AUTHORIZER_DECISION,
                "case {:?} (GET {path}): expected the body {NO_AUTHORIZER_DECISION:?}, got {:?}",
                case.name, answered.body
            );
        } else {
            assert_eq!(
                answered.message().as_deref(),
                Some(case.expected),
                "case {:?} (GET {path}): expected the message {:?}, got {:?}",
                case.name, case.expected, answered.body
            );
        }
    }
}

#[tokio::test]
async fn an_integration_this_emulator_cannot_perform_is_answered_as_one_that_failed() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "invoke-integration-failure").await;

    // Rows differ along the integration configuration that cannot be served: each type
    // that names a backend (Lambda, another AWS action, an HTTP endpoint, a private
    // endpoint), a MOCK with no default integration response, and a MOCK whose response
    // template is a Velocity template rather than a literal body. A method with NO
    // integration is not among them: such an api cannot be deployed at all, so the data
    // plane is never reached with one (`crate::deployment_preconditions`).
    let lambda_uri = format!(
        "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
         arn:aws:lambda:{REGION}:123456789012:function:handler/invocations"
    );
    let cases: Vec<(&str, &str, Option<Value>, Option<Value>)> = vec![
        (
            "a Lambda proxy integration",
            "proxy",
            Some(json!({
                "type": "AWS_PROXY",
                // The integration's own verb: the input member is `integrationHttpMethod`,
                // whose `jsonName` is `httpMethod`, so that is the name a request carries.
                "httpMethod": "POST",
                "uri": lambda_uri,
            })),
            None,
        ),
        (
            "an AWS service action integration",
            "ddb",
            Some(json!({
                "type": "AWS",
                "httpMethod": "POST",
                "uri": format!("arn:aws:apigateway:{REGION}:dynamodb:action/PutItem"),
            })),
            None,
        ),
        (
            "an HTTP proxy integration",
            "http",
            Some(json!({
                "type": "HTTP_PROXY",
                "httpMethod": "GET",
                "uri": "https://example.com/upstream",
            })),
            None,
        ),
        (
            "an HTTP integration",
            "upstream",
            Some(json!({
                "type": "HTTP",
                "httpMethod": "GET",
                "uri": "https://example.com/upstream",
            })),
            None,
        ),
        (
            "a MOCK whose only integration response carries a selection pattern",
            "patterned",
            Some(json!({ "type": "MOCK" })),
            Some(json!({ "selectionPattern": "4\\d\\d" })),
        ),
        (
            "a MOCK whose response template is a Velocity template",
            "velocity",
            Some(json!({ "type": "MOCK" })),
            Some(json!({
                "responseTemplates": {
                    "application/json": "#set($root = $input.path('$'))\n{\"id\":\"$root.id\"}",
                },
            })),
        ),
    ];

    for (case, path_part, integration, integration_response_body) in &cases {
        let resource = create_resource(&gateway, &api, &root, path_part).await;
        let resource = string_member(&resource, "id", case);
        put_method(
            &gateway,
            &api,
            &resource,
            "GET",
            json!({ "authorizationType": "NONE" }),
        )
        .await;
        if let Some(integration) = integration {
            put_integration(&gateway, &api, &resource, "GET", integration.clone()).await;
        }
        if let Some(body) = integration_response_body {
            method_response(&gateway, &api, &resource, "GET", "200", json!({})).await;
            integration_response(&gateway, &api, &resource, "GET", "200", body.clone()).await;
        }
    }
    deploy(&gateway, &api, "prod").await;

    for (case, path_part, _, _) in &cases {
        let answered = invoke(&gateway, &api, "GET", &format!("/prod/{path_part}"), &[]).await;
        assert_eq!(
            answered.status, 500,
            "case {case:?}: expected 500, got {} with {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.message().as_deref(),
            Some("Internal server error"),
            "case {case:?}: expected the API Gateway integration-failure message, got {:?}",
            answered.body
        );
    }
}

#[tokio::test]
async fn a_customized_gateway_response_is_what_the_refused_caller_sees() {
    // Rows differ along which customization applies: the refused type's own, the class
    // fallback for a type with no customization of its own, and none at all (the status
    // code the response type documents).
    struct Case {
        name: &'static str,
        /// The `responseType` `PutGatewayResponse` is called for, with its body.
        customization: Option<(&'static str, Value)>,
        /// The path invoked, which decides which response type the refusal takes.
        path: &'static str,
        expected_status: u16,
        expected_body: &'static str,
    }

    let cases = vec![
        Case {
            name: "no customization: the response type's own documented status",
            customization: None,
            path: "/prod/nope",
            expected_status: 403,
            expected_body: r#"{"message":"Missing Authentication Token"}"#,
        },
        Case {
            name: "the refused type's own customization",
            customization: Some((
                "MISSING_AUTHENTICATION_TOKEN",
                json!({
                    "statusCode": "404",
                    "responseTemplates": { "application/json": r#"{"message":"no route"}"# },
                }),
            )),
            path: "/prod/nope",
            expected_status: 404,
            expected_body: r#"{"message":"no route"}"#,
        },
        Case {
            name: "the 4xx class customization applies to a type with none of its own",
            customization: Some((
                "DEFAULT_4XX",
                json!({
                    "statusCode": "400",
                    "responseTemplates": { "application/json": r#"{"message":"client"}"# },
                }),
            )),
            path: "/prod/nope",
            expected_status: 400,
            expected_body: r#"{"message":"client"}"#,
        },
        Case {
            name: "the 5xx class customization applies to an integration that cannot run",
            customization: Some((
                "DEFAULT_5XX",
                json!({
                    "statusCode": "502",
                    "responseTemplates": { "application/json": r#"{"message":"backend"}"# },
                }),
            )),
            path: "/prod/bare",
            expected_status: 502,
            expected_body: r#"{"message":"backend"}"#,
        },
        Case {
            // Nothing restricts a stored template to ASCII, and every refusal of this api
            // renders through one, so serving one must not fail.
            name: "a customization whose template carries non-ASCII text",
            customization: Some((
                "MISSING_AUTHENTICATION_TOKEN",
                json!({
                    "statusCode": "403",
                    "responseTemplates": { "application/json": r#"{"message":"introuvable ☃"}"# },
                }),
            )),
            path: "/prod/nope",
            expected_status: 403,
            expected_body: r#"{"message":"introuvable ☃"}"#,
        },
    ];

    for case in &cases {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, "invoke-gateway-responses").await;
        // One method whose integration cannot be served, so the 5xx class is reachable
        // too: a MOCK whose only integration response carries a selection pattern, so
        // nothing maps the call the mock returns. The method DOES carry an integration,
        // because an api holding a method with none cannot be deployed at all.
        let bare = create_resource(&gateway, &api, &root, "bare").await;
        let bare = string_member(&bare, "id", case.name);
        put_method(
            &gateway,
            &api,
            &bare,
            "GET",
            json!({ "authorizationType": "NONE" }),
        )
        .await;
        put_integration(&gateway, &api, &bare, "GET", json!({ "type": "MOCK" })).await;
        method_response(&gateway, &api, &bare, "GET", "200", json!({})).await;
        integration_response(
            &gateway,
            &api,
            &bare,
            "GET",
            "200",
            json!({ "selectionPattern": "4\\d\\d" }),
        )
        .await;
        if let Some((response_type, body)) = &case.customization {
            let put = gateway
                .call(
                    "PUT",
                    &format!("/restapis/{api}/gatewayresponses/{response_type}"),
                    Some(body.clone()),
                )
                .await;
            assert_eq!(
                put.status, 201,
                "case {:?}: expected 201 from PutGatewayResponse, got {} with {:?}",
                case.name, put.status, put.body
            );
        }
        deploy(&gateway, &api, "prod").await;

        let answered = invoke(&gateway, &api, "GET", case.path, &[]).await;
        assert_eq!(
            answered.status, case.expected_status,
            "case {:?}: expected {}, got {} with {:?}",
            case.name, case.expected_status, answered.status, answered.body
        );
        assert_eq!(
            answered.body, case.expected_body,
            "case {:?}: expected the body {:?}, got {:?}",
            case.name, case.expected_body, answered.body
        );
    }
}

#[tokio::test]
async fn only_the_invoke_endpoint_of_an_api_this_service_holds_is_served() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "invoke-claim").await;
    mock_resource(
        &gateway,
        &api,
        &root,
        "pets",
        "GET",
        "200",
        json_template(r#"{"at":"pets"}"#),
    )
    .await;
    deploy(&gateway, &api, "prod").await;

    // Rows differ along the host and the path: the dimensions the claim rule generalises
    // over. A host this service does not hold an api for is NOT claimed — the same host
    // shape is the invoke endpoint of an api of the V2 service, and `@connections` on this
    // very host is ApiGatewayManagementApi's modelled path space — so the front door
    // answers its own `404 UnknownService` rather than this service answering for another.
    let other_api = "zzzzzzzzzz";
    let cases: &[(&str, String, &str, u16)] = &[
        (
            "this api's own invoke endpoint",
            invoke_host(&api),
            "/prod/pets",
            200,
        ),
        (
            "an execute-api host for an api id this service does not hold",
            invoke_host(other_api),
            "/prod/pets",
            404,
        ),
        (
            "the @connections path space on this api's host",
            invoke_host(&api),
            "/prod/@connections/abc123",
            404,
        ),
        (
            "the @connections path space without a stage prefix",
            invoke_host(&api),
            "/@connections/abc123",
            404,
        ),
        (
            "the control plane's own host carrying an invoke path",
            format!("apigateway.{REGION}.amazonaws.com"),
            "/prod/pets",
            404,
        ),
    ];

    for (case, host, path, status) in cases {
        let answered = invoke_on_host(&gateway, host, "GET", path, &[]).await;
        assert_eq!(
            answered.status, *status,
            "case {case:?} ({host}{path}): expected {status}, got {} with {:?}",
            answered.status, answered.body
        );
        if *status == 404 {
            assert!(
                answered.body.contains("UnknownService"),
                "case {case:?} ({host}{path}): expected the front door's own refusal, got {:?}",
                answered.body
            );
        }
    }
}

#[tokio::test]
async fn a_stage_serves_the_deployed_configuration_of_a_method_until_the_api_is_redeployed() {
    // A deployment is "a snapshot of your REST API configuration" and a stage serves the
    // deployment its `deploymentId` names, so an edit to a method takes effect at the stage
    // only once the api is deployed again. The rows differ along WHICH part of the method's
    // configuration the edit changes — the body the integration response renders, the
    // integration request that selects which response answers (and so the status) —
    // because the rule is about the whole configuration rather than about one member of
    // it. Deleting the integration outright is not among the rows: the api could then not
    // be deployed again at all, which is the sibling
    // `crate::deployment_preconditions` suite's rule.
    struct Row {
        case: &'static str,
        /// The control-plane call that edits the method after the deployment.
        verb: &'static str,
        /// The path within the method, under
        /// `/restapis/{api}/resources/{resource}/methods/GET`.
        path: &'static str,
        body: Option<Value>,
        /// The status that edit is accepted with.
        accepted: u16,
        /// What the stage answers once the api is deployed AGAIN.
        redeployed: (u16, &'static str),
    }
    /// What every row's method answers while the deployment that was taken before the edit
    /// is the one the stage serves.
    const DEPLOYED: (u16, &str) = (200, r#"{"v":"one"}"#);
    let rows: &[Row] = &[
        Row {
            case: "the integration response's body template",
            verb: "PATCH",
            path: "/integration/responses/200",
            body: Some(json!({
                "patchOperations": [{
                    "op": "replace",
                    "path": "/responseTemplates/application~1json",
                    "value": r#"{"v":"two"}"#,
                }],
            })),
            accepted: 200,
            redeployed: (200, r#"{"v":"two"}"#),
        },
        Row {
            case: "the integration request that selects which response answers",
            verb: "PATCH",
            path: "/integration",
            body: Some(json!({
                "patchOperations": [{
                    "op": "replace",
                    "path": "/requestTemplates/application~1json",
                    "value": r#"{"statusCode": 201}"#,
                }],
            })),
            accepted: 200,
            redeployed: (201, r#"{"v":"created"}"#),
        },
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "deployed-configuration").await;
    // One resource per row, each carrying the SAME method configuration, so the rows differ
    // only in the edit they make to it.
    let mut resources: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let resource = create_resource(&gateway, &api, &root, &format!("r{index}")).await;
        let resource = string_member(&resource, "id", row.case);
        put_method(
            &gateway,
            &api,
            &resource,
            "GET",
            json!({ "authorizationType": "NONE" }),
        )
        .await;
        put_integration(
            &gateway,
            &api,
            &resource,
            "GET",
            json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": r#"{"statusCode": 200}"# },
            }),
        )
        .await;
        for (status, body) in [("200", r#"{"v":"one"}"#), ("201", r#"{"v":"created"}"#)] {
            method_response(&gateway, &api, &resource, "GET", status, json!({})).await;
            integration_response(
                &gateway,
                &api,
                &resource,
                "GET",
                status,
                json!({ "responseTemplates": json_template(body) }),
            )
            .await;
        }
        resources.push(resource);
    }
    deploy(&gateway, &api, "prod").await;

    // What the deployment holds, before any edit.
    for (index, row) in rows.iter().enumerate() {
        let answered = invoke(&gateway, &api, "GET", &format!("/prod/r{index}"), &[]).await;
        assert_eq!(
            (answered.status, answered.body.as_str()),
            DEPLOYED,
            "case {:?}: expected the deployed answer {DEPLOYED:?} before the edit, got {} with \
             {:?}",
            row.case,
            answered.status,
            answered.body
        );
    }

    // The edits, each accepted by the control plane — and each leaving what the stage serves
    // exactly as it was, because the stage serves the deployment it points at.
    for (index, row) in rows.iter().enumerate() {
        let edited = gateway
            .call(
                row.verb,
                &format!(
                    "/restapis/{api}/resources/{resource}/methods/GET{path}",
                    resource = resources[index],
                    path = row.path
                ),
                row.body.clone(),
            )
            .await;
        assert_eq!(
            edited.status, row.accepted,
            "case {:?}: expected the edit to be accepted with {}, got {} with {:?}",
            row.case, row.accepted, edited.status, edited.body
        );
        let answered = invoke(&gateway, &api, "GET", &format!("/prod/r{index}"), &[]).await;
        assert_eq!(
            (answered.status, answered.body.as_str()),
            DEPLOYED,
            "case {:?}: an undeployed edit must not change what the stage serves, got {} with \
             {:?}",
            row.case,
            answered.status,
            answered.body
        );
    }

    // The rule in the other direction: deploying again takes the configuration as it now
    // stands, so every row's edit is what the stage serves from then on.
    deploy(&gateway, &api, "prod").await;
    for (index, row) in rows.iter().enumerate() {
        let answered = invoke(&gateway, &api, "GET", &format!("/prod/r{index}"), &[]).await;
        assert_eq!(
            (answered.status, answered.body.as_str()),
            row.redeployed,
            "case {:?}: expected {:?} once the api is redeployed, got {} with {:?}",
            row.case,
            row.redeployed,
            answered.status,
            answered.body
        );
    }
}

// `PutIntegrationResponse`'s own round trip and its refusals are the sibling
// `integration_response` suite's: this file asserts only what the stage SERVES from the
// records that suite writes.
