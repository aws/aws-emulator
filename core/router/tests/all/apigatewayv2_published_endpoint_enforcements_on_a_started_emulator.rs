// SPDX-License-Identifier: CC0-1.0

//! What an HTTP api's PUBLISHED invoke address enforces, driven on an emulator a client
//! STARTED — the position a client of a deployed stage is in, over a real socket, with no
//! credential and with the route's function actually running.
//!
//! RULE: **the value `Api$ApiEndpoint` carries is an address a client DIALS — "the URI of the
//! API … The stage name is typically appended to this URI to form a complete path to a deployed
//! API stage" — so it is an address THIS emulator answers on, and every request made there is
//! answered from the api's own records:**
//!
//! * **an api whose `DisableExecuteApiEndpoint` is set serves NOTHING on it — `404`
//!   `{"message":"Not Found"}`, decided before the route's integration is reached — while a twin
//!   api over the same function keeps being served in the same instant, and clearing the member
//!   puts the same address back to being served ("clients can invoke your API with the default
//!   https://{api_id}.execute-api.{region}.amazonaws.com endpoint. To require that clients use a
//!   custom domain name to invoke your API, disable the default endpoint",
//!   `Api$DisableExecuteApiEndpoint`);**
//! * **a route the serving stage's `RouteSettings` meter (`ThrottlingRateLimit`,
//!   `ThrottlingBurstLimit`) sheds the part of a burst its bucket has no capacity for with `429`
//!   `{"message":"Too Many Requests"}`, again before the integration is reached, while the twin
//!   route of the same stage under its `DefaultRouteSettings` sheds nothing — and the metered
//!   route is served again as its rate refills the bucket;**
//! * **an `Origin` the request carries never decides whether it is served: a route whose
//!   `AuthorizationType` is `NONE` answers the integration's own body either way, and the only
//!   Origin-dependent difference is the `Access-Control-*` decoration the api's own
//!   `CorsConfiguration` owns — present for an origin `AllowOrigins` admits, absent for one it
//!   does not, and gone from every answer once the configuration is removed, when a preflight no
//!   route matches falls through to the endpoint's own `404` `{"message":"Not Found"}`.**
//!
//! Two crates have to agree for any of it to hold — AWS Lambda holds the function and RUNS it,
//! API Gateway v2 publishes the address, resolves the stage, the route and its integration and
//! makes every one of those decisions — so the suite lives here; and the whole point of running
//! it on a STARTED emulator is that each request crosses a real socket to the address the control
//! plane PUBLISHED, carries no credential and is answered by the binary rather than by an
//! in-process router. The in-crate suites that drive the same records
//! (`published_invoke_socket_front_door`, `published_endpoint_cors_browser_fields`) cannot see
//! that.
//!
//! The rows span what each rule generalises over: which api of two over one function is addressed
//! and both directions of the `DisableExecuteApiEndpoint` transition; the metered route against
//! its twin on the stage's defaults, and both sides of `DeleteRouteSettings`; and the browser
//! fields a request carries (none, an admitted origin, an origin the allowlist does not hold) on
//! an actual request and on a preflight, either side of `DeleteCorsConfiguration`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cfn_deployed_pipeline_on_a_started_emulator::{Started, REGION};
use crate::query_wire::opt_element;

/// The account this emulator serves, which each grant's `SourceArn` carries.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The status the function answers with and the body it returns: an answer carrying them is the
/// FUNCTION's own rather than one the gateway decided.
const SERVED: u16 = 200;
const MARKER: &str = "served-by-the-function";

/// The gateway's own refusals: an address the api serves nothing at, and a request the serving
/// stage's route settings shed.
const NOT_FOUND: u16 = 404;
const NOT_FOUND_MESSAGE: &str = "Not Found";
const TOO_MANY_REQUESTS: u16 = 429;
const TOO_MANY_REQUESTS_MESSAGE: &str = "Too Many Requests";

/// The status the GATEWAY answers a CORS preflight no route of the api matches.
const PREFLIGHT: u16 = 204;

/// The reserved stage "that serves an api's root", reached with no stage segment at all.
const DEFAULT_STAGE: &str = "$default";

/// How long one invocation of the function is given, as its own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 120;

/// How long to wait for work the emulator owes — a stack settling, a function reaching `Active`,
/// an address settling on what it serves — and how long between two looks. Generous on purpose,
/// so a loaded host never decides the outcome; nothing here asserts on elapsed time.
const BUDGET: Duration = Duration::from_secs(120);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// How many consecutive identical answers make a SETTLED answer: a client that polls an address
/// reads one answer twice over before it believes it, which is what tells an address that has
/// taken a change from one that happened to answer once.
const SETTLED_SAMPLES: usize = 3;

/// The handler every api below fronts: it answers the marker, so a `200` carrying it is the
/// function's own answer and a refusal carrying it would be the integration having been reached.
const ANSWERS_THE_MARKER: &str = "import os\n\
     def handler(event, context):\n    \
     return {'statusCode': 200, 'headers': {'content-type': 'text/plain'},\n             \
     'body': os.environ['MARKER']}\n";

/// The origin the api's `CorsConfiguration` admits, one it does not, and the `MaxAge` the
/// configuration carries (inside the modelled `range` of `Cors$MaxAge`, -1..86400).
const ALLOWED_ORIGIN: &str = "https://app.example.com";
const FOREIGN_ORIGIN: &str = "https://other.example.test";
const MAX_AGE: i64 = 300;

/// The template the function is deployed by: CloudFormation archives the inline source into a
/// deployment package this emulator can run, and the function's environment names the marker.
fn template(function: &str) -> String {
    json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": {
            "Backend": {
                "Type": "AWS::Serverless::Function",
                "Properties": {
                    "FunctionName": function,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": FUNCTION_TIMEOUT,
                    "InlineCode": ANSWERS_THE_MARKER,
                    "Environment": { "Variables": { "MARKER": MARKER } },
                },
            },
        },
    })
    .to_string()
}

/// Deploy the function as a stack of its own and answer the ARN Lambda reports for it, once it
/// reports it `Active` — "the function is ready to be invoked"
/// (`FunctionConfiguration$State`), which is the condition this waits on.
fn given_function(emulator: &Started, stack: &str, function: &str) -> String {
    let body = template(function);
    let (status, text) = emulator.cloudformation(
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &body),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
        ],
    );
    assert_eq!(
        status, 200,
        "setup: CreateStack must be accepted, got {status} with {text}; the log:\n{}",
        emulator.log()
    );
    let deadline = Instant::now() + BUDGET;
    let mut reported = String::new();
    let mut described = String::new();
    while Instant::now() < deadline {
        let (status, body) = emulator.cloudformation("DescribeStacks", &[("StackName", stack)]);
        assert_eq!(
            status, 200,
            "setup: DescribeStacks must answer 200, got {status} with {body}"
        );
        reported = opt_element(&body, "StackStatus").unwrap_or_default();
        described = body;
        if reported == "CREATE_COMPLETE"
            || reported.ends_with("_FAILED")
            || reported.contains("ROLLBACK")
        {
            break;
        }
        std::thread::sleep(LOOK_AGAIN);
    }
    assert_eq!(
        reported, "CREATE_COMPLETE",
        "setup: the stack must reach CREATE_COMPLETE, got {reported:?} with {described}; the \
         log:\n{}",
        emulator.log()
    );

    let deadline = Instant::now() + BUDGET;
    let configuration = loop {
        let (status, text) = emulator.call(
            "lambda",
            "GET",
            &format!("/2015-03-31/functions/{function}/configuration"),
            &[],
            Vec::new(),
        );
        assert_eq!(
            status, 200,
            "setup: GetFunctionConfiguration({function:?}) must answer 200, got {status} {text}"
        );
        let reported: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if reported.get("State").and_then(Value::as_str) == Some("Active")
            || Instant::now() >= deadline
        {
            break reported;
        }
        std::thread::sleep(LOOK_AGAIN);
    };
    assert_eq!(
        configuration.get("State").and_then(Value::as_str),
        Some("Active"),
        "setup: Lambda must report {function:?} Active, got {configuration}"
    );
    member("GetFunctionConfiguration", &configuration, "FunctionArn")
}

/// One signed API Gateway v2 control-plane call that has to answer `expected`, parsed.
fn apigatewayv2(
    emulator: &Started,
    what: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let (status, text) = emulator.call(
        "apigateway",
        method,
        path,
        &[("content-type", "application/json")],
        payload.into_bytes(),
    );
    assert_eq!(
        status, expected,
        "setup {what:?}: must answer HTTP {expected}, got {status} with {text}; the log:\n{}",
        emulator.log()
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// A required string member of an answer.
fn member(what: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("setup {what:?}: must carry a non-empty {name:?}, got {body}"))
}

/// Grant `apigateway.amazonaws.com` the invocation of `function`, narrowed to every stage, method
/// and path of `api_id` — the grant a client makes beside each api it fronts a route with.
fn grant(emulator: &Started, function: &str, api_id: &str, statement: &str) {
    let source_arn = format!("arn:aws:execute-api:{REGION}:{ACCOUNT}:{api_id}/*/*");
    let (status, text) = emulator.call(
        "lambda",
        "POST",
        &format!("/2015-03-31/functions/{function}/policy"),
        &[("content-type", "application/json")],
        json!({
            "StatementId": statement,
            "Action": "lambda:InvokeFunction",
            "Principal": "apigateway.amazonaws.com",
            "SourceArn": source_arn,
        })
        .to_string()
        .into_bytes(),
    );
    assert_eq!(
        status, 201,
        "setup: AddPermission({function:?}, {source_arn:?}) must answer 201, got {status} {text}"
    );
}

/// One api of a case: its id and the address it PUBLISHED, split into the authority a client
/// dials and the path it carries ahead of the stage.
struct Published {
    api_id: String,
    authority: String,
    base_path: String,
}

/// Create an api whose `routes` all front one `AWS_PROXY` integration over `function_arn`, with
/// the `stage` the case states, grant the gateway the invocation of that function for this api
/// alone, and answer the address the api published.
fn given_api(
    emulator: &Started,
    name: &str,
    function: &str,
    function_arn: &str,
    cors: Option<Value>,
    routes: &[&str],
    stage: Value,
) -> Published {
    let mut create = json!({ "name": name, "protocolType": "HTTP" });
    if let Some(cors) = cors {
        create["corsConfiguration"] = cors;
    }
    let api = apigatewayv2(emulator, "CreateApi", "POST", "/v2/apis", Some(create), 201);
    let api_id = member("CreateApi", &api, "apiId");
    let announced = member("CreateApi", &api, "apiEndpoint");
    grant(emulator, function, &api_id, &format!("may-invoke-{api_id}"));

    let integration = apigatewayv2(
        emulator,
        "CreateIntegration",
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "AWS_PROXY",
            "integrationUri": function_arn,
            "integrationMethod": "POST",
            "payloadFormatVersion": "2.0",
        })),
        201,
    );
    let integration_id = member("CreateIntegration", &integration, "integrationId");
    for route_key in routes {
        apigatewayv2(
            emulator,
            "CreateRoute",
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({
                "routeKey": route_key,
                "target": format!("integrations/{integration_id}"),
            })),
            201,
        );
    }
    apigatewayv2(
        emulator,
        "CreateStage",
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(stage),
        201,
    );

    let (authority, base_path) = published_parts(&announced);
    assert!(
        announced.starts_with("https://") && authority.contains(':'),
        "setup: an HTTP api must publish an https address a client can dial, got {announced:?}"
    );
    Published {
        api_id,
        authority,
        base_path,
    }
}

/// The authority a client dials and the path ahead of the stage, out of a published endpoint.
fn published_parts(announced: &str) -> (String, String) {
    let rest = announced
        .split_once("://")
        .map_or(announced, |(_scheme, rest)| rest);
    match rest.find('/') {
        Some(at) => (rest[..at].to_string(), rest[at..].to_string()),
        None => (rest.to_string(), String::new()),
    }
}

/// What one request to the published address was answered with.
#[derive(Debug, Clone)]
struct Answered {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Answered {
    /// The `message` the gateway's own refusals carry, or the empty string for an answer that is
    /// not one of them.
    fn message(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .as_ref()
            .and_then(|body| body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The value this answer carries for the field `name`, matched the way a field name is
    /// (case-insensitively, RFC 9110 §5.1), or `None` when it wrote none.
    fn field(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Whether the body is the one the route's function returns.
    fn carries_the_functions_body(&self) -> bool {
        self.body == MARKER
    }

    /// Every `Access-Control-*` field the answer carries, which is the only Origin-dependent
    /// difference there may be between two answers.
    fn decoration(&self) -> Vec<(String, String)> {
        let mut named: Vec<(String, String)> = self
            .headers
            .iter()
            .filter(|(field, _)| field.to_ascii_lowercase().starts_with("access-control-"))
            .map(|(field, value)| (field.to_ascii_lowercase(), value.clone()))
            .collect();
        named.sort();
        named
    }
}

/// One request a CLIENT of a published stage makes: dialled at the address the api published,
/// carrying its own authority as the `Host`, whatever browser fields the case sends and no
/// credential at all.
fn requested(
    emulator: &Started,
    api: &Published,
    method: &str,
    within: &str,
    headers: &[(&str, &str)],
) -> Answered {
    let path = format!("{}{within}", api.base_path);
    let url = format!("https://{}{path}", api.authority);
    let mut request = emulator
        .client()
        .request(
            reqwest::Method::from_bytes(method.as_bytes()).expect("a usable method"),
            &url,
        )
        .header("host", &api.authority);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request.send().unwrap_or_else(|error| {
        panic!(
            "nothing answered at {url}: {error}; the log:\n{}",
            emulator.log()
        )
    });
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    Answered {
        status,
        headers,
        body: response.text().unwrap_or_default(),
    }
}

/// A plain unsigned `GET within` at the published address.
fn get(emulator: &Started, api: &Published, within: &str) -> Answered {
    requested(emulator, api, "GET", within, &[])
}

/// Look until `within` of `api` answers `status` SETTLED — [`SETTLED_SAMPLES`] consecutive
/// answers of that status — and answer the last one seen, so a case asserts on the CONDITION
/// rather than on how fast this host runs.
fn settled(emulator: &Started, api: &Published, within: &str, status: u16) -> Answered {
    let deadline = Instant::now() + BUDGET;
    let mut last = get(emulator, api, within);
    let mut run = usize::from(last.status == status);
    while run < SETTLED_SAMPLES && Instant::now() < deadline {
        std::thread::sleep(LOOK_AGAIN);
        last = get(emulator, api, within);
        run = if last.status == status { run + 1 } else { 0 };
    }
    last
}

/// Look until `within` of `api` answers `status` ONCE, and answer the last one seen: the
/// condition for a route the stage METERS, where a settled run of answers is not a condition the
/// configuration can hold — the rate admits one request at a time and a caller looking again at
/// once is shed by the setting under test.
fn answered_once(emulator: &Started, api: &Published, within: &str, status: u16) -> Answered {
    let deadline = Instant::now() + BUDGET;
    loop {
        let answered = get(emulator, api, within);
        if answered.status == status || Instant::now() >= deadline {
            return answered;
        }
        std::thread::sleep(LOOK_AGAIN);
    }
}

/// Set `DisableExecuteApiEndpoint` on `api_id` and read the member back, so each leg starts from
/// a value a client can read.
///
/// WHICH success status this PATCH answers is `mutating_success_status`'s subject in the owning
/// crate, not this suite's: what matters here is that the update succeeded and the member is
/// readable, so any `2xx` is taken.
fn set_disabled(emulator: &Started, api_id: &str, disabled: bool) {
    let (status, text) = emulator.call(
        "apigateway",
        "PATCH",
        &format!("/v2/apis/{api_id}"),
        &[("content-type", "application/json")],
        json!({ "disableExecuteApiEndpoint": disabled })
            .to_string()
            .into_bytes(),
    );
    assert!(
        (200..300).contains(&status),
        "setup: UpdateApi must succeed, got {status} with {text}; the log:\n{}",
        emulator.log()
    );
    let updated: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    assert_eq!(
        updated.get("disableExecuteApiEndpoint"),
        Some(&json!(disabled)),
        "setup: UpdateApi must echo disableExecuteApiEndpoint={disabled}, got {updated}"
    );
    let fetched = apigatewayv2(
        emulator,
        "GetApi",
        "GET",
        &format!("/v2/apis/{api_id}"),
        None,
        200,
    );
    assert_eq!(
        fetched.get("disableExecuteApiEndpoint"),
        Some(&json!(disabled)),
        "setup: GetApi must report disableExecuteApiEndpoint={disabled}, got {fetched}"
    );
}

/// RULE (the disabled endpoint): at the address it published, an api whose
/// `DisableExecuteApiEndpoint` is set answers `404` `{"message":"Not Found"}` without reaching
/// the route's integration, the twin api over the same function keeps being served in the same
/// instant, and clearing the member puts the same address back to being served.
#[test]
fn a_disabled_default_endpoint_refuses_at_the_address_it_published() {
    let emulator = Started::start();
    let function = "apigwv2-published-disable";
    let arn = given_function(&emulator, "apigwv2-published-disable", function);
    let stage = json!({ "stageName": DEFAULT_STAGE, "autoDeploy": true });
    let target = given_api(
        &emulator,
        "published-disable-target",
        function,
        &arn,
        None,
        &["GET /hit"],
        stage.clone(),
    );
    let twin = given_api(
        &emulator,
        "published-disable-twin",
        function,
        &arn,
        None,
        &["GET /hit"],
        stage,
    );

    // Both apis are served by the function to begin with, at the address each published.
    for (which, api) in [("the target", &target), ("the twin", &twin)] {
        let answered = settled(&emulator, api, "/hit", SERVED);
        assert!(
            answered.status == SERVED && answered.carries_the_functions_body(),
            "case {which:?}: expected the api served by its function at its published address, \
             got {answered:?}; the log:\n{}",
            emulator.log()
        );
    }

    // The target turns its own default endpoint off: its address serves nothing of the api, with
    // nothing of the integration in the answer, while the twin keeps being served.
    set_disabled(&emulator, &target.api_id, true);
    let refused = settled(&emulator, &target, "/hit", NOT_FOUND);
    assert_eq!(
        (refused.status, refused.message().as_str()),
        (NOT_FOUND, NOT_FOUND_MESSAGE),
        "case {:?}: expected {NOT_FOUND} {NOT_FOUND_MESSAGE:?} on the disabled endpoint, got \
         {refused:?}; the log:\n{}",
        "the disabled target",
        emulator.log()
    );
    assert!(
        !refused.carries_the_functions_body(),
        "case {:?}: a refused request must not carry the function's body, got {refused:?}",
        "the disabled target"
    );
    let served = get(&emulator, &twin, "/hit");
    assert!(
        served.status == SERVED && served.carries_the_functions_body(),
        "case {:?}: the twin api must be served in the same instant, got {served:?}",
        "the twin of a disabled api"
    );

    // Clearing the member puts the SAME address back to being served, with no redeployment.
    set_disabled(&emulator, &target.api_id, false);
    let restored = settled(&emulator, &target, "/hit", SERVED);
    assert!(
        restored.status == SERVED && restored.carries_the_functions_body(),
        "case {:?}: expected the restored endpoint served by its function, got {restored:?}; \
         the log:\n{}",
        "the restored target",
        emulator.log()
    );
}

/// The rate and burst the stage states for the metered route, and for every other route of it.
const METERED_RATE: f64 = 1.0;
const METERED_BURST: i32 = 1;
const DEFAULT_RATE: f64 = 500.0;
const DEFAULT_BURST: i32 = 200;

/// How many requests each burst below sends at one route: more than the metered burst admits and
/// well inside the stage's own defaults.
const BURST_REQUESTS: usize = 24;

/// RULE (the throttled route): at the address it published, a route the serving stage's
/// `RouteSettings` meter sheds the part of a burst its bucket has no capacity for with `429`
/// `{"message":"Too Many Requests"}` and without reaching the route's integration, the twin route
/// of the same stage on its `DefaultRouteSettings` sheds nothing in the same instant, the metered
/// route is served again as its rate refills the bucket, and `DeleteRouteSettings` returns it to
/// those defaults.
#[test]
fn a_throttled_route_sheds_at_the_address_it_published_while_its_twin_serves() {
    let emulator = Started::start();
    let function = "apigwv2-published-throttle";
    let arn = given_function(&emulator, "apigwv2-published-throttle", function);
    let limited_key = "GET /limited";
    let api = given_api(
        &emulator,
        "published-throttled",
        function,
        &arn,
        None,
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
    );

    // The stage reports the override for the metered route only: what the burst below shows is
    // that setting, read back from the resource the client created.
    let stage = apigatewayv2(
        &emulator,
        "GetStage",
        "GET",
        &format!("/v2/apis/{}/stages/%24default", api.api_id),
        None,
        200,
    );
    assert_eq!(
        stage.pointer("/routeSettings"),
        Some(&json!({
            limited_key: {
                "throttlingRateLimit": METERED_RATE,
                "throttlingBurstLimit": METERED_BURST
            }
        })),
        "setup: GetStage must report the override for {limited_key:?} alone, got {stage}"
    );

    // A caller that waits for each answer before asking again is served on both routes: the
    // metered route's rate refills the bucket between two such requests, which is what a route
    // metered at a rate must not shed.
    for route in ["/limited", "/twin"] {
        let answered = answered_once(&emulator, &api, route, SERVED);
        assert!(
            answered.status == SERVED && answered.carries_the_functions_body(),
            "case {route:?}: a caller that waits for each answer must be served by the \
             function, got {answered:?}; the log:\n{}",
            emulator.log()
        );
    }

    // One burst at each route. The metered route sheds the excess and the twin sheds nothing —
    // both decided by the api, neither by the address.
    let sent = burst(&emulator, &api, "/limited");
    let shed = sent
        .iter()
        .filter(|answered| answered.status == TOO_MANY_REQUESTS)
        .count();
    assert!(
        shed >= 1,
        "case {:?}: expected the metered route to shed some of {BURST_REQUESTS} requests, got \
         {sent:?}; the log:\n{}",
        "a burst at the metered route",
        emulator.log()
    );
    for answered in &sent {
        match answered.status {
            TOO_MANY_REQUESTS => {
                assert_eq!(
                    answered.message(),
                    TOO_MANY_REQUESTS_MESSAGE,
                    "case {:?}: a shed request must carry {TOO_MANY_REQUESTS_MESSAGE:?}, got \
                     {answered:?}",
                    "a shed request"
                );
                assert!(
                    !answered.carries_the_functions_body(),
                    "case {:?}: a shed request must not reach the route's integration, got \
                     {answered:?}",
                    "a shed request"
                );
            }
            SERVED => assert!(
                answered.carries_the_functions_body(),
                "case {:?}: a served request must carry the function's body, got {answered:?}",
                "a served request of the metered route"
            ),
            _ => panic!(
                "case {:?}: expected the metered route either served or shed, got {answered:?}; \
                 the log:\n{}",
                "a request of the metered route",
                emulator.log()
            ),
        }
    }
    assert_served(&emulator, &burst(&emulator, &api, "/twin"), "the twin route");

    // The metered route is served again once its rate has refilled the bucket: the burst drew on
    // a budget the rate repays, it did not close the route.
    let again = answered_once(&emulator, &api, "/limited", SERVED);
    assert!(
        again.status == SERVED && again.carries_the_functions_body(),
        "case {:?}: expected the metered route served again as its rate refills, got {again:?}; \
         the log:\n{}",
        "the metered route after its burst",
        emulator.log()
    );

    // `DeleteRouteSettings` drops the override, so the same burst at the same address meets the
    // stage's own defaults and sheds nothing.
    let (status, removed) = emulator.call(
        "apigateway",
        "DELETE",
        &format!(
            "/v2/apis/{}/stages/%24default/routesettings/GET%20%2Flimited",
            api.api_id
        ),
        &[],
        Vec::new(),
    );
    assert_eq!(
        status, 204,
        "setup: DeleteRouteSettings must answer 204, got {status} with {removed}"
    );
    let stage = apigatewayv2(
        &emulator,
        "GetStage",
        "GET",
        &format!("/v2/apis/{}/stages/%24default", api.api_id),
        None,
        200,
    );
    assert_eq!(
        stage
            .pointer("/routeSettings")
            .and_then(Value::as_object)
            .map(|settings| settings.contains_key(limited_key)),
        Some(false),
        "setup: GetStage must no longer report an override for {limited_key:?}, got {stage}"
    );
    assert_served(
        &emulator,
        &burst(&emulator, &api, "/limited"),
        "the route whose override was deleted",
    );
}

/// One burst of [`BURST_REQUESTS`] requests at `within` of `api`, at the address the api
/// published.
fn burst(emulator: &Started, api: &Published, within: &str) -> Vec<Answered> {
    (0..BURST_REQUESTS)
        .map(|_| get(emulator, api, within))
        .collect()
}

/// Assert every answer of `sent` is the route's function answering.
fn assert_served(emulator: &Started, sent: &[Answered], what: &str) {
    for answered in sent {
        assert!(
            answered.status == SERVED && answered.carries_the_functions_body(),
            "case {what:?}: expected every request served by the function, got {answered:?}; \
             the log:\n{}",
            emulator.log()
        );
    }
}

/// RULE (the browser fields): at the address it published, an `Origin` never decides whether an
/// api's data plane serves a request — the route's integration answers either way — and the only
/// Origin-dependent difference is the `Access-Control-*` decoration the api's own
/// `CorsConfiguration` owns; once that configuration is removed, no answer carries one and a
/// preflight no route matches falls through to the endpoint's own `404`.
#[test]
fn an_origin_only_decides_the_decoration_at_the_address_it_published() {
    let emulator = Started::start();
    let function = "apigwv2-published-cors";
    let arn = given_function(&emulator, "apigwv2-published-cors", function);
    let api = given_api(
        &emulator,
        "published-cors",
        function,
        &arn,
        Some(json!({
            "allowOrigins": [ALLOWED_ORIGIN],
            "allowMethods": ["GET"],
            "maxAge": MAX_AGE,
        })),
        &["GET /a"],
        json!({ "stageName": DEFAULT_STAGE, "autoDeploy": true }),
    );

    // The route answers before any browser field is sent, which is the control every row below
    // is read against.
    let plain = settled(&emulator, &api, "/a", SERVED);
    assert!(
        plain.status == SERVED && plain.carries_the_functions_body(),
        "case {:?}: expected the route's own answer, got {plain:?}; the log:\n{}",
        "a request carrying no browser field",
        emulator.log()
    );
    assert!(
        plain.decoration().is_empty(),
        "case {:?}: a request carrying no Origin earns no decoration, got {:?}",
        "a request carrying no browser field",
        plain.decoration()
    );

    // The rows: the origin sent, and the `access-control-allow-origin` the answer must carry.
    for (case, origin, granted) in [
        (
            "an origin the configuration admits",
            ALLOWED_ORIGIN,
            Some(ALLOWED_ORIGIN),
        ),
        ("an origin it does not hold", FOREIGN_ORIGIN, None),
    ] {
        let answered = requested(&emulator, &api, "GET", "/a", &[("origin", origin)]);
        assert!(
            answered.status == SERVED && answered.carries_the_functions_body(),
            "case {case:?}: an Origin never decides whether the route is served, got \
             {answered:?}; the log:\n{}",
            emulator.log()
        );
        assert_eq!(
            answered.field("access-control-allow-origin"),
            granted,
            "case {case:?}: expected access-control-allow-origin {granted:?}, got {answered:?}"
        );
    }

    // A preflight no route of the api matches is the GATEWAY's own answer, out of the api's own
    // configuration: the origin echoed, the method admitted and the configured max age.
    let preflight = requested(
        &emulator,
        &api,
        "OPTIONS",
        "/a",
        &[
            ("origin", ALLOWED_ORIGIN),
            ("access-control-request-method", "GET"),
            ("access-control-request-headers", "content-type"),
        ],
    );
    assert_eq!(
        preflight.status, PREFLIGHT,
        "case {:?}: expected the gateway's own {PREFLIGHT}, got {preflight:?}; the log:\n{}",
        "a preflight no route matches",
        emulator.log()
    );
    assert_eq!(
        preflight.field("access-control-allow-origin"),
        Some(ALLOWED_ORIGIN),
        "case {:?}: expected the origin echoed, got {preflight:?}",
        "a preflight no route matches"
    );
    assert!(
        preflight
            .field("access-control-allow-methods")
            .is_some_and(|methods| methods.split(',').any(|method| method.trim() == "GET")),
        "case {:?}: expected GET among the methods the configuration admits, got {preflight:?}",
        "a preflight no route matches"
    );
    assert_eq!(
        preflight.field("access-control-max-age"),
        Some(MAX_AGE.to_string().as_str()),
        "case {:?}: expected the configured max age, got {preflight:?}",
        "a preflight no route matches"
    );

    // The configuration is REMOVED in place, which keeps the api and its id.
    let (status, text) = emulator.call(
        "apigateway",
        "DELETE",
        &format!("/v2/apis/{}/cors", api.api_id),
        &[],
        Vec::new(),
    );
    assert_eq!(
        status, 204,
        "setup: DeleteCorsConfiguration must answer 204, got {status} with {text}"
    );
    let fetched = apigatewayv2(
        &emulator,
        "GetApi",
        "GET",
        &format!("/v2/apis/{}", api.api_id),
        None,
        200,
    );
    assert_eq!(
        fetched.get("corsConfiguration"),
        None,
        "setup: GetApi must no longer report a corsConfiguration, got {fetched}"
    );

    // The same preflight now falls through to route matching, which nothing matches, so it is the
    // endpoint's own `404` with no decoration at all; and the plain GET still answers the
    // integration's own body, now without one either.
    let fell_through = requested(
        &emulator,
        &api,
        "OPTIONS",
        "/a",
        &[
            ("origin", ALLOWED_ORIGIN),
            ("access-control-request-method", "GET"),
        ],
    );
    assert_eq!(
        (fell_through.status, fell_through.message().as_str()),
        (NOT_FOUND, NOT_FOUND_MESSAGE),
        "case {:?}: expected {NOT_FOUND} {NOT_FOUND_MESSAGE:?} once no configuration answers it, \
         got {fell_through:?}; the log:\n{}",
        "a preflight with the configuration removed",
        emulator.log()
    );
    assert!(
        fell_through.decoration().is_empty(),
        "case {:?}: expected no access-control field at all, got {:?}",
        "a preflight with the configuration removed",
        fell_through.decoration()
    );
    let still_served = requested(&emulator, &api, "GET", "/a", &[("origin", ALLOWED_ORIGIN)]);
    assert!(
        still_served.status == SERVED && still_served.carries_the_functions_body(),
        "case {:?}: the route keeps answering with no configuration, got {still_served:?}; the \
         log:\n{}",
        "an Origin-bearing GET with the configuration removed",
        emulator.log()
    );
    assert!(
        still_served.decoration().is_empty(),
        "case {:?}: expected no access-control field at all, got {:?}",
        "an Origin-bearing GET with the configuration removed",
        still_served.decoration()
    );
}
