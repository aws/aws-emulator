// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how a request to a FUNCTION URL that the emulator's authentication
//! layer turned away is worded.
//!
//! RULE: a function URL is an HTTP endpoint a stock client follows, so a caller that endpoint
//! does not admit is answered the refusal Lambda's own front door answers —
//! `403 {"Message":"Forbidden"}` — and not the refusal of this service's modelled API. The
//! same layer turning away a CONTROL-plane call still words it as that API's own refusal, so
//! the restating is of one request space and not of every request.
//!
//! `AuthType` decides who is turned away: `AWS_IAM` "restrict[s] access to authenticated
//! users only", so a request whose SigV4 signature the emulator did not verify is refused,
//! while `NONE` "bypass[es] IAM authentication to create a public endpoint" and is served
//! however it arrived — which is what makes the refusal the auth type's rather than a
//! blanket one.
//!
//! Every row drives a real HTTP request through a router that VERIFIES signatures, as the
//! emulator does when it is started with request authentication on. The rows differ along
//! what the rule generalises over: how the request is credentialled (no credential at all, a
//! signature scoped to another service, a signature that does not verify), which of the
//! addressings reaches the endpoint (the published host, the local-endpoint path, the
//! path-style first segment), the `AuthType` the URL enforces, and whether the request
//! addresses a function URL at all.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::function_url_data_plane::{
    given_configured_url, given_function, router_on, send, Answer, Endpoint,
};

/// The emulator's own endpoint, which a client that resolves no AWS hostname reaches, and
/// where the two path addressings are read.
const OWN_ENDPOINT: &str = "127.0.0.1:9297";

/// The reserved path segment a data plane is named behind on that endpoint
/// (`service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`), and the label that
/// names this plane under it.
const EMULATOR_NAMESPACE: &str = "_aws";
/// See [`EMULATOR_NAMESPACE`].
const PLANE_LABEL: &str = "lambda-url";

/// The body Lambda's own front door answers a caller it does not admit with.
const FORBIDDEN_BODY: &str = r#"{"Message":"Forbidden"}"#;

/// The handler every function below runs, which answers a described response so a row that
/// IS admitted is visibly served by the function.
fn handler() -> String {
    r#"
def lambda_handler(event, context):
    return {"statusCode": 200, "body": "served"}
"#
    .to_string()
}

/// How a request is credentialled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Credential {
    /// No `Authorization` header at all.
    None,
    /// A signature whose credential scope names another service.
    ScopedElsewhere,
    /// A signature scoped to this service that does not verify.
    Mismatched,
}

/// The headers one credential travels in.
fn headers(credential: Credential) -> Vec<(&'static str, &'static str)> {
    match credential {
        Credential::None => Vec::new(),
        Credential::ScopedElsewhere => vec![
            (
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=emulator-test-key/20260101/us-east-1/s3/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef",
            ),
            ("x-amz-date", "20260101T000000Z"),
        ],
        Credential::Mismatched => vec![
            (
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef",
            ),
            ("x-amz-date", "20260101T000000Z"),
        ],
    }
}

/// Which addressing a row reaches the endpoint by: the three a function URL resolves under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Addressing {
    /// The host `FunctionUrlConfig$FunctionUrl` publishes.
    PublishedHost,
    /// The reserved local-endpoint path on the emulator's own endpoint.
    LocalEndpointPath,
    /// The url id as the FIRST path segment on the emulator's own endpoint.
    PathStyle,
}

/// The host and path one addressing of `endpoint` reaches it at.
fn addressed(endpoint: &Endpoint, addressing: Addressing) -> (String, String) {
    match addressing {
        Addressing::PublishedHost => (endpoint.host.clone(), "/probe".to_string()),
        Addressing::LocalEndpointPath => (
            OWN_ENDPOINT.to_string(),
            format!("/{EMULATOR_NAMESPACE}/{PLANE_LABEL}/{}/probe", endpoint.url_id),
        ),
        Addressing::PathStyle => (
            OWN_ENDPOINT.to_string(),
            format!("/{}/probe", endpoint.url_id),
        ),
    }
}

/// One row of the refusal table.
struct Row {
    case: &'static str,
    credential: Credential,
    addressing: Addressing,
}

/// RULE: every caller an `AWS_IAM` function URL does not admit is answered Lambda's own
/// front-door refusal, whichever way the credential failed and whichever addressing reached
/// the endpoint.
#[tokio::test]
async fn an_unadmitted_caller_of_a_function_url_reads_lambdas_own_forbidden() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    given_function(&router, "url-refused", &handler()).await;
    let restricted = given_configured_url(&router, "url-refused", None, "AWS_IAM", &Value::Null)
        .await;
    // From here on the SAME emulator verifies request signatures, as it does when it is
    // started with request authentication on.
    let mut authenticating = router_on(&engine);
    authenticating.set_request_authentication(true);
    // The unauthenticated router above is kept only to build the resources; nothing below
    // uses it.
    router.set_request_authentication(true);

    let rows: &[Row] = &[
        Row {
            case: "no credential at all, on the published host",
            credential: Credential::None,
            addressing: Addressing::PublishedHost,
        },
        Row {
            case: "a signature scoped to another service, on the published host",
            credential: Credential::ScopedElsewhere,
            addressing: Addressing::PublishedHost,
        },
        Row {
            case: "a signature that does not verify, on the published host",
            credential: Credential::Mismatched,
            addressing: Addressing::PublishedHost,
        },
        Row {
            case: "no credential at all, on the local-endpoint path",
            credential: Credential::None,
            addressing: Addressing::LocalEndpointPath,
        },
        Row {
            case: "a signature that does not verify, on the local-endpoint path",
            credential: Credential::Mismatched,
            addressing: Addressing::LocalEndpointPath,
        },
        Row {
            case: "no credential at all, path-style on the emulator's own endpoint",
            credential: Credential::None,
            addressing: Addressing::PathStyle,
        },
    ];
    for row in rows {
        let (host, path) = addressed(&restricted, row.addressing);
        let answer = send(
            &authenticating,
            "GET",
            &host,
            &path,
            &[],
            &headers(row.credential),
        )
        .await;
        assert_eq!(
            answer.status,
            403,
            "case {:?}: expected HTTP 403, got {}",
            row.case,
            answer.summary()
        );
        assert_eq!(
            answer.text(),
            FORBIDDEN_BODY,
            "case {:?}: expected the body {FORBIDDEN_BODY}, got {}",
            row.case,
            answer.summary()
        );
    }
}

/// RULE: the restating is of the function-URL request space alone. A `NONE` endpoint granted
/// to the public is SERVED however it arrived, and a control-plane call the same
/// authentication layer turns away keeps this service's own API refusal — an error code and
/// the layer's own sentence, never Lambda's front-door body.
#[tokio::test]
async fn only_a_function_url_request_reads_that_refusal() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    given_function(&router, "url-public", &handler()).await;
    let public =
        given_configured_url(&router, "url-public", None, "NONE", &Value::Null).await;
    // What makes a `NONE` endpoint public is the function's own resource-based policy.
    crate::function_url_data_plane::grant(
        &router,
        "url-public",
        None,
        &json!({
            "StatementId": "public-url",
            "Action": "lambda:InvokeFunctionUrl",
            "Principal": "*",
            "FunctionUrlAuthType": "NONE",
        }),
    )
    .await;

    let mut authenticating = router_on(&engine);
    authenticating.set_request_authentication(true);

    // A public endpoint, reached with no credential at all: served by the function.
    let served = send(
        &authenticating,
        "GET",
        &public.host,
        "/probe",
        &[],
        &[],
    )
    .await;
    assert_eq!(
        served.status,
        200,
        "case \"a public endpoint carrying no credential\": expected the handler's answer, got {}",
        served.summary()
    );
    assert_ne!(
        served.text(),
        FORBIDDEN_BODY,
        "case \"a public endpoint carrying no credential\": expected the function's own answer, \
         got {}",
        served.summary()
    );

    // A CONTROL-plane call the same layer turns away, over each credential: this service's
    // own API refusal, which names an error code and never Lambda's front-door body.
    // (case, the credential)
    let rows: &[(&str, Credential)] = &[
        ("no credential at all", Credential::None),
        (
            "a signature scoped to another service",
            Credential::ScopedElsewhere,
        ),
        (
            "a signature that does not verify",
            Credential::Mismatched,
        ),
    ];
    for (case, credential) in rows {
        let answer: Answer = send(
            &authenticating,
            "GET",
            "lambda.us-east-1.amazonaws.com",
            "/2015-03-31/functions/url-public/configuration",
            &[],
            &headers(*credential),
        )
        .await;
        assert_ne!(
            answer.text(),
            FORBIDDEN_BODY,
            "case {case:?}: a control-plane refusal keeps this service's own wording, got {}",
            answer.summary()
        );
        // A request the layer turned away is refused; one it admitted is served. Either way
        // the body is the API's, which is what this row is about: the function-URL wording
        // reaches no control-plane answer.
        if answer.status == 403 {
            assert!(
                answer
                    .json()
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some(),
                "case {case:?}: expected this service's own refusal envelope, got {}",
                answer.summary()
            );
        }
    }
}
