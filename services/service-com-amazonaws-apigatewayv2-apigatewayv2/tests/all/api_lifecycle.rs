// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `Api`/`Route` lifecycle.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly.
//!
//! The tables below span the class each rule generalises over: the protocol type, which
//! optional members a request carries, route-key form, page size, and resource count.
//! The lifecycle the issue cites is ONE row of each.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test requests are signed for. Deliberately NOT the emulator's
/// `us-east-1` default, so an `ApiEndpoint` built from a hardcoded region fails.
pub(crate) const REGION: &str = "eu-west-1";

/// How many characters the id of a resource the ACCOUNT holds carries (and of the pages
/// and mappings nested in one). `Id` is an unconstrained string in the model, so the
/// length of a minted id is this service's own convention per kind of resource.
const ACCOUNT_ID_LEN: usize = 10;

/// How many characters the id of a resource nested inside an api carries: the short
/// opaque form API Gateway v2 answers for an authorizer, a model, a deployment, a route
/// response and an integration response.
const NESTED_ID_LEN: usize = 6;

/// How many characters a `RouteId` and an `IntegrationId` carry: one more than
/// [`NESTED_ID_LEN`].
const ROUTE_ID_LEN: usize = 7;

/// A fresh emulator holding only this service, with request authentication off.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(engine),
    );
    router
}

/// A fresh emulator holding this service AND the API Gateway (v1) service that owns REST
/// APIs — the arrangement the router assembles.
///
/// A suite that writes a `ProductRestEndpointPage` needs it: the page documents one REST
/// endpoint, and a write naming an endpoint the account does not have is refused, so the
/// endpoint has to be created through the service that OWNS it ([`rest_endpoint`]).
pub(crate) fn emulator_with_rest_apis() -> MultiServiceRouter {
    emulator_with_rest_apis_and_engine().0
}

/// [`emulator_with_rest_apis`] and the engine both services were built from, for a suite
/// that asserts on the store itself — what a refused write must NOT have persisted.
pub(crate) fn emulator_with_rest_apis_and_engine(
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// Create one REST API with `stage` deployed on it, through the service that OWNS REST APIs,
/// and answer the `RestApiId` that service MINTED.
///
/// A stage serves a DEPLOYMENT, and a deployment is the snapshot of the API's methods, so
/// the API gets one method on its root resource first — the shortest path a client takes to
/// a stage it can invoke. What a suite calls "an endpoint the account holds" is therefore
/// what that service reports, not an identifier a test made up.
pub(crate) async fn rest_endpoint(
    emulator: &MultiServiceRouter,
    name: &str,
    stage: &str,
) -> String {
    let (status, created) =
        call(emulator, "POST", "/restapis", Some(json!({ "name": name }))).await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected 201 from CreateRestApi, got {status} {created}"
    );
    let rest_api_id = as_str(&created, "id");
    let root = as_str(&created, "rootResourceId");

    let (status, method) = call(
        emulator,
        "PUT",
        &format!("/restapis/{rest_api_id}/resources/{root}/methods/GET"),
        Some(json!({ "authorizationType": "NONE" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected 201 from PutMethod, got {status} {method}"
    );

    let (status, deployment) = call(
        emulator,
        "POST",
        &format!("/restapis/{rest_api_id}/deployments"),
        Some(json!({ "stageName": stage })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected 201 from CreateDeployment, got {status} {deployment}"
    );
    rest_api_id
}

/// Send one request, signed for [`REGION`], and return `(status, parsed body)`. An
/// empty body parses to `Value::Null`, which is how a 204 is asserted.
pub(crate) async fn call(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_region(emulator, REGION, method, uri, body).await
}

/// Send one request signed for `region` and return `(status, parsed body)`.
///
/// The SigV4 credential scope is what routes the request to this service and what
/// names the region its endpoints are reported in, so a suite that asserts on a
/// region-bearing value drives this directly; [`call`] is this with [`REGION`].
pub(crate) async fn call_in_region(
    emulator: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_at_host(
        emulator,
        &format!("apigateway.{region}.amazonaws.com"),
        region,
        method,
        uri,
        body,
    )
    .await
}

/// [`call_in_region`] addressed at `host`: the endpoint the request is SENT to, which the
/// credential scope is independent of.
///
/// A client configured with one local endpoint URL sends the emulator's own address as the
/// `Host` for every service, and a client that resolves AWS's own hosts sends this
/// service's. Both route here through the credential scope, and a value this service
/// ANNOUNCES for a client to fetch follows the endpoint the request came in on — see
/// `state::announced_portal_default_domain_name` — so a suite that asserts on such a value
/// drives this directly.
pub(crate) async fn call_at_host(
    emulator: &MultiServiceRouter,
    host: &str,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let (status, _headers, parsed) =
        call_at_host_reading_headers(emulator, host, region, method, uri, body).await;
    (status, parsed)
}

/// [`call_at_host`] keeping the response HEADERS as well, for a case that asserts on how an
/// answer is FRAMED (the length it states) and not only on its status and body.
pub(crate) async fn call_at_host_reading_headers(
    emulator: &MultiServiceRouter,
    host: &str,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, http::HeaderMap, Value) {
    let payload = match &body {
        Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize request body")),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/{region}/apigateway/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");

    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, headers, parsed)
}

/// [`call`] keeping the response headers: `(status, headers, parsed body)`.
pub(crate) async fn call_reading_headers(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, http::HeaderMap, Value) {
    call_at_host_reading_headers(
        emulator,
        &format!("apigateway.{REGION}.amazonaws.com"),
        REGION,
        method,
        uri,
        body,
    )
    .await
}

/// The `__type` / error code a failure response carries.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn as_str(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One row of the CreateApi table.
struct ApiCase {
    label: &'static str,
    request: Value,
    /// The `RouteSelectionExpression` the stored record must carry.
    expect_route_selection: &'static str,
}

fn api_cases() -> Vec<ApiCase> {
    vec![
        ApiCase {
            label: "http, only the required members",
            request: json!({ "name": "minimal-http", "protocolType": "HTTP" }),
            // Defaulted: "If not provided, this will be the default for HTTP APIs".
            expect_route_selection: "$request.method $request.path",
        },
        ApiCase {
            label: "http, every optional member the slice stores",
            request: json!({
                "name": "fully-specified",
                "protocolType": "HTTP",
                "description": "an api with every optional member set",
                "version": "2024-01-01",
                "disableExecuteApiEndpoint": true,
                "ipAddressType": "dualstack",
                "corsConfiguration": {
                    "allowOrigins": ["https://example.com", "https://other.example.com"],
                    "allowMethods": ["GET", "POST"],
                    "allowHeaders": ["content-type"],
                    "exposeHeaders": ["x-trace"],
                    "allowCredentials": true,
                    "maxAge": 600
                },
                "tags": { "team": "platform", "env": "test" }
            }),
            expect_route_selection: "$request.method $request.path",
        },
        ApiCase {
            label: "http, explicit route selection expression",
            request: json!({
                "name": "explicit-selection",
                "protocolType": "HTTP",
                "routeSelectionExpression": "$request.method $request.path"
            }),
            expect_route_selection: "$request.method $request.path",
        },
        ApiCase {
            label: "websocket, route selection expression supplied",
            request: json!({
                "name": "socket-api",
                "protocolType": "WEBSOCKET",
                "routeSelectionExpression": "$request.body.action",
                "apiKeySelectionExpression": "$request.querystring.apiKey",
                "disableSchemaValidation": true
            }),
            expect_route_selection: "$request.body.action",
        },
        ApiCase {
            label: "websocket, one-character name (the low end of the 1..128 length)",
            request: json!({
                "name": "a",
                "protocolType": "WEBSOCKET",
                "routeSelectionExpression": "$request.body.action"
            }),
            expect_route_selection: "$request.body.action",
        },
    ]
}

/// How far `CreatedDate` may sit from the test's own clock reading. A few seconds is
/// plenty of slack for a CI scheduling hiccup while still pinning the instant.
const CREATED_DATE_SLACK_SECONDS: f64 = 10.0;

/// `CreatedDate` must be the instant the api was created.
///
/// The ENCODING is part of the contract: every timestamp member of this service targets
/// `__timestampIso8601` (`@timestampFormat("date-time")`), so the value is an RFC 3339
/// string and never the protocol's default epoch-seconds number. [`instant`] is what
/// pins that; this asserts the INSTANT, which is what the handler decides.
pub(crate) fn assert_created_now(case: &str, body: &Value) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default();
    let seconds = instant(case, body, "createdDate");
    assert!(
        (now - seconds).abs() <= CREATED_DATE_SLACK_SECONDS,
        "case {case:?}: CreatedDate must be the creation instant; got {seconds} against a clock reading {now}"
    );
}

/// The instant a timestamp member reports, in epoch seconds — the one decoder every
/// suite reads a timestamp through, so two of them cannot drift.
///
/// Requires the RFC 3339 / ISO-8601 date-time STRING the `date-time` format on every one
/// of this service's timestamp targets makes part of the wire contract: a JSON number is
/// the protocol default no generated client can deserialize into a `date-time`, so it
/// fails here. Panics when the member is absent as well: a response that must carry an
/// instant and does not is a failure, not a zero. `tests/all/iso8601_timestamp_members.rs`
/// states the rule in full.
pub(crate) fn instant(case: &str, body: &Value, member: &str) -> f64 {
    match body.get(member) {
        Some(Value::String(rendered)) => ::aws_smithy_types::DateTime::from_str(
            rendered,
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {member} {rendered:?} is not ISO-8601: {error}")
        })
        .as_secs_f64(),
        other => panic!(
            "case {case:?}: the response must report {member} as an RFC 3339 date-time string, \
             got {other:?}"
        ),
    }
}

/// Whether `member` carries nothing: absent, null, or an empty collection — the one
/// reading every suite asserts "the request named none of this, so the record carries
/// nothing for it" through, so two of them cannot drift.
pub(crate) fn carries_nothing(body: &Value, member: &str) -> bool {
    body.get(member).is_none_or(|value| match value {
        Value::Null => true,
        Value::Object(map) => map.is_empty(),
        Value::Array(items) => items.is_empty(),
        _ => false,
    })
}

/// How long a wait for a CONDITION may take before the test gives up: far beyond any span
/// this emulator owes a transition, so a loaded host reaches the same outcome as an idle
/// one. It is not a budget any case tests — only the condition is asserted on.
const CONDITION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// Read `uri` until `settled` holds of the answer, and answer that reading.
///
/// The wait is for the CONDITION — the record is re-read until it reports what the case
/// waits for — never for a fixed time, so neither a fast nor a loaded host changes what a
/// case observes. Between readings the runtime is yielded to, so the transition the clock
/// owes can come due; the deadline is [`CONDITION_DEADLINE`].
pub(crate) async fn read_until(
    emulator: &MultiServiceRouter,
    case: &str,
    uri: &str,
    settled: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = std::time::Instant::now() + CONDITION_DEADLINE;
    loop {
        let (status, body) = call(emulator, "GET", uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GET {uri}, got {status} {body}"
        );
        if settled(&body) {
            return body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: {uri} never reported what the case waits for, last read {body}"
        );
        tokio::task::yield_now().await;
    }
}

/// Wait until this test's clock reads a LATER whole second than `seconds`.
///
/// The condition a case needs when it asserts that a stored instant did NOT move: the
/// service reports a timestamp at whole-second granularity, so only a reading in a later
/// second could show a move at all. The wait is for that reading, not for a fixed time, and
/// gives up at [`CONDITION_DEADLINE`].
pub(crate) async fn past_the_whole_second_of(case: &str, seconds: f64) {
    let deadline = std::time::Instant::now() + CONDITION_DEADLINE;
    while now_epoch_seconds().floor() <= seconds.floor() {
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the clock never reached a second later than {seconds}"
        );
        tokio::task::yield_now().await;
    }
}

/// This test's clock reading, in epoch seconds.
pub(crate) fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// `value` with every null object entry dropped, recursively.
///
/// The ONE normalizer every suite compares a request against a reader's answer through,
/// so a structure member a reader omits and one it reports as null compare equal rather
/// than being asserted as two different things.
pub(crate) fn strip_nulls(value: &Value) -> Value {
    match value {
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .filter(|(_, entry)| !entry.is_null())
                .map(|(key, entry)| (key.clone(), strip_nulls(entry)))
                .collect::<serde_json::Map<String, Value>>(),
        ),
        Value::Array(entries) => Value::Array(entries.iter().map(strip_nulls).collect()),
        other => other.clone(),
    }
}

/// How many characters the id this service mints for `kind` carries.
///
/// `kind` is the member a response reports the id under (`authorizerId`, `RouteId`) or the
/// resource kind itself, in any case. An unknown kind panics rather than being waved
/// through on a default, so a member added here without a length is a failure.
pub(crate) fn minted_id_len(kind: &str) -> usize {
    let lowered = kind.to_ascii_lowercase();
    match lowered.strip_suffix("id").unwrap_or(&lowered) {
        "api" | "portal" | "portalproduct" | "productpage" | "productrestendpointpage"
        | "vpclink" | "domainname" | "apimapping" | "routingrule" => ACCOUNT_ID_LEN,
        "route" | "integration" => ROUTE_ID_LEN,
        "model" | "deployment" | "authorizer" | "routeresponse" | "integrationresponse" => {
            NESTED_ID_LEN
        }
        other => panic!("this suite knows no minted-id length for {other:?}"),
    }
}

/// A minted id is lowercase alphanumerics, as many of them as its kind's convention gives
/// it ([`minted_id_len`]).
pub(crate) fn assert_minted_id(case: &str, member: &str, id: &str) {
    let expected = minted_id_len(member);
    assert_eq!(
        id.len(),
        expected,
        "case {case:?}: expected a {expected}-character {member}, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()),
        "case {case:?}: expected a lowercase-alphanumeric {member}, got {id:?}"
    );
}

#[tokio::test]
async fn create_api_persists_the_record_get_api_reports_back() {
    let emulator = emulator();

    for case in api_cases() {
        let (status, created) = call(&emulator, "POST", "/v2/apis", Some(case.request.clone())).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateApi, got {status} {created}",
            case.label
        );

        let api_id = as_str(&created, "apiId");
        assert_minted_id(case.label, "ApiId", &api_id);

        // The endpoint is the address the api is reached at, with the scheme its protocol
        // type names (see `api_endpoint_scheme.rs` for the table over that).
        crate::api_endpoint_scheme::assert_announced_endpoint(
            case.label,
            "CreateApi",
            &as_str(&created, "protocolType"),
            &api_id,
            REGION,
            &as_str(&created, "apiEndpoint"),
        );
        assert_eq!(
            as_str(&created, "routeSelectionExpression"),
            case.expect_route_selection,
            "case {:?}: wrong RouteSelectionExpression, got {created}",
            case.label
        );
        assert_eq!(
            as_str(&created, "name"),
            as_str(&case.request, "name"),
            "case {:?}: CreateApi must report the stored Name, got {created}",
            case.label
        );
        assert_eq!(
            as_str(&created, "protocolType"),
            as_str(&case.request, "protocolType"),
            "case {:?}: CreateApi must report the stored ProtocolType, got {created}",
            case.label
        );
        assert_created_now(case.label, &created);

        // The whole record must come back from the reader, field for field.
        let (status, fetched) =
            call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetApi, got {status} {fetched}",
            case.label
        );
        assert_eq!(
            fetched, created,
            "case {:?}: GetApi must report the record CreateApi stored",
            case.label
        );

        // A read must not change what a later caller sees.
        let (_, refetched) = call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            refetched, fetched,
            "case {:?}: GetApi changed the record it reported",
            case.label
        );
    }
}

#[tokio::test]
async fn minted_api_ids_are_distinct() {
    let emulator = emulator();
    let mut ids = BTreeSet::new();

    for n in 0..24 {
        let label = format!("api #{n}");
        let (status, created) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(json!({ "name": format!("api-{n}"), "protocolType": "HTTP" })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {label:?}: expected 201 from CreateApi, got {status} {created}"
        );
        let api_id = as_str(&created, "apiId");
        assert_minted_id(&label, "ApiId", &api_id);
        assert!(
            ids.insert(api_id.clone()),
            "case {label:?}: ApiId {api_id:?} was minted twice"
        );
    }
}

#[tokio::test]
async fn create_api_refuses_a_request_the_protocol_type_does_not_allow() {
    let emulator = emulator();

    // Each row is refused by a rule the model's own docs state, with the member that
    // breaks it. `RouteSelectionExpression` "is required for WebSocket APIs";
    // `CorsConfiguration` is "Supported only for HTTP APIs"; `RouteKey` "is part of quick
    // create", which needs a `Target`.
    //
    // A quick-create `Target` on a WEBSOCKET api is NOT here: API Gateway accepts it and
    // quick-creates nothing for such an api, which `quick_create_is_http_only` holds.
    let cases = [
        (
            "websocket without RouteSelectionExpression",
            json!({ "name": "no-selection", "protocolType": "WEBSOCKET" }),
        ),
        (
            "websocket with a CorsConfiguration",
            json!({
                "name": "socket-cors",
                "protocolType": "WEBSOCKET",
                "routeSelectionExpression": "$request.body.action",
                "corsConfiguration": { "allowOrigins": ["https://example.com"] }
            }),
        ),
        (
            "http RouteKey without a Target",
            json!({
                "name": "route-key-only",
                "protocolType": "HTTP",
                "routeKey": "GET /items"
            }),
        ),
    ];

    for (label, request) in cases {
        let (status, body) = call(&emulator, "POST", "/v2/apis", Some(request)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from CreateApi, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {label:?}: expected BadRequestException, got {body}"
        );
    }

    // And nothing was persisted by any refused request.
    let (status, listed) = call(&emulator, "GET", "/v2/apis", None).await;
    assert_eq!(status, 200, "expected 200 from GetApis, got {status} {listed}");
    assert_eq!(
        listed["items"].as_array().map(Vec::len),
        Some(0),
        "a refused CreateApi must persist nothing, got {listed}"
    );
}

#[tokio::test]
async fn quick_create_mints_the_catch_all_route_with_the_api() {
    let emulator = emulator();

    // `RouteKey` absent -> "$default"; `RouteKey` given -> that key. Both need a Target,
    // and either route is the one quick create minted with the api, so API Gateway
    // manages it (`Route$ApiGatewayManaged`).
    let cases = [
        ("default route key", None, "$default"),
        ("explicit route key", Some("GET /items"), "GET /items"),
    ];

    for (label, route_key, expect_key) in cases {
        let mut request = json!({
            "name": format!("quick-create-{expect_key}"),
            "protocolType": "HTTP",
            "target": "https://example.com/upstream"
        });
        if let Some(key) = route_key {
            request["routeKey"] = json!(key);
        }

        let (status, created) = call(&emulator, "POST", "/v2/apis", Some(request)).await;
        assert_eq!(
            status, 201,
            "case {label:?}: expected 201 from CreateApi, got {status} {created}"
        );
        let api_id = as_str(&created, "apiId");

        let (status, routes) =
            call(&emulator, "GET", &format!("/v2/apis/{api_id}/routes"), None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetRoutes, got {status} {routes}"
        );
        let items = routes["items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            items.len(),
            1,
            "case {label:?}: quick create must store exactly one route, got {routes}"
        );
        assert_eq!(
            as_str(&items[0], "routeKey"),
            expect_key,
            "case {label:?}: wrong RouteKey on the quick-create route, got {routes}"
        );
        // The route is wired to the integration quick create minted from the `Target`
        // ("Quick create produces an API with an integration, a default catch-all
        // route, …", `CreateApi$Target`), which is the resource that carries the
        // endpoint address; `integrations/{IntegrationId}` is the form a route's target
        // takes. The whole quick-create set is `quick_create_servable_set`'s rule.
        let target = as_str(&items[0], "target");
        let integration_id = target.strip_prefix("integrations/").unwrap_or_else(|| {
            panic!("case {label:?}: the quick-create route must target an integration, got {routes}")
        });
        let (status, integration) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations/{integration_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetIntegration for {target:?}, got {status} {integration}"
        );
        assert_eq!(
            as_str(&integration, "integrationUri"),
            "https://example.com/upstream",
            "case {label:?}: the quick-create integration must carry the Target, got {integration}"
        );
        assert_eq!(
            items[0]["apiGatewayManaged"].as_bool(),
            Some(true),
            "case {label:?}: the route quick create mints with the api is \
             API-Gateway-managed, got {routes}"
        );
    }
}

/// One row of the CreateRoute table.
struct RouteCase {
    label: &'static str,
    request: Value,
}

fn route_cases(integration_id: &str, authorizer_id: &str) -> Vec<RouteCase> {
    vec![
        RouteCase {
            label: "method and path",
            request: json!({ "routeKey": "GET /items" }),
        },
        RouteCase {
            label: "greedy path variable",
            // A target addresses an integration of the route's OWN api, so the id is one
            // this api minted.
            request: json!({
                "routeKey": "ANY /{proxy+}",
                "target": format!("integrations/{integration_id}")
            }),
        },
        RouteCase {
            label: "catch-all",
            request: json!({ "routeKey": "$default", "operationName": "catchAll" }),
        },
        RouteCase {
            label: "websocket connect key with every optional member",
            request: json!({
                "routeKey": "$connect",
                "apiKeyRequired": true,
                "authorizationType": "AWS_IAM",
                "authorizationScopes": ["scope/one", "scope/two"],
                "operationName": "connect",
                "modelSelectionExpression": "$request.body.action",
                "routeResponseSelectionExpression": "$default",
                "requestModels": { "default": "Empty" },
                "requestParameters": { "route.request.header.x-trace": { "required": true } }
            }),
        },
        RouteCase {
            label: "path with a single variable",
            // `CUSTOM` is the authorizer-backed type a WebSocket api's route may carry
            // ("For WebSocket APIs, valid values are NONE …, AWS_IAM … and CUSTOM"), and
            // the id names the authorizer this api holds.
            request: json!({
                "routeKey": "DELETE /items/{id}",
                "authorizationType": "CUSTOM",
                "authorizerId": authorizer_id
            }),
        },
    ]
}

#[tokio::test]
async fn create_route_persists_the_record_get_route_reports_back() {
    let emulator = emulator();
    let (_, api) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "route-host", "protocolType": "WEBSOCKET", "routeSelectionExpression": "$request.body.action" })),
    )
    .await;
    let api_id = as_str(&api, "apiId");

    // The two resources a route of this api can reference: an integration for a
    // `Target` and an authorizer for an `AuthorizerId`.
    let (status, integration) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({ "integrationType": "MOCK" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateIntegration, got {status} {integration}"
    );
    let integration_id = as_str(&integration, "integrationId");
    let (status, authorizer) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(json!({
            "name": "route-authorizer",
            "authorizerType": "REQUEST",
            "identitySource": ["$request.header.Authorization"],
            "authorizerUri": "arn:aws:apigateway:eu-west-1:lambda:path/2015-03-31/functions/arn:aws:lambda:eu-west-1:000000000000:function:route-authorizer/invocations"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateAuthorizer, got {status} {authorizer}"
    );
    let authorizer_id = as_str(&authorizer, "authorizerId");

    let mut stored_ids = BTreeSet::new();
    for case in route_cases(&integration_id, &authorizer_id) {
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(case.request.clone()),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateRoute, got {status} {created}",
            case.label
        );

        let route_id = as_str(&created, "routeId");
        assert_minted_id(case.label, "RouteId", &route_id);
        assert!(
            stored_ids.insert(route_id.clone()),
            "case {:?}: RouteId {route_id:?} was minted twice",
            case.label
        );
        assert_eq!(
            as_str(&created, "routeKey"),
            as_str(&case.request, "routeKey"),
            "case {:?}: CreateRoute must report the stored RouteKey, got {created}",
            case.label
        );
        // "valid values are NONE for open access" — a route that named no
        // authorization type is open.
        let expected_authorization = case
            .request
            .get("authorizationType")
            .and_then(Value::as_str)
            .unwrap_or("NONE");
        assert_eq!(
            as_str(&created, "authorizationType"),
            expected_authorization,
            "case {:?}: wrong AuthorizationType, got {created}",
            case.label
        );

        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/routes/{route_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetRoute, got {status} {fetched}",
            case.label
        );
        assert_eq!(
            fetched, created,
            "case {:?}: GetRoute must report the record CreateRoute stored",
            case.label
        );
    }

    // Every stored route, and only those, is listed for its parent api.
    let (status, routes) = call(&emulator, "GET", &format!("/v2/apis/{api_id}/routes"), None).await;
    assert_eq!(status, 200, "expected 200 from GetRoutes, got {status} {routes}");
    let listed: BTreeSet<String> = routes["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|route| as_str(route, "routeId"))
        .collect();
    assert_eq!(
        listed, stored_ids,
        "GetRoutes must list exactly the routes stored for the api, got {routes}"
    );
}

#[tokio::test]
async fn a_duplicate_route_key_conflicts_within_one_api_but_not_across_apis() {
    let emulator = emulator();
    let mut api_ids = Vec::new();
    for n in 0..2 {
        let (_, api) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(json!({ "name": format!("conflict-host-{n}"), "protocolType": "HTTP" })),
        )
        .await;
        api_ids.push(as_str(&api, "apiId"));
    }

    for route_key in ["GET /items", "$default", "ANY /{proxy+}"] {
        // The same key is accepted once per api...
        for api_id in &api_ids {
            let (status, body) = call(
                &emulator,
                "POST",
                &format!("/v2/apis/{api_id}/routes"),
                Some(json!({ "routeKey": route_key })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {route_key:?} on {api_id}: a route key is unique per api, not globally; got {status} {body}"
            );
        }
        // ...and refused the second time on the same api.
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{}/routes", api_ids[0]),
            Some(json!({ "routeKey": route_key })),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {route_key:?}: expected 409 for a duplicate route key, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {route_key:?}: expected ConflictException, got {body}"
        );
    }
}

#[tokio::test]
async fn an_unknown_api_or_route_is_not_found() {
    let emulator = emulator();
    let (_, api) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "lookup-host", "protocolType": "HTTP" })),
    )
    .await;
    let api_id = as_str(&api, "apiId");
    let (_, route) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({ "routeKey": "GET /items" })),
    )
    .await;
    let route_id = as_str(&route, "routeId");

    // Each row names a resource the store does not hold, reached through a different
    // operation; all of them answer 404 NotFoundException.
    let cases: Vec<(&str, &str, &str, Option<Value>)> = vec![
        ("GetApi on an unknown api", "GET", "/v2/apis/nosuchapi0", None),
        (
            "GetRoutes on an unknown api",
            "GET",
            "/v2/apis/nosuchapi0/routes",
            None,
        ),
        (
            "GetRoute on an unknown api",
            "GET",
            "/v2/apis/nosuchapi0/routes/nosuchrte0",
            None,
        ),
        (
            "DeleteApi on an unknown api",
            "DELETE",
            "/v2/apis/nosuchapi0",
            None,
        ),
        (
            "CreateRoute on an unknown api",
            "POST",
            "/v2/apis/nosuchapi0/routes",
            Some(json!({ "routeKey": "GET /items" })),
        ),
    ];
    for (label, method, uri, body) in cases {
        let (status, response) = call(&emulator, method, uri, body).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "NotFoundException",
            "case {label:?}: expected NotFoundException, got {response}"
        );
    }

    // An unknown route id under a KNOWN api is a 404 too; the known one is not.
    let (status, response) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/routes/nosuchrte0"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"GetRoute on an unknown route of a known api\": expected 404, got {status} {response}"
    );
    let (status, response) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/routes/{route_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"GetRoute on the stored route\": expected 200, got {status} {response}"
    );
}

#[tokio::test]
async fn delete_api_removes_the_api_and_cascades_to_its_routes() {
    let emulator = emulator();
    let (_, keep) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "survivor", "protocolType": "HTTP" })),
    )
    .await;
    let keep_id = as_str(&keep, "apiId");

    let (_, doomed) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "doomed", "protocolType": "HTTP" })),
    )
    .await;
    let doomed_id = as_str(&doomed, "apiId");
    for route_key in ["GET /items", "POST /items", "$default"] {
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{doomed_id}/routes"),
            Some(json!({ "routeKey": route_key })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {route_key:?}: expected 201 from CreateRoute, got {status} {body}"
        );
    }

    let (status, body) = call(&emulator, "DELETE", &format!("/v2/apis/{doomed_id}"), None).await;
    assert_eq!(
        status, 204,
        "case \"DeleteApi\": expected 204, got {status} {body}"
    );
    assert_eq!(
        body,
        Value::Null,
        "case \"DeleteApi\": expected an empty body, got {body}"
    );

    // Gone from the reader, from the listing, and from its children.
    for (label, method, uri, body) in [
        (
            "GetApi after delete",
            "GET",
            format!("/v2/apis/{doomed_id}"),
            None,
        ),
        (
            "GetRoutes after delete",
            "GET",
            format!("/v2/apis/{doomed_id}/routes"),
            None,
        ),
        (
            "CreateRoute after delete",
            "POST",
            format!("/v2/apis/{doomed_id}/routes"),
            Some(json!({ "routeKey": "GET /late" })),
        ),
        (
            "DeleteApi twice",
            "DELETE",
            format!("/v2/apis/{doomed_id}"),
            None,
        ),
    ] {
        let (status, response) = call(&emulator, method, &uri, body).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 after the api was deleted, got {status} {response}"
        );
    }

    let (status, listed) = call(&emulator, "GET", "/v2/apis", None).await;
    assert_eq!(status, 200, "expected 200 from GetApis, got {status} {listed}");
    let ids: Vec<String> = listed["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|api| as_str(api, "apiId"))
        .collect();
    assert_eq!(
        ids,
        vec![keep_id.clone()],
        "GetApis must list the surviving api and not the deleted one, got {listed}"
    );
}

#[tokio::test]
async fn get_apis_pages_every_api_exactly_once_at_every_page_size() {
    // The rule generalises over page size and over how many resources exist, so the
    // table walks both: each count is paged at every size from 1 up to one past it.
    for total in [0_usize, 1, 2, 5] {
        let emulator = emulator();
        let mut expected = BTreeSet::new();
        for n in 0..total {
            let (status, created) = call(
                &emulator,
                "POST",
                "/v2/apis",
                Some(json!({ "name": format!("paged-{n}"), "protocolType": "HTTP" })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case total={total}: expected 201 from CreateApi, got {status} {created}"
            );
            expected.insert(as_str(&created, "apiId"));
        }

        for page_size in 1..=total + 1 {
            let label = format!("total={total}, maxResults={page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            // One extra iteration than pages are possible, so a token that never
            // clears is caught instead of looping forever.
            for _ in 0..total + 2 {
                let uri = match &token {
                    Some(t) => format!("/v2/apis?maxResults={page_size}&nextToken={t}"),
                    None => format!("/v2/apis?maxResults={page_size}"),
                };
                let (status, body) = call(&emulator, "GET", &uri, None).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: expected 200 from GetApis, got {status} {body}"
                );
                let items = body["items"].as_array().cloned().unwrap_or_default();
                assert!(
                    items.len() <= page_size,
                    "case {label:?}: a page must hold at most MaxResults items, got {body}"
                );
                seen.extend(items.iter().map(|api| as_str(api, "apiId")));
                token = body
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                if token.is_none() {
                    break;
                }
            }
            assert!(
                token.is_none(),
                "case {label:?}: paging did not terminate; last NextToken was {token:?}"
            );
            assert_eq!(
                seen.len(),
                expected.len(),
                "case {label:?}: every api must be reported exactly once, got {seen:?}"
            );
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<_>>(),
                expected,
                "case {label:?}: paging reported the wrong set, got {seen:?}"
            );
            let mut sorted = seen.clone();
            sorted.sort();
            assert_eq!(
                seen, sorted,
                "case {label:?}: paging must walk a stable ApiId order, got {seen:?}"
            );
        }

        // No MaxResults at all reports the whole collection in one page.
        let (status, body) = call(&emulator, "GET", "/v2/apis", None).await;
        assert_eq!(
            status, 200,
            "case total={total}: expected 200 from GetApis, got {status} {body}"
        );
        assert_eq!(
            body["items"].as_array().map(Vec::len),
            Some(total),
            "case total={total}: an unpaged GetApis must report every api, got {body}"
        );
        assert!(
            body.get("nextToken").is_none() || body["nextToken"].is_null(),
            "case total={total}: an unpaged GetApis has nothing left to report, got {body}"
        );
    }
}

#[tokio::test]
async fn get_routes_pages_every_route_exactly_once_at_every_page_size() {
    let emulator = emulator();
    let (_, api) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "paged-routes", "protocolType": "HTTP" })),
    )
    .await;
    let api_id = as_str(&api, "apiId");

    let mut expected = BTreeSet::new();
    for n in 0..4 {
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({ "routeKey": format!("GET /items/{n}") })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case route #{n}: expected 201 from CreateRoute, got {status} {created}"
        );
        expected.insert(as_str(&created, "routeId"));
    }

    for page_size in 1..=5 {
        let label = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..6 {
            let uri = match &token {
                Some(t) => format!("/v2/apis/{api_id}/routes?maxResults={page_size}&nextToken={t}"),
                None => format!("/v2/apis/{api_id}/routes?maxResults={page_size}"),
            };
            let (status, body) = call(&emulator, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200 from GetRoutes, got {status} {body}"
            );
            let items = body["items"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "case {label:?}: a page must hold at most MaxResults items, got {body}"
            );
            seen.extend(items.iter().map(|route| as_str(route, "routeId")));
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {label:?}: paging did not terminate; last NextToken was {token:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            expected,
            "case {label:?}: paging reported the wrong set of routes, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            expected.len(),
            "case {label:?}: every route must be reported exactly once, got {seen:?}"
        );
    }
}

/// AWS's published API Gateway quotas, which are also the bounds that stop a loop of
/// create calls growing the emulator's store without end. Source: the API Gateway
/// "Quotas for HTTP APIs" page — 600 APIs per account per Region, 300 routes per API.
const MAX_APIS: usize = 600;
const MAX_ROUTES_PER_API: usize = 300;

/// The largest page either paginated operation serves, and the page size used when a
/// request names no `MaxResults`.
const MAX_PAGE_SIZE: usize = 100;

#[tokio::test]
async fn caller_driven_growth_and_page_size_are_bounded() {
    let emulator = emulator();

    // Fill the api store to its quota; every create up to the bound succeeds.
    let mut first_api = String::new();
    for n in 0..MAX_APIS {
        let (status, created) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(json!({ "name": format!("bounded-{n}"), "protocolType": "HTTP" })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case api #{n} of {MAX_APIS}: expected 201 from CreateApi, got {status} {created}"
        );
        if n == 0 {
            first_api = as_str(&created, "apiId");
        }
    }

    // One past the bound is refused, with the modelled limit error.
    let (status, body) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "one-too-many", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 429,
        "case \"api #{MAX_APIS} past the quota\": expected 429, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "case \"api past the quota\": expected TooManyRequestsException, got {body}"
    );

    // A page is bounded server-side whatever the caller asks for: no MaxResults, and a
    // MaxResults far above the maximum, both serve at most MAX_PAGE_SIZE and report the
    // rest through NextToken.
    for (label, uri) in [
        ("no MaxResults", "/v2/apis".to_string()),
        ("MaxResults far above the maximum", "/v2/apis?maxResults=1000000".to_string()),
        ("MaxResults at the maximum", format!("/v2/apis?maxResults={MAX_PAGE_SIZE}")),
    ] {
        let (status, body) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetApis, got {status} {body}"
        );
        assert_eq!(
            body["items"].as_array().map(Vec::len),
            Some(MAX_PAGE_SIZE),
            "case {label:?}: a page must be clamped to {MAX_PAGE_SIZE} items"
        );
        assert!(
            body.get("nextToken").and_then(Value::as_str).is_some(),
            "case {label:?}: a clamped page must report what is left through NextToken"
        );
    }

    // Routes under one api are bounded the same way.
    for n in 0..MAX_ROUTES_PER_API {
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{first_api}/routes"),
            Some(json!({ "routeKey": format!("GET /bounded/{n}") })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case route #{n} of {MAX_ROUTES_PER_API}: expected 201 from CreateRoute, got {status} {created}"
        );
    }
    let (status, body) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{first_api}/routes"),
        Some(json!({ "routeKey": "GET /one-too-many" })),
    )
    .await;
    assert_eq!(
        status, 429,
        "case \"route #{MAX_ROUTES_PER_API} past the quota\": expected 429, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "case \"route past the quota\": expected TooManyRequestsException, got {body}"
    );
}

#[tokio::test]
async fn a_max_results_that_is_not_a_positive_integer_is_refused() {
    let emulator = emulator();
    let (_, api) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "max-results-host", "protocolType": "HTTP" })),
    )
    .await;
    let api_id = as_str(&api, "apiId");

    // `MaxResults` is modelled as `__string`, so the numeric rule is the service's:
    // a positive decimal integer. Each row breaks it in a different way, and both
    // paginated operations answer the same way.
    for raw in ["0", "-1", "abc", "1.5", "%20"] {
        for (label, uri) in [
            ("GetApis", format!("/v2/apis?maxResults={raw}")),
            (
                "GetRoutes",
                format!("/v2/apis/{api_id}/routes?maxResults={raw}"),
            ),
        ] {
            let (status, body) = call(&emulator, "GET", &uri, None).await;
            assert_eq!(
                status, 400,
                "case {label} maxResults={raw:?}: expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {label} maxResults={raw:?}: expected BadRequestException, got {body}"
            );
        }
    }

    // And a well-formed value at the other end of the range is accepted.
    for raw in ["1", "100", "2147483647"] {
        let (status, body) = call(&emulator, "GET", &format!("/v2/apis?maxResults={raw}"), None).await;
        assert_eq!(
            status, 200,
            "case GetApis maxResults={raw:?}: expected 200, got {status} {body}"
        );
    }
}
