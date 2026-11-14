// SPDX-License-Identifier: CC0-1.0

//! The `execute-api` HTTP data plane's CORS decoration: which `Access-Control-*` headers a
//! deployed HTTP api's stage puts on an answer, and which it withholds.
//!
//! The rule these cases assert, one level above the recorded case (a CDK-synthesised
//! `HttpApi` whose `corsPreflight` allows one origin and the `GET` method, fronting a
//! `$default` route):
//!
//! > An api that carries a `CorsConfiguration` at all OWNS the `Access-Control-*` headers of
//! > every answer it gives, whatever produced that answer: an `Origin` the configuration's
//! > `AllowOrigins` admits is echoed back in `access-control-allow-origin` (the wildcard when
//! > that is what admitted it), an actual request additionally carries `ExposeHeaders`, either
//! > carries `access-control-allow-credentials: true` when `AllowCredentials` is set, and a
//! > request whose origin the configuration does not admit — matching exactly or through the
//! > wildcard entry — carries NO `Access-Control-*` header at all. A PREFLIGHT — the `OPTIONS`
//! > request carrying `Access-Control-Request-Method` — is answered by the GATEWAY itself with
//! > `204` and the configured `AllowMethods`, `AllowHeaders` and `MaxAge` when the method it
//! > asks about is admitted (and with no CORS header when the origin or that method is not),
//! > reaching no route and no integration of the api. An api carrying no configuration
//! > decorates nothing and answers an `OPTIONS` request from its routes like any other.
//!
//! `smithy_query(query="shape", shape="Cors")` is where each member's meaning comes from —
//! `AllowOrigins` "a collection of allowed origins", `AllowMethods` "a collection of allowed
//! HTTP methods", `AllowHeaders` "a collection of allowed headers", `ExposeHeaders` "a
//! collection of exposed headers", `AllowCredentials` "whether credentials are included in
//! the CORS request", `MaxAge` "the number of seconds that the browser should cache preflight
//! request results" with `range` -1..86400 — and `Api$CorsConfiguration` is why the gateway
//! and not a backend applies it: the configuration is a member of the API.
//!
//! The rows span what the rule generalises over: how many origins the collection holds and
//! which one the request comes from, the wildcard entry, a spelling that differs from the
//! collection's only in case, a collection that is absent or empty, whether the request is a
//! preflight, whether the method it asks about is admitted, which `Cors` members the
//! configuration carries at all, and WHICH BACKEND the matched route fronts — an
//! `HTTP_PROXY` upstream the caller is redirected to (`307`) and an `AWS_PROXY` integration
//! that produces no response (`500`) — because the decoration is the api's and is
//! independent of what answered. The recorded case is one row.
//!
//! What these cases do not reach: the `200` and the handler's body the recorded check polls
//! for. That answer is the Lambda function's own, and this emulator has no in-process runtime
//! for a deployment package (lambda `Invoke` answers `501` for `RequestResponse`), so an
//! `AWS_PROXY` route here meets the "integration produced no response" class
//! (`http_api_integration_fault_status`). The runtime is owned by the open issues
//! `20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md`
//! and `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`,
//! and the dispatch that maps a function's answer back onto the response by
//! `20261025T030504Z-apigatewayv2-lambda-an-aws-proxy-route-cannot-answer-200-until-a.md`.
//! The decoration asserted here is applied to whatever those produce.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator};

/// The stage that serves an api's root, reached with no stage segment in the path at all.
const ROOT_STAGE: &str = "$default";

/// The prefix of every response header this suite is about, so an answer can be asserted to
/// carry EXACTLY the decoration the configuration states and nothing else.
const CORS_HEADER_PREFIX: &str = "access-control-";

/// The status an `HTTP_PROXY` upstream's caller is redirected with.
const REDIRECT: u16 = 307;

/// The status a matched route whose integration produced no response is answered with.
const INTEGRATION_FAULT: u16 = 500;

/// The status the GATEWAY answers a preflight of a CORS-configured api with: it answers the
/// preflight itself, so no route of the api is matched and no integration is reached.
const PREFLIGHT: u16 = 204;

/// The modelled bounds of `IntegerWithLengthBetweenMinus1And86400`, the shape `Cors$MaxAge`
/// targets (`range` min -1 max 86400). NEITHER is a bound the service enforces, so a value
/// outside them is stored and reported like any other; see the note on
/// `crate::ops::types::Cors` in the crate under test.
const MODELLED_MIN_MAX_AGE: i64 = -1;
const MODELLED_MAX_MAX_AGE: i64 = 86_400;

/// The backend the matched route fronts. The decoration is the api's, so it must not depend
/// on which of these answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    /// "HTTP_PROXY: for integrating the route or method request with an HTTP endpoint"
    /// (`IntegrationType`) — the caller is redirected to the stored `IntegrationUri`.
    HttpProxy,
    /// "AWS_PROXY: for integrating the route or method request with the Lambda
    /// function-invoking action" (`IntegrationType`).
    AwsProxy,
}

impl Backend {
    fn integration(self) -> Value {
        match self {
            Self::HttpProxy => json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://upstream.example.test/forward",
                "integrationMethod": "ANY",
                "payloadFormatVersion": "1.0"
            }),
            Self::AwsProxy => json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function:cors-backend",
                "payloadFormatVersion": "2.0"
            }),
        }
    }

    /// The status this backend's answer carries, which the decoration must leave alone.
    fn status(self) -> u16 {
        match self {
            Self::HttpProxy => REDIRECT,
            Self::AwsProxy => INTEGRATION_FAULT,
        }
    }
}

/// The answer to one unsigned data-plane request: its status and every `Access-Control-*`
/// header it carries.
struct Answer {
    status: u16,
    cors: BTreeMap<String, String>,
    /// Where an `HTTP_PROXY` redirect sends the caller, so the row can assert the decoration
    /// did not displace the answer itself.
    location: Option<String>,
}

impl Answer {
    fn summary(&self) -> String {
        format!("{} with CORS headers {:?}", self.status, self.cors)
    }
}

/// Send one unsigned request to an api's invoke URL — what a browser does.
async fn invoke(
    emulator: &MultiServiceRouter,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::new())
        .expect("build the invoke request");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let mut cors = BTreeMap::new();
    let mut location = None;
    for (name, value) in response.headers() {
        let name = name.as_str().to_ascii_lowercase();
        let value = String::from_utf8_lossy(value.as_bytes()).to_string();
        if name.starts_with(CORS_HEADER_PREFIX) {
            cors.insert(name, value);
        } else if name == "location" {
            location = Some(value);
        }
    }
    Answer {
        status,
        cors,
        location,
    }
}

/// Create an HTTP api carrying `cors`, one integration, a `$default` route over it and a
/// deployed root stage; answer the host its own `ApiEndpoint` names and its id.
async fn deploy(
    emulator: &MultiServiceRouter,
    case: &str,
    cors: Option<Value>,
    backend: Backend,
) -> (String, String) {
    let mut request = json!({ "name": "cors-data-plane-api", "protocolType": "HTTP" });
    if let Some(cors) = cors {
        request["corsConfiguration"] = cors;
    }
    let (status, api) = call(emulator, "POST", "/v2/apis", Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi must answer 201, got {status} {api}"
    );
    let api_id = as_str(&api, "apiId");
    let endpoint = as_str(&api, "apiEndpoint");
    let endpoint_host = endpoint
        .split_once("://")
        .map_or(endpoint.as_str(), |(_scheme, host)| host)
        .to_string();

    let (status, integration) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(backend.integration()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration must answer 201, got {status} {integration}"
    );
    let integration_id = as_str(&integration, "integrationId");

    // The `$default` route catches every method and path, so one deployment serves both the
    // actual request and the preflight.
    let (status, route) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": "$default",
            "target": format!("integrations/{integration_id}")
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoute must answer 201, got {status} {route}"
    );

    let (status, stage) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": ROOT_STAGE, "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201, got {status} {stage}"
    );
    (api_id, endpoint_host)
}

/// One row: the api's CORS configuration, the request that reaches it, and the decoration
/// that request's answer must carry — stated in full, so a row asserts the exact header set
/// rather than a subset.
struct Row {
    case: &'static str,
    /// The `corsConfiguration` the api is created with, or `None` for an api carrying none.
    cors: Option<fn() -> Value>,
    method: &'static str,
    /// The request headers the browser sends: `Origin`, and for a preflight also
    /// `Access-Control-Request-Method`.
    request_headers: &'static [(&'static str, &'static str)],
    /// Every `Access-Control-*` header the answer must carry, and nothing else.
    expected: &'static [(&'static str, &'static str)],
}

impl Row {
    /// Whether the GATEWAY answers this request itself: a PREFLIGHT — the `OPTIONS` request
    /// carrying `Access-Control-Request-Method` — of an api that carries a configuration at
    /// all, whether or not that configuration admits the origin or the method it asks about.
    fn answered_by_the_gateway(&self) -> bool {
        self.cors.is_some()
            && self.method == "OPTIONS"
            && self
                .request_headers
                .iter()
                .any(|(name, _)| *name == "access-control-request-method")
    }

    /// The status this row's answer must carry: [`PREFLIGHT`] for a preflight the gateway
    /// answered itself, and the backend's own class for every request a route served.
    fn status(&self, backend: Backend) -> u16 {
        if self.answered_by_the_gateway() {
            PREFLIGHT
        } else {
            backend.status()
        }
    }
}

/// The configuration the recorded case carries: one allow-listed origin and the `GET` method.
fn one_origin_one_method() -> Value {
    json!({
        "allowOrigins": ["https://example.test"],
        "allowMethods": ["GET"]
    })
}

/// Several origins and several methods, so the row can show WHICH origin is echoed and that
/// the whole method collection is what the preflight answers with.
fn several_origins_and_methods() -> Value {
    json!({
        "allowOrigins": ["https://first.example.test", "https://second.example.test"],
        "allowMethods": ["GET", "POST", "DELETE"]
    })
}

/// The wildcard in both collections: "the entry a CORS list carries to stand for every
/// value".
fn wildcard() -> Value {
    json!({ "allowOrigins": ["*"], "allowMethods": ["*"] })
}

/// A configuration carrying the other members but NO `AllowOrigins` at all — the member the
/// whole rule hangs on, so nothing is admitted and nothing is decorated.
fn no_allowed_origins() -> Value {
    json!({ "allowMethods": ["GET"], "allowHeaders": ["content-type"], "maxAge": 600 })
}

/// A configuration whose collections are present but EMPTY, which names no origin and no
/// method either.
fn empty_collections() -> Value {
    json!({ "allowOrigins": [], "allowMethods": [] })
}

/// Every member of `Cors` set at once. Credentials are allowed, which is why neither
/// collection carries the wildcard — a configuration that allows credentialed requests
/// cannot answer them with one, and the control plane refuses that pair.
fn every_member() -> Value {
    json!({
        "allowOrigins": ["https://example.test"],
        "allowMethods": ["GET", "OPTIONS"],
        "allowHeaders": ["content-type", "x-api-key"],
        "exposeHeaders": ["x-trace", "x-request-id"],
        "allowCredentials": true,
        "maxAge": 600
    })
}

fn rows() -> &'static [Row] {
    &[
        Row {
            case: "the recorded case: a simple GET from the one allow-listed origin",
            cors: Some(one_origin_one_method),
            method: "GET",
            request_headers: &[("origin", "https://example.test")],
            expected: &[("access-control-allow-origin", "https://example.test")],
        },
        Row {
            case: "the recorded case: a preflight from that origin asking about GET",
            cors: Some(one_origin_one_method),
            method: "OPTIONS",
            request_headers: &[
                ("origin", "https://example.test"),
                ("access-control-request-method", "GET"),
            ],
            expected: &[
                ("access-control-allow-origin", "https://example.test"),
                ("access-control-allow-methods", "GET"),
            ],
        },
        Row {
            case: "a preflight asking about a method the configuration does not allow",
            cors: Some(one_origin_one_method),
            method: "OPTIONS",
            request_headers: &[
                ("origin", "https://example.test"),
                ("access-control-request-method", "DELETE"),
            ],
            expected: &[],
        },
        Row {
            case: "a simple GET from an origin the configuration does not allow",
            cors: Some(one_origin_one_method),
            method: "GET",
            request_headers: &[("origin", "https://evil.example.test")],
            expected: &[],
        },
        Row {
            case: "a preflight from an origin the configuration does not allow",
            cors: Some(one_origin_one_method),
            method: "OPTIONS",
            request_headers: &[
                ("origin", "https://evil.example.test"),
                ("access-control-request-method", "GET"),
            ],
            expected: &[],
        },
        Row {
            case: "a request carrying no Origin at all asks for no decoration",
            cors: Some(one_origin_one_method),
            method: "GET",
            request_headers: &[],
            expected: &[],
        },
        Row {
            case: "the SECOND of several allow-listed origins is the one echoed",
            cors: Some(several_origins_and_methods),
            method: "GET",
            request_headers: &[("origin", "https://second.example.test")],
            expected: &[("access-control-allow-origin", "https://second.example.test")],
        },
        Row {
            case: "a preflight against several allowed methods carries the whole collection",
            cors: Some(several_origins_and_methods),
            method: "OPTIONS",
            request_headers: &[
                ("origin", "https://first.example.test"),
                ("access-control-request-method", "DELETE"),
            ],
            expected: &[
                ("access-control-allow-origin", "https://first.example.test"),
                ("access-control-allow-methods", "GET,POST,DELETE"),
            ],
        },
        Row {
            case: "an origin whose host differs from the allow-listed one only in case is \
                   not that origin",
            cors: Some(one_origin_one_method),
            method: "GET",
            request_headers: &[("origin", "https://EXAMPLE.test")],
            expected: &[],
        },
        Row {
            case: "the wildcard answers the wildcard, for an origin it never named",
            cors: Some(wildcard),
            method: "GET",
            request_headers: &[("origin", "https://anywhere.example.test")],
            expected: &[("access-control-allow-origin", "*")],
        },
        Row {
            case: "a preflight under the wildcard, for a method it never named",
            cors: Some(wildcard),
            method: "OPTIONS",
            request_headers: &[
                ("origin", "https://anywhere.example.test"),
                ("access-control-request-method", "PATCH"),
            ],
            expected: &[
                ("access-control-allow-origin", "*"),
                ("access-control-allow-methods", "*"),
            ],
        },
        Row {
            case: "every member set: a preflight carries the allow-lists, the max age and \
                   the credentials flag, and not the exposed headers",
            cors: Some(every_member),
            method: "OPTIONS",
            request_headers: &[
                ("origin", "https://example.test"),
                ("access-control-request-method", "GET"),
            ],
            expected: &[
                ("access-control-allow-origin", "https://example.test"),
                ("access-control-allow-methods", "GET,OPTIONS"),
                ("access-control-allow-headers", "content-type,x-api-key"),
                ("access-control-max-age", "600"),
                ("access-control-allow-credentials", "true"),
            ],
        },
        Row {
            case: "every member set: the actual request carries the exposed headers and the \
                   credentials flag, and none of the preflight-only ones",
            cors: Some(every_member),
            method: "GET",
            request_headers: &[("origin", "https://example.test")],
            expected: &[
                ("access-control-allow-origin", "https://example.test"),
                ("access-control-expose-headers", "x-trace,x-request-id"),
                ("access-control-allow-credentials", "true"),
            ],
        },
        Row {
            case: "an OPTIONS request carrying an Origin but naming no method is not a \
                   preflight, so it is decorated as an actual request",
            cors: Some(every_member),
            method: "OPTIONS",
            request_headers: &[("origin", "https://example.test")],
            expected: &[
                ("access-control-allow-origin", "https://example.test"),
                ("access-control-expose-headers", "x-trace,x-request-id"),
                ("access-control-allow-credentials", "true"),
            ],
        },
        Row {
            case: "a configuration naming no allowed origin at all admits none",
            cors: Some(no_allowed_origins),
            method: "GET",
            request_headers: &[("origin", "https://example.test")],
            expected: &[],
        },
        Row {
            case: "a preflight against a configuration naming no allowed origin",
            cors: Some(no_allowed_origins),
            method: "OPTIONS",
            request_headers: &[
                ("origin", "https://example.test"),
                ("access-control-request-method", "GET"),
            ],
            expected: &[],
        },
        Row {
            case: "empty collections name no origin and no method",
            cors: Some(empty_collections),
            method: "GET",
            request_headers: &[("origin", "https://example.test")],
            expected: &[],
        },
        Row {
            case: "an api carrying no CorsConfiguration decorates nothing",
            cors: None,
            method: "GET",
            request_headers: &[("origin", "https://example.test")],
            expected: &[],
        },
        Row {
            case: "a preflight against an api carrying no CorsConfiguration",
            cors: None,
            method: "OPTIONS",
            request_headers: &[
                ("origin", "https://example.test"),
                ("access-control-request-method", "GET"),
            ],
            expected: &[],
        },
    ]
}

/// Assert the answer carries EXACTLY `expected` as its `Access-Control-*` headers, and the
/// status this row's answer owes.
fn decorated(case: &str, backend: Backend, row: &Row, answer: &Answer) {
    let wanted: BTreeMap<String, String> = row
        .expected
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        answer.cors,
        wanted,
        "case {case:?} behind {backend:?}: expected the CORS headers {wanted:?}, got {}",
        answer.summary()
    );
    // Either the gateway answered the preflight itself — which reaches no backend at all, so
    // the status is its own and no `Location` of an upstream is carried — or the answer is the
    // one the data plane already computed, whose status the decoration leaves untouched.
    assert_eq!(
        answer.status,
        row.status(backend),
        "case {case:?} behind {backend:?}: expected status {}, got {}",
        row.status(backend),
        answer.summary()
    );
    if backend == Backend::HttpProxy && !row.answered_by_the_gateway() {
        assert_eq!(
            answer.location.as_deref(),
            Some("https://upstream.example.test/forward"),
            "case {case:?}: the redirect must still name the upstream, got {:?}",
            answer.location
        );
    }
    if row.answered_by_the_gateway() {
        assert_eq!(
            answer.location, None,
            "case {case:?} behind {backend:?}: a preflight the gateway answered reaches no \
             integration, so it carries no upstream, got {:?}",
            answer.location
        );
    }
}

#[tokio::test]
async fn an_apis_cors_configuration_decorates_what_its_stage_answers() {
    for row in rows() {
        for backend in [Backend::HttpProxy, Backend::AwsProxy] {
            let case = row.case;
            let emulator = emulator();
            let (api_id, endpoint_host) =
                deploy(&emulator, case, row.cors.map(|cors| cors()), backend).await;

            // The api's own `ApiEndpoint` host, the addressing `Api$ApiEndpoint` documents.
            let answer = invoke(
                &emulator,
                &endpoint_host,
                row.method,
                "/items",
                row.request_headers,
            )
            .await;
            decorated(case, backend, row, &answer);

            // The same request at the emulator's own endpoint, with the api id carried in
            // the path: the same records decide it, so the same decoration is answered.
            let answer = invoke(
                &emulator,
                "127.0.0.1:9297",
                row.method,
                &format!("/_aws/execute-api/{api_id}/items"),
                row.request_headers,
            )
            .await;
            decorated(case, backend, row, &answer);
        }
    }
}

/// `Cors$MaxAge` is an integer, and whatever it holds is the number of seconds the
/// preflight's answer reports: the modelled bounds are stored and reported, and so is a
/// value outside either of them, since the service enforces neither.
#[tokio::test]
async fn the_configured_max_age_is_what_the_preflight_reports() {
    for max_age in [
        MODELLED_MIN_MAX_AGE - 1,
        MODELLED_MIN_MAX_AGE,
        0,
        600,
        MODELLED_MAX_MAX_AGE,
        MODELLED_MAX_MAX_AGE + 1,
    ] {
        let case = format!("MaxAge {max_age}");
        let emulator = emulator();
        let (_api_id, endpoint_host) = deploy(
            &emulator,
            &case,
            Some(json!({
                "allowOrigins": ["https://example.test"],
                "allowMethods": ["GET"],
                "maxAge": max_age
            })),
            Backend::AwsProxy,
        )
        .await;
        let answer = invoke(
            &emulator,
            &endpoint_host,
            "OPTIONS",
            "/items",
            &[
                ("origin", "https://example.test"),
                ("access-control-request-method", "GET"),
            ],
        )
        .await;
        assert_eq!(
            answer.cors.get("access-control-max-age").map(String::as_str),
            Some(max_age.to_string().as_str()),
            "case {case:?}: expected the configured max age, got {}",
            answer.summary()
        );
    }
}

/// A preflight is a READ of the api's records: repeating it answers the same decoration, and
/// `GetApi` reports the same configuration afterwards — nothing about the api moved because a
/// browser asked.
#[tokio::test]
async fn a_preflight_changes_nothing_a_later_caller_sees() {
    let case = "a preflight repeated against one deployed stage";
    let emulator = emulator();
    let (api_id, endpoint_host) = deploy(
        &emulator,
        case,
        Some(every_member()),
        Backend::AwsProxy,
    )
    .await;

    let (status, before) = call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetApi must answer 200, got {status} {before}"
    );

    let mut first: Option<BTreeMap<String, String>> = None;
    for attempt in 1..=3 {
        let answer = invoke(
            &emulator,
            &endpoint_host,
            "OPTIONS",
            "/items",
            &[
                ("origin", "https://example.test"),
                ("access-control-request-method", "GET"),
            ],
        )
        .await;
        match &first {
            None => first = Some(answer.cors.clone()),
            Some(expected) => assert_eq!(
                &answer.cors,
                expected,
                "case {case:?}: preflight {attempt} answered a different decoration, got {}",
                answer.summary()
            ),
        }
        let (status, after) = call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetApi must answer 200 after preflight {attempt}, got {status} {after}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: preflight {attempt} changed the api GetApi reports"
        );
    }
}
