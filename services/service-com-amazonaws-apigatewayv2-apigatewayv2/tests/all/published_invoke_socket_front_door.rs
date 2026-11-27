// SPDX-License-Identifier: CC0-1.0

//! What the address an HTTP api PUBLISHES answers, over a real connection to it.
//!
//! RULE: **the value `Api$ApiEndpoint` carries is an address a client DIALS — "the URI of the
//! API … the stage name is typically appended to this URI to form a complete path to a
//! deployed API stage" — and every request made there is answered by the api's own records,
//! read on that connection exactly as they are read at any other address the api is reached
//! at: the route's integration answers a request the api serves, the gateway's own `404`
//! `{"message":"Not Found"}` answers one addressed to an api whose `DisableExecuteApiEndpoint`
//! is set or whose api id resolves to no api at all, and its `429`
//! `{"message":"Too Many Requests"}` answers one the serving stage's `RouteSettings` throttle
//! sheds — each refusal made before the integration is reached, and each lifted the moment the
//! record that caused it is (`UpdateApi` clearing the member, `DeleteRouteSettings` dropping
//! the override).**
//!
//! Why this suite exists beside the in-process ones: the address the control plane publishes is
//! a SOCKET of this service's own ([`crate::service::invoke_plane_socket`]) whenever this
//! emulator is served on a port at all, and a request arriving on it is read, bounded and
//! handed to the router by this service's own endpoint rather than by this emulator's front
//! door. The suites that drive the same records in process
//! (`disabled_execute_api_endpoint`, `http_api_throttle_burst`,
//! `http_api_published_endpoint_throttled_stage`, `http_api_unresolved_endpoint_root`) cannot
//! see what that endpoint decides for itself, so what they pin is asserted here AT the
//! published address, over a real TCP connection, with the function the route fronts actually
//! running.
//!
//! The rows span what each rule generalises over: the form the client builds the invoke URL in
//! (the published endpoint on its own, for the stage that serves the api's root, and that
//! endpoint with the stage name appended), which api of two over the same function is
//! addressed (the one whose member is set, and its twin), both directions of the
//! `DisableExecuteApiEndpoint` transition, a route the stage's `RouteSettings` throttle and its
//! twin on the stage's `DefaultRouteSettings`, and both sides of `DeleteRouteSettings`.

use serde_json::{json, Value};
use smithy_server_runtime::{EndpointUrl, MultiServiceRouter};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::custom_domain_mapping_invoke::ANSWER_DEADLINE;
use crate::http_api_backend_dispatch::{
    control, emulator_with_lambda, gateway_may_invoke, given_ungranted_function,
    lambda_integration,
};

/// The endpoint this emulator is taken to be served at, which is what makes it publish a
/// dialable address for an api's data plane at all — a well-formed authority nothing dials: the
/// socket the api is published on binds a loopback port of its own.
const SERVED_AT: &str = "https://127.0.0.1:9297";

/// The status the route's function answers with, and the body it returns: an answer carrying
/// them is one the FUNCTION produced rather than one the gateway decided.
const SERVED: u16 = 200;
const MARKER: &str = "served-by-the-function";

/// The status and sentence an HTTP api's endpoint answers a request it serves nothing for: an
/// api that turned its default endpoint off, and an api id that resolves to no api.
const NOT_FOUND: u16 = 404;
const NOT_FOUND_MESSAGE: &str = "Not Found";

/// The status and sentence a request the serving stage's route settings shed carries.
const TOO_MANY_REQUESTS: u16 = 429;
const TOO_MANY_REQUESTS_MESSAGE: &str = "Too Many Requests";

/// The reserved stage "that serves an api's root", reached with no stage segment at all as
/// well as with its name spelled out.
const DEFAULT_STAGE: &str = "$default";

/// The handler every api below fronts: it answers the marker, so a 200 carrying it is the
/// function's own answer and a refusal carrying it would be the integration having been
/// reached.
const ANSWERS_THE_MARKER: &str = "def lambda_handler(event, context):\n\
     \x20   return {'statusCode': 200, 'body': 'served-by-the-function'}\n";

/// The Region every control-plane call below is signed for — the one
/// `http_api_backend_dispatch` creates its functions in — which is the Region an api's own
/// `execute-api` ARN names.
const REGION: &str = "us-east-1";

/// The `aws:SourceArn` each api's grant is narrowed to: every stage, method and path of THAT
/// api, which is the form a synthesised app provisions
/// (`arn:aws:execute-api:{region}:{account}:{api-id}/{stage}/{method}/{path}`). The grant is
/// per api, so a row shows the gateway's invocation admitted for the api it was granted on.
fn this_apis_routes(api_id: &str) -> String {
    let account = ::engine::emulator_account_id();
    format!("arn:aws:execute-api:{REGION}:{account}:{api_id}/*/*/*")
}

/// What one request to the published address was answered with: the status, the header fields
/// as received, and the body verbatim.
///
/// `pub(crate)` with [`request`] below, because every suite that drives one of this service's
/// core-owned HTTP addresses reads an answer off the wire the same way.
#[derive(Debug)]
pub(crate) struct Answered {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
}

impl Answered {
    /// The `message` the gateway's own refusals carry, or the empty string for an answer that
    /// is not one of them.
    pub(crate) fn message(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .as_ref()
            .and_then(|body| body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The answer's body as JSON, for a request a function served.
    pub(crate) fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }

    /// The value this answer carries for the field `name`, matched the way a field name is
    /// (case-insensitively, RFC 9110 §5.1), or `None` when it wrote none — the ONE reading of
    /// a field off an answer read on one of these addresses.
    pub(crate) fn field(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Whether the body is the one the route's function returns.
    fn carries_the_functions_body(&self) -> bool {
        self.body == MARKER
    }
}

/// Send one request to a core-owned address of this service over a real connection and read
/// the whole answer.
///
/// Every such address negotiates encryption in band, so a client that opens in the clear is
/// served in the clear — which is what this does, one request per connection
/// (`Connection: close`), so the read ends with the answer. `host` is the `Host` the client
/// sends: the address itself, or the custom DOMAIN NAME a caller pointed at it. `headers` are
/// whatever else the client sends.
///
/// `pub(crate)`: this is the ONE raw HTTP/1.1 client the suites that drive these addresses
/// use, so none of them can read an answer differently from another.
pub(crate) async fn request(
    case: &str,
    authority: &str,
    method: &str,
    path: &str,
    host: &str,
    headers: &[(&str, &str)],
) -> Answered {
    let port = dialable_port(case, authority);
    let mut stream = tokio::time::timeout(
        ANSWER_DEADLINE,
        TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)),
    )
    .await
    .unwrap_or_else(|_| panic!("case {case:?}: {authority} did not accept a connection in time"))
    .unwrap_or_else(|error| panic!("case {case:?}: {authority} refused a connection: {error}"));
    let mut wire = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (name, value) in headers {
        wire.push_str(&format!("{name}: {value}\r\n"));
    }
    wire.push_str("\r\n");
    tokio::time::timeout(ANSWER_DEADLINE, stream.write_all(wire.as_bytes()))
        .await
        .unwrap_or_else(|_| panic!("case {case:?}: the request was not written in time"))
        .unwrap_or_else(|error| panic!("case {case:?}: the request was not written: {error}"));
    let mut raw = Vec::new();
    tokio::time::timeout(ANSWER_DEADLINE, stream.read_to_end(&mut raw))
        .await
        .unwrap_or_else(|_| panic!("case {case:?}: {authority} did not answer in time"))
        .unwrap_or_else(|error| panic!("case {case:?}: the answer was not read: {error}"));
    let answered = String::from_utf8_lossy(&raw).to_string();
    let (head, body) = answered
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("case {case:?}: expected an HTTP answer, got {answered:?}"));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: expected a status line, got {head:?}"));
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    Answered {
        status,
        headers,
        body: body.to_string(),
    }
}

/// The PORT a client dials `authority` on: the port the published authority names, which is
/// the one the socket that api is published on landed on.
///
/// A published authority is the ADDRESS that socket landed on (`127.0.0.1:{port}`,
/// [`announced_parts`]) rather than a name, so a client dials it as it stands and no resolver
/// is involved. The port is read off the authority here because the connection below is opened
/// on the loopback interface the core bound.
///
/// `pub(crate)`: the suites that drive this address read the port off a published authority the
/// same way.
pub(crate) fn dialable_port(case: &str, authority: &str) -> u16 {
    authority
        .rsplit_once(':')
        .and_then(|(_host, port)| port.parse::<u16>().ok())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the published address {authority:?} names no port to dial")
        })
}

/// [`request`], as a plain `GET path` at the published address with nothing else sent.
async fn get(case: &str, authority: &str, path: &str) -> Answered {
    request(case, authority, "GET", path, authority, &[]).await
}

/// This service and AWS Lambda on one engine, with this emulator taken to be SERVED on a port —
/// which is what makes an api publish a socket of its own rather than the endpoint a request
/// arrived at.
///
/// `pub(crate)` with [`member`] and [`announced_parts`] below, because every suite that drives
/// the address an api PUBLISHES stands one up the same way and reads the published value the
/// same way.
pub(crate) fn served_emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut emulator, engine) = emulator_with_lambda();
    emulator.set_url_base(EndpointUrl::parse(SERVED_AT).expect("a well-formed endpoint"));
    (emulator, engine)
}

/// One api of a case: its id, and the authority and base path a client of its published
/// `ApiEndpoint` addresses.
struct Published {
    api_id: String,
    authority: String,
    base: String,
}

impl Published {
    /// The two forms a client builds the invoke URL in for `route_path`: the published endpoint
    /// on its own, which addresses the stage that serves the api's root, and that endpoint with
    /// the stage name appended.
    fn forms(&self, route_path: &str) -> [(&'static str, String); 2] {
        [
            (
                "the published endpoint on its own",
                format!("{}{route_path}", self.base),
            ),
            (
                "the published endpoint with the stage name appended",
                format!("{}/{DEFAULT_STAGE}{route_path}", self.base),
            ),
        ]
    }
}

/// Create an api whose `routes` all front one `AWS_PROXY` integration over `function_arn`, with
/// the `stage` the case states, grant the gateway the invocation of that function for this api
/// alone, and answer the address the api published.
async fn given_api(
    emulator: &MultiServiceRouter,
    case: &str,
    name: &str,
    function_name: &str,
    function_arn: &str,
    routes: &[&str],
    stage: Value,
) -> Published {
    let (status, api) = control(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi answered {status} {api}"
    );
    let api_id = member(case, &api, "apiId");
    let announced = member(case, &api, "apiEndpoint");

    let (status, integration) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(lambda_integration(function_arn, "2.0")),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration answered {status} {integration}"
    );
    let integration_id = member(case, &integration, "integrationId");

    for route_key in routes {
        let (status, route) = control(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({
                "routeKey": route_key,
                "target": format!("integrations/{integration_id}")
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateRoute {route_key:?} answered {status} {route}"
        );
    }

    // The grant the gateway's invocation of the route's function needs, narrowed to THIS api's
    // own `execute-api` ARN — the `AddPermission` a client makes beside each api it fronts a
    // route with.
    gateway_may_invoke(
        emulator,
        case,
        function_name,
        Some(&this_apis_routes(&api_id)),
    )
    .await;

    let (status, created) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(stage),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage answered {status} {created}"
    );

    let (authority, base) = announced_parts(case, &api_id, &announced);
    Published {
        api_id,
        authority,
        base,
    }
}

/// One string member of an answer.
pub(crate) fn member(case: &str, answer: &Value, name: &str) -> String {
    answer
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the answer carries no {name}: {answer}"))
        .to_string()
}

/// The authority and base path a client builds the invoke URL from, asserted to be the address
/// this emulator publishes an api's data plane at: the ADDRESS the socket serving that plane
/// landed on — a loopback address literal carrying its port, which a client dials as given —
/// with the api's own id behind the reserved data-plane path, so appending the stage name and
/// route path to the published value is the complete path to a deployed stage of THIS api.
pub(crate) fn announced_parts(case: &str, api_id: &str, announced: &str) -> (String, String) {
    let rest = announced.strip_prefix("https://").unwrap_or_else(|| {
        panic!("case {case:?}: an HTTP api must announce an https endpoint, got {announced:?}")
    });
    let (authority, base) = match rest.find('/') {
        Some(at) => (rest[..at].to_string(), rest[at..].to_string()),
        None => (rest.to_string(), String::new()),
    };
    let host = authority
        .rsplit_once(':')
        .map_or(authority.as_str(), |(host, _port)| host);
    let address: std::net::IpAddr = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .unwrap_or_else(|_| {
            panic!(
                "case {case:?}: a published endpoint must name the address its socket landed on, \
                 got {announced:?}"
            )
        });
    assert!(
        address.is_loopback(),
        "case {case:?}: expected the interface the core binds its listeners on, got {announced:?}"
    );
    assert!(
        authority.rsplit_once(':').is_some_and(|(_host, port)| port
            .parse::<u16>()
            .is_ok_and(|port| port != 0)),
        "case {case:?}: a served emulator must announce an address a client can dial, got \
         {announced:?}"
    );
    assert_eq!(
        base,
        format!("/_aws/execute-api/{api_id}"),
        "case {case:?}: expected the api named behind the reserved data-plane path, got \
         {announced:?}"
    );
    (authority, base)
}

/// Set `DisableExecuteApiEndpoint` on `api_id` and assert the control plane reports the value
/// back, so each leg starts from a value a client can read.
async fn set_disabled(emulator: &MultiServiceRouter, case: &str, api_id: &str, disabled: bool) {
    let (status, updated) = control(
        emulator,
        "PATCH",
        &format!("/v2/apis/{api_id}"),
        Some(json!({ "disableExecuteApiEndpoint": disabled })),
    )
    .await;
    // WHICH success status this PATCH answers is `mutating_success_status`'s subject, not this
    // suite's: what matters here is that the update succeeded and the member is readable.
    assert!(
        (200..300).contains(&status),
        "case {case:?}: UpdateApi must succeed, got {status} {updated}"
    );
    assert_eq!(
        updated.get("disableExecuteApiEndpoint"),
        Some(&json!(disabled)),
        "case {case:?}: UpdateApi must echo disableExecuteApiEndpoint={disabled}, got {updated}"
    );
    let (status, fetched) = control(emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetApi answered {status} {fetched}"
    );
    assert_eq!(
        fetched.get("disableExecuteApiEndpoint"),
        Some(&json!(disabled)),
        "case {case:?}: GetApi must report disableExecuteApiEndpoint={disabled}, got {fetched}"
    );
}

/// RULE (the disabled endpoint): at the address it published, an api whose
/// `DisableExecuteApiEndpoint` is set answers `404` `{"message":"Not Found"}` without reaching
/// the route's integration, the twin api over the same function keeps being served by it in the
/// same instant, and clearing the member puts the same address back to being served.
#[tokio::test(flavor = "multi_thread")]
async fn a_disabled_default_endpoint_refuses_at_the_address_it_published() {
    let case = "two apis over one function, one of which turns its default endpoint off";
    let (emulator, engine) = served_emulator();
    let function_name = "published-socket-disable";
    let function = given_ungranted_function(
        &emulator,
        &engine,
        case,
        function_name,
        ANSWERS_THE_MARKER,
    )
    .await;
    let route_path = "/hit";
    let stage = json!({ "stageName": DEFAULT_STAGE, "autoDeploy": true });
    let target = given_api(
        &emulator,
        case,
        "published-socket-target",
        function_name,
        &function,
        &["GET /hit"],
        stage.clone(),
    )
    .await;
    let twin = given_api(
        &emulator,
        case,
        "published-socket-twin",
        function_name,
        &function,
        &["GET /hit"],
        stage,
    )
    .await;
    // One socket serves the account's apis, each under its own path, so the two published
    // values differ in the api each names and are dialled on the one address.
    assert_ne!(
        target.base, twin.base,
        "case {case:?}: expected each api published under its own path, got {} for both",
        target.base
    );
    assert_eq!(
        target.authority, twin.authority,
        "case {case:?}: expected both apis published on one socket, got {} and {}",
        target.authority,
        twin.authority
    );

    // Both apis are served by the function to begin with, in every form a client builds the
    // invoke URL in.
    for (label, api) in [("target", &target), ("twin", &twin)] {
        for (form, path) in api.forms(route_path) {
            let answered = get(case, &api.authority, &path).await;
            assert_eq!(
                answered.status, SERVED,
                "case {case:?}: expected the {label} api served by its function at {form} \
                 ({path}), got {answered:?}"
            );
            assert!(
                answered.carries_the_functions_body(),
                "case {case:?}: expected the function's own body at {form} ({path}), got \
                 {answered:?}"
            );
        }
    }

    // The target turns its own default endpoint off: every form of its address is refused, with
    // nothing of the integration in the answer.
    set_disabled(&emulator, case, &target.api_id, true).await;
    for (form, path) in target.forms(route_path) {
        let answered = get(case, &target.authority, &path).await;
        assert_eq!(
            (answered.status, answered.message().as_str()),
            (NOT_FOUND, NOT_FOUND_MESSAGE),
            "case {case:?}: expected {NOT_FOUND} {NOT_FOUND_MESSAGE:?} on the disabled \
             endpoint at {form} ({path}), got {answered:?}"
        );
        assert!(
            !answered.carries_the_functions_body(),
            "case {case:?}: a refused request must not carry the function's body, got \
             {answered:?}"
        );
    }

    // The twin over the same function answers in the same instant: the refusal is scoped to the
    // one api whose member is set.
    for (form, path) in twin.forms(route_path) {
        let answered = get(case, &twin.authority, &path).await;
        assert_eq!(
            answered.status, SERVED,
            "case {case:?}: expected the twin api still served by the function at {form} \
             ({path}), got {answered:?}"
        );
        assert!(
            answered.carries_the_functions_body(),
            "case {case:?}: expected the function's own body from the twin at {form} ({path}), \
             got {answered:?}"
        );
    }

    // Clearing the member puts the SAME address back to being served, with no redeployment.
    set_disabled(&emulator, case, &target.api_id, false).await;
    for (form, path) in target.forms(route_path) {
        let answered = get(case, &target.authority, &path).await;
        assert_eq!(
            answered.status, SERVED,
            "case {case:?}: expected the restored endpoint served by its function at {form} \
             ({path}), got {answered:?}"
        );
        assert!(
            answered.carries_the_functions_body(),
            "case {case:?}: expected the function's own body once restored at {form} ({path}), \
             got {answered:?}"
        );
    }

    // And once the api is DELETED, the same address answers for no api at all: its root is the
    // endpoint's own `404`, not the refusal of a caller's admission.
    let (status, deleted) = control(
        &emulator,
        "DELETE",
        &format!("/v2/apis/{}", target.api_id),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: DeleteApi answered {status} {deleted}"
    );
    let answered = get(case, &target.authority, &format!("{}/", target.base)).await;
    assert_eq!(
        (answered.status, answered.message().as_str()),
        (NOT_FOUND, NOT_FOUND_MESSAGE),
        "case {case:?}: expected {NOT_FOUND} {NOT_FOUND_MESSAGE:?} at the address of a deleted \
         api, got {answered:?}"
    );
}

/// The rate and burst the stage states for the metered route, and for every other route of it.
///
/// The metered pair is written in fractions of a request a second, so the rate has repaid
/// nothing a case drew by the time the case ends however slowly the host runs it: what such a
/// route sheds is the stage's settings rather than the clock, and nothing here asserts on
/// elapsed time. The stage's own defaults are generous enough that the twin route sheds nothing.
const METERED_RATE: f64 = 0.01;
const METERED_BURST: i32 = 1;
const DEFAULT_RATE: f64 = 500.0;
const DEFAULT_BURST: i32 = 200;

/// How many requests each burst below sends at one route: more than the metered burst admits,
/// and well inside the stage's own defaults.
const BURST_REQUESTS: usize = 6;

/// RULE (the throttled route): at the address it published, a route the serving stage's
/// `RouteSettings` throttle sheds `429` `{"message":"Too Many Requests"}` without reaching the
/// route's integration, the twin route of the same stage on its `DefaultRouteSettings` sheds
/// nothing, and `DeleteRouteSettings` returns the route to those defaults.
#[tokio::test(flavor = "multi_thread")]
async fn a_throttled_route_sheds_at_the_address_it_published_while_its_twin_serves() {
    let case = "one stage metering one of its two routes";
    let (emulator, engine) = served_emulator();
    let function_name = "published-socket-throttle";
    let function = given_ungranted_function(
        &emulator,
        &engine,
        case,
        function_name,
        ANSWERS_THE_MARKER,
    )
    .await;
    let limited_key = "GET /limited";
    let api = given_api(
        &emulator,
        case,
        "published-socket-throttled",
        function_name,
        &function,
        &[limited_key, "GET /twin"],
        json!({
            "stageName": DEFAULT_STAGE,
            "autoDeploy": true,
            "defaultRouteSettings": {
                "throttlingRateLimit": DEFAULT_RATE,
                "throttlingBurstLimit": DEFAULT_BURST
            },
            "routeSettings": {
                limited_key: {
                    "throttlingRateLimit": METERED_RATE,
                    "throttlingBurstLimit": METERED_BURST
                }
            }
        }),
    )
    .await;

    // The stage reports the override for the metered route only: what the burst below shows is
    // that setting, read back from the resource the client created.
    let (status, stage) = control(
        &emulator,
        "GET",
        &format!("/v2/apis/{}/stages/%24default", api.api_id),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetStage answered {status} {stage}"
    );
    assert_eq!(
        stage.pointer("/routeSettings").cloned(),
        Some(json!({
            limited_key: {
                "throttlingRateLimit": METERED_RATE,
                "throttlingBurstLimit": METERED_BURST
            }
        })),
        "case {case:?}: GetStage must report the override for {limited_key:?} alone, got {stage}"
    );

    // One burst at each route, at the address the api published. The metered route sheds the
    // excess and the twin sheds nothing — both answered by the api, neither by the address.
    let sent = burst(case, &api, "/limited").await;
    let shed = sent
        .iter()
        .filter(|answered| answered.status == TOO_MANY_REQUESTS)
        .count();
    assert!(
        shed >= 1,
        "case {case:?}: expected the metered route to shed some of {BURST_REQUESTS} requests, \
         got {sent:?}"
    );
    for answered in &sent {
        match answered.status {
            TOO_MANY_REQUESTS => {
                assert_eq!(
                    answered.message(),
                    TOO_MANY_REQUESTS_MESSAGE,
                    "case {case:?}: a shed request must carry {TOO_MANY_REQUESTS_MESSAGE:?}, \
                     got {answered:?}"
                );
                assert!(
                    !answered.carries_the_functions_body(),
                    "case {case:?}: a shed request must not reach the route's integration, got \
                     {answered:?}"
                );
            }
            SERVED => assert!(
                answered.carries_the_functions_body(),
                "case {case:?}: a served request must carry the function's body, got {answered:?}"
            ),
            _ => panic!(
                "case {case:?}: expected the metered route either served or shed, got {answered:?}"
            ),
        }
    }
    assert_served(case, &burst(case, &api, "/twin").await, "the twin route");

    // `DeleteRouteSettings` drops the override, so the same burst at the same address meets the
    // stage's own defaults and sheds nothing.
    let (status, removed) = control(
        &emulator,
        "DELETE",
        &format!(
            "/v2/apis/{}/stages/%24default/routesettings/GET%20%2Flimited",
            api.api_id
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: DeleteRouteSettings answered {status} {removed}"
    );
    let (status, stage) = control(
        &emulator,
        "GET",
        &format!("/v2/apis/{}/stages/%24default", api.api_id),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetStage answered {status} {stage}"
    );
    assert_eq!(
        stage
            .pointer("/routeSettings")
            .and_then(Value::as_object)
            .map(|settings| settings.contains_key(limited_key)),
        Some(false),
        "case {case:?}: GetStage must no longer report an override for {limited_key:?}, got \
         {stage}"
    );
    assert_served(
        case,
        &burst(case, &api, "/limited").await,
        "the route whose override was deleted",
    );
}

/// One burst of [`BURST_REQUESTS`] requests at `route_path` of `api`, at the address the api
/// published, each on its own connection.
async fn burst(case: &str, api: &Published, route_path: &str) -> Vec<Answered> {
    let path = format!("{}{route_path}", api.base);
    let mut answers = Vec::with_capacity(BURST_REQUESTS);
    for _ in 0..BURST_REQUESTS {
        answers.push(get(case, &api.authority, &path).await);
    }
    answers
}

/// Assert every answer of `sent` is the route's function answering.
fn assert_served(case: &str, sent: &[Answered], what: &str) {
    for answered in sent {
        assert_eq!(
            answered.status, SERVED,
            "case {case:?}: expected {what} served by its function, got {answered:?}"
        );
        assert!(
            answered.carries_the_functions_body(),
            "case {case:?}: expected {what} to carry the function's body, got {answered:?}"
        );
    }
}
