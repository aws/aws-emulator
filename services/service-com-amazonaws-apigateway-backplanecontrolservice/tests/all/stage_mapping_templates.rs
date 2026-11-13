// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MAPPING TEMPLATES a deployed stage applies: an integration's
//! `requestTemplates` and an integration response's `responseTemplates`.
//!
//! Every case drives the real wire path — an unsigned request to the api's own invoke host
//! through the multi-service router, with the control plane driven through the same
//! emulator ([`Gateway`]) — so what is asserted is what a client of a published invoke URL
//! sees.
//!
//! The RULE under test, over a SET of inputs rather than one recorded call: a mapping
//! template is applied by RENDERING it against the call, and the text it produced is what
//! the integration is called with and what the caller is answered — the integration request
//! template's rendering naming, for a `MOCK`, the status code that selects the integration
//! response, and the response template's rendering being the body. A reference of a
//! template names one of the variables API Gateway documents a mapping template against
//! (the method request, the stage's variables, the call's context), resolved from THIS
//! call; a template naming anything else, or carrying a directive that would have to be
//! executed, is an integration the call cannot be performed through and is answered with
//! the api's `API_CONFIGURATION_ERROR` gateway response.
//!
//! The rows differ along the variable a template names (payload, query string, header,
//! path parameter, stage variable, context member), along the form of the reference (bare,
//! braced, quiet, single- and double-quoted arguments), along what the request carries for
//! it (present, absent, several values), along where the template sits (the integration
//! request and the integration response) and along the forms that are NOT rendered.

use serde_json::{json, Value};

use crate::execute_api_data_plane::{
    integration_response, method_response, put_integration, put_method,
};
use crate::rest_api_lifecycle::{create_api, create_resource, resource_under, string_member, Gateway};
use crate::stage_request_validation::{invoke, JSON, STAGE};

/// The stage variables every case's stage carries, so a template naming one resolves.
fn stage_variables() -> Value {
    json!({ "backend": "beta", "tenantHeader": "x-tenant" })
}

/// The payload every case POSTs, so a template reading the method request's body has one.
const PAYLOAD: &str = r#"{"id":"a1","qty":2,"nested":{"deep":"value"}}"#;

/// `CreateDeployment` with no stage of its own, as its deployment id.
async fn deployment_of(gateway: &Gateway, api: &str) -> String {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "description": "the snapshot the stage serves" })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateDeployment({api}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", "CreateDeployment")
}

/// `CreateStage` for [`STAGE`] over `deployment`, carrying [`stage_variables`].
async fn create_stage(gateway: &Gateway, api: &str, deployment: &str) {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/stages"),
            Some(json!({
                "stageName": STAGE,
                "deploymentId": deployment,
                "variables": stage_variables(),
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateStage({STAGE}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// A `POST` method on `resource` with a `MOCK` integration carrying `request_templates` and
/// an integration response per `(status, response template)` entry of `responses`.
async fn mock_route(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    request_templates: Value,
    responses: &[(&str, &str)],
) {
    put_method(
        gateway,
        api,
        resource,
        "POST",
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    put_integration(
        gateway,
        api,
        resource,
        "POST",
        json!({ "type": "MOCK", "requestTemplates": request_templates }),
    )
    .await;
    for (status, template) in responses {
        method_response(gateway, api, resource, "POST", status, json!({})).await;
        integration_response(
            gateway,
            api,
            resource,
            "POST",
            status,
            json!({ "responseTemplates": { JSON: template } }),
        )
        .await;
    }
}

/// One rendered-template case: the response template under test, the query string the
/// request carries, and the body the stage must answer.
struct Rendered {
    case: &'static str,
    template: &'static str,
    query: &'static str,
    body: &'static str,
}

/// The rows every case below runs: one per variable form a template may name, each with the
/// text it renders to for the request the case sends. The api id and the stage are
/// per-run values, so the two rows naming them are asserted against the ids the case
/// created rather than against a literal.
fn rendered_rows() -> Vec<Rendered> {
    vec![
        Rendered {
            case: "a literal template is the body",
            template: r#"{"literal":true}"#,
            query: "",
            body: r#"{"literal":true}"#,
        },
        Rendered {
            case: "a query-string parameter the request carries",
            template: r#"{"tenant":"$input.params('tenant')"}"#,
            query: "?tenant=acme",
            body: r#"{"tenant":"acme"}"#,
        },
        Rendered {
            case: "a query-string parameter the request does not carry",
            template: r#"{"tenant":"$input.params('tenant')"}"#,
            query: "",
            body: r#"{"tenant":""}"#,
        },
        Rendered {
            case: "a query-string value a client percent-encoded",
            template: r#"{"tenant":"$input.params('tenant')"}"#,
            query: "?tenant=ac%20me",
            body: r#"{"tenant":"ac me"}"#,
        },
        Rendered {
            case: "a query-string value carrying a form-encoded space",
            template: r#"{"tenant":"$input.params('tenant')"}"#,
            query: "?tenant=ac+me",
            body: r#"{"tenant":"ac me"}"#,
        },
        Rendered {
            case: "the first value of a repeated query-string parameter",
            template: r#"{"tenant":"$input.params('tenant')"}"#,
            query: "?tenant=first&tenant=second",
            body: r#"{"tenant":"first"}"#,
        },
        Rendered {
            case: "a header, named case-insensitively as a header name compares",
            template: r#"{"trace":"$input.params('X-TRACE')"}"#,
            query: "",
            body: r#"{"trace":"t-1"}"#,
        },
        Rendered {
            case: "the braced spelling of a reference",
            template: r#"{"tenant":"${input.params('tenant')}"}"#,
            query: "?tenant=acme",
            body: r#"{"tenant":"acme"}"#,
        },
        Rendered {
            case: "the quiet spelling of a reference, with a double-quoted argument",
            template: r#"{"tenant":"$!input.params("tenant")"}"#,
            query: "?tenant=acme",
            body: r#"{"tenant":"acme"}"#,
        },
        Rendered {
            case: "a JSONPath over the integration's own answer",
            template: r#"{"mocked":$input.json('$.statusCode')}"#,
            query: "",
            body: r#"{"mocked":200}"#,
        },
        Rendered {
            case: "the integration's answer verbatim",
            template: "$input.body",
            query: "",
            body: r#"{"statusCode": 200}"#,
        },
        Rendered {
            case: "a stage variable the stage defines",
            template: r#"{"backend":"$stageVariables.backend"}"#,
            query: "",
            body: r#"{"backend":"beta"}"#,
        },
        Rendered {
            case: "a stage variable read through get()",
            template: r#"{"backend":"$stageVariables.get('backend')"}"#,
            query: "",
            body: r#"{"backend":"beta"}"#,
        },
        Rendered {
            case: "the context member naming the verb of the call",
            template: r#"{"verb":"$context.httpMethod"}"#,
            query: "",
            body: r#"{"verb":"POST"}"#,
        },
        Rendered {
            case: "two references and literal text in one template",
            template: r#"$context.httpMethod $stageVariables.backend"#,
            query: "",
            body: "POST beta",
        },
    ]
}

/// The path part of the row at `index`: one resource per row, so every row's template is
/// stood up on a method of its own and one deployment serves them all.
fn route_of(index: usize) -> String {
    format!("t{index}")
}

#[tokio::test]
async fn a_mapping_template_is_rendered_against_the_call_it_serves() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "mapping-templates").await;
    let rows = rendered_rows();
    for (index, row) in rows.iter().enumerate() {
        let resource = resource_under(&gateway, &api, &root, &route_of(index)).await;
        mock_route(
            &gateway,
            &api,
            &resource,
            json!({ JSON: r#"{"statusCode": 200}"# }),
            &[("200", row.template)],
        )
        .await;
    }
    let deployment = deployment_of(&gateway, &api).await;
    create_stage(&gateway, &api, &deployment).await;

    for (index, row) in rows.iter().enumerate() {
        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/{}{}", route_of(index), row.query),
            &[("content-type", JSON), ("x-trace", "t-1")],
            PAYLOAD,
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {}: expected the integration to answer 200, got {} with {:?}",
            row.case, answered.status, answered.body
        );
        assert_eq!(
            answered.body, row.body,
            "case {}: expected the rendered body {:?}, got {:?}",
            row.case, row.body, answered.body
        );
    }
}

#[tokio::test]
async fn the_api_id_and_the_stage_of_the_call_are_rendered_from_the_records() {
    // The two context members whose value is a per-run id: asserted against the records
    // the case created, not against a literal.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "mapping-template-context").await;
    let echo = resource_under(&gateway, &api, &root, "echo").await;
    mock_route(
        &gateway,
        &api,
        &echo,
        json!({ JSON: r#"{"statusCode": 200}"# }),
        &[(
            "200",
            r#"{"api":"$context.apiId","stage":"$context.stage","path":"$context.path","at":"$context.resourcePath"}"#,
        )],
    )
    .await;
    let deployment = deployment_of(&gateway, &api).await;
    create_stage(&gateway, &api, &deployment).await;

    let answered = invoke(
        &gateway,
        &api,
        "POST",
        &format!("/{STAGE}/echo"),
        &[("content-type", JSON)],
        PAYLOAD,
    )
    .await;
    let expected = json!({
        "api": api,
        "stage": STAGE,
        "path": format!("/{STAGE}/echo"),
        "at": "/echo",
    });
    let read: Value = serde_json::from_str(&answered.body).unwrap_or(Value::Null);
    assert_eq!(
        read, expected,
        "case the call's own context: expected {expected:?}, got {:?}",
        answered.body
    );
}

#[tokio::test]
async fn a_path_parameter_of_the_matched_resource_is_rendered_from_the_request() {
    // The path parameter is bound by the resource template the request matched, so the rows
    // differ along the segment the request carries — including one that looks like neither
    // of the others.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "mapping-template-path").await;
    let pets = resource_under(&gateway, &api, &root, "pets").await;
    let one = create_resource(&gateway, &api, &pets, "{petId}").await;
    let one = string_member(&one, "id", "{petId}");
    mock_route(
        &gateway,
        &api,
        &one,
        json!({ JSON: r#"{"statusCode": 200}"# }),
        &[("200", r#"{"pet":"$input.params('petId')"}"#)],
    )
    .await;
    let deployment = deployment_of(&gateway, &api).await;
    create_stage(&gateway, &api, &deployment).await;

    for segment in ["fido", "7", "a-b_c"] {
        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/pets/{segment}"),
            &[("content-type", JSON)],
            PAYLOAD,
        )
        .await;
        let expected = format!(r#"{{"pet":"{segment}"}}"#);
        assert_eq!(
            answered.body, expected,
            "case the path parameter {segment:?}: expected {expected:?}, got {:?}",
            answered.body
        );
    }
}

#[tokio::test]
async fn the_rendered_request_template_names_the_integration_response_the_call_maps_to() {
    // A `MOCK`'s request template is where the status comes from, so a template that
    // COMPUTES it from the request selects the integration response the rendering names —
    // and a request naming no stored response maps to the default one.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "mapping-template-status").await;
    let echo = resource_under(&gateway, &api, &root, "echo").await;
    mock_route(
        &gateway,
        &api,
        &echo,
        json!({ JSON: r#"{"statusCode": $input.params('code')}"# }),
        &[
            ("200", r#"{"mapped":"ok"}"#),
            ("201", r#"{"mapped":"created"}"#),
        ],
    )
    .await;
    let deployment = deployment_of(&gateway, &api).await;
    create_stage(&gateway, &api, &deployment).await;

    let cases: &[(&str, u16, &str)] = &[
        ("?code=200", 200, r#"{"mapped":"ok"}"#),
        ("?code=201", 201, r#"{"mapped":"created"}"#),
        // No code to render: the rendering is not a status-carrying object, so the call
        // maps to the default integration response, which is the lowest status code stored
        // without a selection pattern.
        ("", 200, r#"{"mapped":"ok"}"#),
    ];
    for (query, status, body) in cases {
        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/echo{query}"),
            &[("content-type", JSON)],
            PAYLOAD,
        )
        .await;
        assert_eq!(
            (answered.status, answered.body.as_str()),
            (*status, *body),
            "case the request template rendering {query:?}: expected {status} {body:?}, got \
             {} {:?}",
            answered.status,
            answered.body
        );
    }
}

#[tokio::test]
async fn a_template_this_emulator_does_not_render_is_a_configuration_error() {
    // The forms that are NOT rendered: a DIRECTIVE, which is a statement that would have to
    // be executed, and a reference naming a variable, a member or a transformation outside
    // the documented set this call carries values for. Each is answered through the api's
    // own `API_CONFIGURATION_ERROR` gateway response rather than with a body guessed at.
    let unrendered: &[(&str, &str)] = &[
        ("a directive", "#set($root = $input.path('$'))\n{\"a\":1}"),
        ("a comment directive", "## a comment"),
        (
            "a transformation this emulator does not perform",
            r#"{"parsed":"$util.parseJson($input.body)"}"#,
        ),
        (
            "a variable outside the documented set",
            r#"{"who":"$whatever.name"}"#,
        ),
        (
            "a context member this call carries no value for",
            r#"{"who":"$context.status"}"#,
        ),
        (
            "a stage variable the stage does not define",
            r#"{"where":"$stageVariables.undefined"}"#,
        ),
        (
            "a JSONPath selecting a structure rather than a value",
            r#"{"all":"$input.path('$')"}"#,
        ),
    ];
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "mapping-template-refusals").await;
    for (index, (_case, template)) in unrendered.iter().enumerate() {
        let resource = resource_under(&gateway, &api, &root, &route_of(index)).await;
        mock_route(
            &gateway,
            &api,
            &resource,
            json!({ JSON: r#"{"statusCode": 200}"# }),
            &[("200", template)],
        )
        .await;
    }
    let deployment = deployment_of(&gateway, &api).await;
    create_stage(&gateway, &api, &deployment).await;

    for (index, (case, _template)) in unrendered.iter().enumerate() {
        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/{}", route_of(index)),
            &[("content-type", JSON)],
            PAYLOAD,
        )
        .await;
        assert_eq!(
            answered.status, 500,
            "case {case}: expected the API_CONFIGURATION_ERROR status, got {} with {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.message(),
            "Internal server error",
            "case {case}: expected the gateway response's own sentence, got {:?}",
            answered.body
        );
    }
}

#[tokio::test]
async fn a_request_template_this_emulator_does_not_render_is_a_configuration_error() {
    // The integration request a `MOCK` answers with IS its request template rendered, so a
    // request template this emulator does not render leaves the call with no integration to
    // perform — the same exit its response template takes, rather than a status and a body
    // nothing produced.
    let unrendered: &[(&str, &str)] = &[
        ("a directive", "#set($code = 200)\n{\"statusCode\": $code}"),
        (
            "a context member this call carries no value for",
            r#"{"statusCode": 200, "who": "$context.status"}"#,
        ),
    ];
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "mapping-template-request-refusals").await;
    for (index, (_case, template)) in unrendered.iter().enumerate() {
        let resource = resource_under(&gateway, &api, &root, &route_of(index)).await;
        mock_route(
            &gateway,
            &api,
            &resource,
            json!({ JSON: template }),
            &[("200", r#"{"mapped":"ok"}"#)],
        )
        .await;
    }
    let deployment = deployment_of(&gateway, &api).await;
    create_stage(&gateway, &api, &deployment).await;

    for (index, (case, _template)) in unrendered.iter().enumerate() {
        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/{}", route_of(index)),
            &[("content-type", JSON)],
            PAYLOAD,
        )
        .await;
        assert_eq!(
            answered.status, 500,
            "case a request template with {case}: expected the API_CONFIGURATION_ERROR \
             status, got {} with {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.message(),
            "Internal server error",
            "case a request template with {case}: expected the gateway response's own \
             sentence, got {:?}",
            answered.body
        );
    }
}

#[tokio::test]
async fn a_simulated_invocation_renders_the_same_template_the_stage_does() {
    // `TestInvokeMethod` "simulate[s] the invocation of a Method" through the same records,
    // so one stored template renders one way whichever side drives it: the members the
    // caller simulates the request with stand where the wire request's own would.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "mapping-template-parity").await;
    let echo = resource_under(&gateway, &api, &root, "echo").await;
    mock_route(
        &gateway,
        &api,
        &echo,
        json!({ JSON: r#"{"statusCode": 200}"# }),
        &[(
            "200",
            r#"{"tenant":"$input.params('tenant')","verb":"$context.httpMethod","backend":"$stageVariables.backend"}"#,
        )],
    )
    .await;
    let deployment = deployment_of(&gateway, &api).await;
    create_stage(&gateway, &api, &deployment).await;

    let answered = invoke(
        &gateway,
        &api,
        "POST",
        &format!("/{STAGE}/echo?tenant=acme"),
        &[("content-type", JSON)],
        PAYLOAD,
    )
    .await;
    let simulated = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/resources/{echo}/methods/POST"),
            Some(json!({
                "pathWithQueryString": "/echo?tenant=acme",
                "body": PAYLOAD,
                "headers": { "Content-Type": JSON },
                "stageVariables": stage_variables(),
            })),
        )
        .await;
    assert_eq!(
        simulated.status, 200,
        "case TestInvokeMethod: expected 200, got {} with {:?}",
        simulated.status, simulated.body
    );
    let reported = string_member(&simulated.body, "body", "TestInvokeMethod");
    assert_eq!(
        reported, answered.body,
        "case the simulated rendering: expected the body the stage served {:?}, got {reported:?}",
        answered.body
    );
}

#[tokio::test]
async fn a_simulated_invocation_renders_the_path_and_headers_it_was_given() {
    // The members a caller simulates a request with are what its templates render against:
    // `pathWithQueryString` is where a caller "specif[ies] path parameters and query string
    // parameters", so a path parameter of the resource's own template is bound from it, and
    // a header reaches the template through EITHER member it may be carried in.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "mapping-template-simulated").await;
    let pets = resource_under(&gateway, &api, &root, "pets").await;
    let one = create_resource(&gateway, &api, &pets, "{petId}").await;
    let one = string_member(&one, "id", "{petId}");
    mock_route(
        &gateway,
        &api,
        &one,
        json!({ JSON: r#"{"statusCode": 200}"# }),
        &[(
            "200",
            r#"{"pet":"$input.params('petId')","tenant":"$input.params('tenant')","trace":"$input.params('x-trace')"}"#,
        )],
    )
    .await;

    // The header travels in `multiValueHeaders` only, which is the member a single-valued
    // read still sees the first value of.
    let simulated = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/resources/{one}/methods/POST"),
            Some(json!({
                "pathWithQueryString": "/pets/fido?tenant=acme",
                "body": PAYLOAD,
                "headers": { "Content-Type": JSON },
                "multiValueHeaders": { "x-trace": ["t-1", "t-2"] },
            })),
        )
        .await;
    assert_eq!(
        simulated.status, 200,
        "case TestInvokeMethod: expected 200, got {} with {:?}",
        simulated.status, simulated.body
    );
    let reported = string_member(&simulated.body, "body", "TestInvokeMethod");
    let expected = r#"{"pet":"fido","tenant":"acme","trace":"t-1"}"#;
    assert_eq!(
        reported, expected,
        "case the simulated path and headers: expected {expected:?}, got {reported:?}"
    );
}
