// SPDX-License-Identifier: CC0-1.0

//! The `execute-api` data plane NAMES THE REQUEST it answered: every response a deployed
//! REST API stage serves carries the id of that request in the response header API Gateway
//! reports it in, so a client holding a response can correlate it with what the API
//! processed (its access-log and execution-log records, a support case, a trace).
//!
//! The rule asserted here is about the RESPONSE, not about any particular id: an id is
//! minted per request and no value is ever a requirement, so what every row checks is that
//! the header is THERE, that it has the shape every AWS request id has, and that two
//! responses never report one id. The one request that names ITSELF is the exception the
//! table carries beside them: API Gateway documents the request id as one a client may
//! override by sending that same header, so such a response reports the id the request
//! stated. The rows differ along what the rule generalises over —
//! the outcome class (an integration served in full, each refusal class the data plane
//! answers, a server-side configuration fault), the addressing (the api's own invoke host
//! and the reserved data-plane path prefix), the verb, and whether the request carried a
//! SigV4 credential (which decides WHICH of the two seams serves it) — because the rule
//! holds for every response the endpoint serves, not for the successful ones.
//!
//! Every case drives the real wire path: an HTTP request through the multi-service router
//! into the mounted service, with the control plane driven through the same emulator
//! ([`Gateway`], this crate's one wire-test harness), so nothing here reaches into state.

use std::collections::BTreeSet;

use bytes::Bytes;
use serde_json::{json, Value};

use crate::rest_api_lifecycle::{create_api, create_resource, string_member, Gateway};

/// The region an api's own invoke host is addressed in; any region is served.
const REGION: &str = "us-east-1";

/// The emulator's own endpoint, as a client on this machine reaches it: a loopback address,
/// which names no service to the front door, so the data plane is addressed by the reserved
/// path prefix instead of by host.
const OWN_ENDPOINT: &str = "127.0.0.1:9297";

/// The reserved path prefix that names the data plane ahead of the api id.
const DATA_PLANE_PREFIX: &str = "/_aws/execute-api";

/// The response header the request id is reported in. API Gateway spells it
/// `x-amzn-RequestId`; HTTP header names are case-insensitive and the wire carries this one
/// lowercased, which is how it is read back here.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// A SigV4 credential scope, as a signed SDK request carries it. The signature itself is
/// the emulator's business and is not verified in these tests (the harness runs the router
/// with request authentication off), so what a case varies is its PRESENCE — which is what
/// decides whether the anonymous or the credentialed seam serves the invoke.
const SIGNED: &str =
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/execute-api/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// A request id a CLIENT made up and sent, so a row can assert the answer reports the id the
/// request STATED for itself — the override API Gateway documents — and every row that sent
/// none reports an id of the endpoint's own instead.
const CLIENT_SENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// One answered invoke: the status line, the reported request id, and the body verbatim.
struct Invoked {
    status: u16,
    request_id: Option<String>,
    body: String,
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form every AWS request id carries.
///
/// Only the header's PRESENCE and its per-request uniqueness are what a correlating client
/// needs; this pins the shape as well, so the reported id cannot drift into something no
/// AWS client would recognise as one.
fn has_request_id_form(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        let hex = group.len() == width
            && group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
        if !hex {
            return false;
        }
    }
    groups.next().is_none()
}

/// Issue one invoke of `path` with `verb`, addressed at `host`, carrying `headers`.
async fn invoke(
    gateway: &Gateway,
    host: &str,
    verb: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Invoked {
    let mut builder = http::Request::builder()
        .method(verb)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from_static(b"{\"sent\":true}"))
        .expect("the invoke request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let request_id = response
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    Invoked {
        status,
        request_id,
        body,
    }
}

/// The invoke host of `api`: the endpoint `RestApi$disableExecuteApiEndpoint` documents,
/// `https://{api_id}.execute-api.{region}.amazonaws.com`.
fn invoke_host(api: &str) -> String {
    format!("{api}.execute-api.{REGION}.amazonaws.com")
}

/// One control-plane call, asserting it was accepted.
async fn accepted(gateway: &Gateway, method: &str, uri: &str, request: Value) {
    let answer = gateway.call(method, uri, Some(request)).await;
    assert!(
        (200..300).contains(&answer.status),
        "case {uri}: expected the control-plane call to be accepted, got {} with {:?}",
        answer.status,
        answer.body
    );
}

/// A child resource of `parent` named `path_part`, answering its resource id.
async fn child(gateway: &Gateway, api: &str, parent: &str, path_part: &str) -> String {
    let created = create_resource(gateway, api, parent, path_part).await;
    string_member(&created, "id", &format!("CreateResource({path_part})"))
}

/// A `MOCK` `verb` method on `resource` under `authorization`, answering `body` — the one
/// integration type that needs no backend, so it is what a row asserting a SERVED response
/// is built from.
async fn mock_method(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    authorization: &str,
    body: &str,
) {
    let method = format!("/restapis/{api}/resources/{resource}/methods/{verb}");
    accepted(
        gateway,
        "PUT",
        &method,
        json!({ "authorizationType": authorization }),
    )
    .await;
    accepted(
        gateway,
        "PUT",
        &format!("{method}/integration"),
        json!({
            "type": "MOCK",
            "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
        }),
    )
    .await;
    accepted(gateway, "PUT", &format!("{method}/responses/200"), json!({})).await;
    accepted(
        gateway,
        "PUT",
        &format!("{method}/integration/responses/200"),
        json!({ "responseTemplates": { "application/json": body } }),
    )
    .await;
}

/// `CreateDeployment` of `stage`, as a developer publishes one.
async fn deploy(gateway: &Gateway, api: &str, stage: &str) {
    accepted(
        gateway,
        "POST",
        &format!("/restapis/{api}/deployments"),
        json!({ "stageName": stage }),
    )
    .await;
}

/// One api deployed to `prod` with a method for each outcome class the table below drives,
/// answering the api id.
async fn deployed_api(gateway: &Gateway, name: &str) -> String {
    let (api, root) = create_api(gateway, name).await;
    mock_method(gateway, &api, &root, "GET", "NONE", r#"{"at":"root"}"#).await;
    let pets = child(gateway, &api, &root, "pets").await;
    mock_method(gateway, &api, &pets, "GET", "NONE", r#"{"at":"pets"}"#).await;
    let items = child(gateway, &api, &root, "items").await;
    mock_method(gateway, &api, &items, "POST", "NONE", r#"{"at":"items"}"#).await;
    // Reached only with a SigV4 credential, which is what routes the invoke through the
    // credentialed seam rather than the anonymous one.
    let signed = child(gateway, &api, &root, "signed").await;
    mock_method(gateway, &api, &signed, "GET", "AWS_IAM", r#"{"at":"signed"}"#).await;
    // A method whose authorization type no authorizer of this api can serve: a
    // configuration the stage cannot authorize, which is the server-side fault class. (A
    // `CUSTOM` method naming no authorizer cannot be put at all — `PutMethod` requires one
    // the api holds — so the type is one no `Method$authorizationType` value names.)
    let broken = child(gateway, &api, &root, "broken").await;
    accepted(
        gateway,
        "PUT",
        &format!("/restapis/{api}/resources/{broken}/methods/GET"),
        json!({ "authorizationType": "SOMETHING_ELSE" }),
    )
    .await;
    // Its integration, without which the api could not be deployed at all: what this row
    // drives is the authorization the stage cannot serve, not a missing backend.
    accepted(
        gateway,
        "PUT",
        &format!("/restapis/{api}/resources/{broken}/methods/GET/integration"),
        json!({ "type": "MOCK" }),
    )
    .await;
    deploy(gateway, &api, "prod").await;
    api
}

#[tokio::test]
async fn every_response_a_deployed_stage_serves_names_the_request_it_answered() {
    let gateway = Gateway::new();
    let api = deployed_api(&gateway, "request-id-table").await;

    /// One driven invoke: how it addresses the endpoint, what it carries, and the outcome
    /// class it lands in — named so a row that stops exercising its class fails loudly
    /// instead of asserting the header on some other answer.
    struct Case {
        case: &'static str,
        host: String,
        verb: &'static str,
        path: String,
        headers: Vec<(&'static str, &'static str)>,
        status: u16,
    }

    let cases = vec![
        Case {
            case: "an integration served in full on the stage root, on the api's own host",
            host: invoke_host(&api),
            verb: "GET",
            path: "/prod/".to_string(),
            headers: vec![],
            status: 200,
        },
        Case {
            case: "an integration served in full through the reserved data-plane prefix",
            host: OWN_ENDPOINT.to_string(),
            verb: "GET",
            path: format!("{DATA_PLANE_PREFIX}/{api}/prod/pets"),
            headers: vec![],
            status: 200,
        },
        Case {
            case: "an integration served in full on another verb",
            host: invoke_host(&api),
            verb: "POST",
            path: "/prod/items".to_string(),
            headers: vec![],
            status: 200,
        },
        Case {
            case: "an AWS_IAM method reached WITH a SigV4 credential, through the \
                   credentialed seam",
            host: invoke_host(&api),
            verb: "GET",
            path: "/prod/signed".to_string(),
            headers: vec![("authorization", SIGNED)],
            status: 200,
        },
        Case {
            case: "an AWS_IAM method reached with no credential at all",
            host: invoke_host(&api),
            verb: "GET",
            path: "/prod/signed".to_string(),
            headers: vec![],
            status: 403,
        },
        Case {
            case: "a path the deployment holds no resource for",
            host: invoke_host(&api),
            verb: "GET",
            path: "/prod/absent".to_string(),
            headers: vec![],
            status: 403,
        },
        Case {
            case: "a stage this api does not hold",
            host: invoke_host(&api),
            verb: "GET",
            path: "/staging/pets".to_string(),
            headers: vec![],
            status: 403,
        },
        Case {
            case: "a method whose authorization type no authorizer can serve",
            host: invoke_host(&api),
            verb: "GET",
            path: "/prod/broken".to_string(),
            headers: vec![],
            status: 500,
        },
        Case {
            case: "a request that states a request id of its own",
            host: invoke_host(&api),
            verb: "GET",
            path: "/prod/pets".to_string(),
            headers: vec![(REQUEST_ID_HEADER, CLIENT_SENT_ID)],
            status: 200,
        },
    ];

    let mut reported: BTreeSet<String> = BTreeSet::new();
    for case in &cases {
        let answered = invoke(&gateway, &case.host, case.verb, &case.path, &case.headers).await;
        assert_eq!(
            answered.status, case.status,
            "case {:?}: expected the {} outcome class, got {} with {:?}",
            case.case, case.status, answered.status, answered.body
        );
        let Some(request_id) = answered.request_id.clone() else {
            panic!(
                "case {:?}: expected the response to carry a {REQUEST_ID_HEADER} header, got \
                 none ({} with {:?})",
                case.case, answered.status, answered.body
            );
        };
        assert!(
            has_request_id_form(&request_id),
            "case {:?}: expected a request id of the 8-4-4-4-12 lowercase-hex form, got \
             {request_id:?}",
            case.case
        );
        match case
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(REQUEST_ID_HEADER))
        {
            // The request named ITSELF, which is the override API Gateway documents: the
            // answer reports that id and not one of the endpoint's own.
            Some((_, stated)) => assert_eq!(
                request_id, *stated,
                "case {:?}: expected the answer to report the id the request stated, got \
                 {request_id:?}",
                case.case
            ),
            None => assert_ne!(
                request_id, CLIENT_SENT_ID,
                "case {:?}: expected the id the endpoint minted for this request, which no \
                 request stated, got {request_id:?}",
                case.case
            ),
        }
        assert!(
            reported.insert(request_id.clone()),
            "case {:?}: expected an id no other response reported, got the already-reported \
             {request_id:?}",
            case.case
        );
    }

    // The point of the table: EVERY outcome class reports one, so a client correlating on
    // the header never has to branch on whether its call succeeded.
    assert_eq!(
        reported.len(),
        cases.len(),
        "case \"the table as a whole\": expected one distinct id per response, got {reported:?}"
    );
}

#[tokio::test]
async fn repeating_one_invoke_reports_a_different_id_each_time() {
    let gateway = Gateway::new();
    let api = deployed_api(&gateway, "request-id-per-request").await;

    // The id names the REQUEST, not the stage, the deployment or the method: the same call,
    // byte for byte, is answered with a fresh one every time. Four repeats — more than one,
    // so a cached or state-derived id cannot pass.
    let mut reported: BTreeSet<String> = BTreeSet::new();
    for repeat in 1..=4 {
        let answered = invoke(&gateway, &invoke_host(&api), "GET", "/prod/pets", &[]).await;
        assert_eq!(
            answered.status, 200,
            "case \"repeat {repeat}\": expected 200, got {} with {:?}",
            answered.status, answered.body
        );
        let request_id = answered.request_id.unwrap_or_default();
        assert!(
            has_request_id_form(&request_id),
            "case \"repeat {repeat}\": expected a reported request id, got {request_id:?}"
        );
        assert!(
            reported.insert(request_id.clone()),
            "case \"repeat {repeat}\": expected an id no earlier repeat reported, got \
             {request_id:?}"
        );
    }
    assert_eq!(
        reported.len(),
        4,
        "case \"four repeats of one invoke\": expected four distinct ids, got {reported:?}"
    );
}
